//! Preset-driven character loading. Only curated assets/character files are read.
use crate::curated::CollectionPaths;
use crate::{
    AssetIssue, CharacterBody, CharacterSlot, ClothingItem, Texture, TextureId, TextureStore,
    UiChoice, WardrobeCatalog, resolve_resource,
};
use anyhow::{Context, Result, bail, ensure};
use glam::{Mat4, Vec3};
use sdo_platform::assets as fs;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Clone)]
pub struct CharacterBone {
    pub name: String,
    pub local: Mat4,
    pub global: Mat4,
    pub parent: Option<usize>,
}
#[derive(Debug, Clone)]
pub struct CharacterSkeleton {
    pub source: PathBuf,
    pub bones: Vec<CharacterBone>,
}
#[derive(Debug, Clone, Copy)]
pub struct CharacterVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}
#[derive(Debug, Clone)]
pub struct CharacterSurface {
    pub vertices: Vec<CharacterVertex>,
    pub indices: Vec<u32>,
    pub texture: TextureId,
    pub diffuse: [f32; 4],
    pub skinning: Arc<CharacterSkinning>,
}
#[derive(Debug)]
pub struct CharacterSkinning {
    pub vertices: Vec<CharacterBinding>,
    pub offsets: Vec<Mat4>,
}
#[derive(Debug)]
pub struct CharacterBinding {
    pub position: Vec3,
    pub normal: Vec3,
    pub bones: [usize; 4],
    pub weights: [f32; 4],
}
impl CharacterSurface {
    /// Skin original export vertices once using their inverse bind matrices.
    pub fn posed_vertices(&self, globals: &[Mat4]) -> Vec<CharacterVertex> {
        let mut matrices = Vec::new();
        self.skin_vertices(globals, &mut matrices).collect()
    }
    pub fn skin_vertices<'a>(
        &'a self,
        globals: &[Mat4],
        matrices: &'a mut Vec<Mat4>,
    ) -> impl Iterator<Item = CharacterVertex> + 'a {
        matrices.clear();
        matrices.extend(
            globals
                .iter()
                .zip(&self.skinning.offsets)
                .map(|(g, o)| *g * *o),
        );
        self.skinning
            .vertices
            .iter()
            .zip(&self.vertices)
            .map(move |(v, original)| {
                let (mut p, mut n) = (Vec3::ZERO, Vec3::ZERO);
                for i in 0..4 {
                    if v.weights[i].abs() < 1e-6 {
                        continue;
                    }
                    let m = matrices[v.bones[i]];
                    p += m.transform_point3(v.position) * v.weights[i];
                    n += m.transform_vector3(v.normal) * v.weights[i];
                }
                CharacterVertex {
                    position: p.to_array(),
                    normal: n.normalize_or_zero().to_array(),
                    uv: original.uv,
                }
            })
    }
}
#[derive(Debug, Clone)]
pub struct CharacterPart {
    pub source: PathBuf,
    pub surfaces: Vec<CharacterSurface>,
    bind_locals: Vec<Option<Mat4>>,
}
impl CharacterPart {
    /// Provider-produced geometry already assembled in the skeleton's bind pose.
    pub fn from_surfaces(source: PathBuf, surfaces: Vec<CharacterSurface>) -> Self {
        Self {
            source,
            surfaces,
            bind_locals: Vec::new(),
        }
    }
}
/// A curated motion filename plus an explicit application playback rate.
/// MOT files contain frame keys but do not declare a playback rate.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterAnimation {
    pub motion: String,
    pub frames_per_second: f32,
}
#[derive(Debug, Clone)]
pub struct CharacterPreset {
    pub choice: UiChoice,
    pub entry: PathBuf,
    pub skeleton: Arc<CharacterSkeleton>,
    pub parts: Vec<Arc<CharacterPart>>,
    pub bounds: [[f32; 3]; 2],
    pub idle: Option<CharacterAnimation>,
    pub walk: Option<CharacterAnimation>,
    pub miss: Option<CharacterAnimation>,
    pub body: Option<CharacterBody>,
    pub equipment: BTreeMap<CharacterSlot, String>,
}
type Outfits = BTreeMap<String, BTreeMap<CharacterSlot, String>>;
/// Preset ID -> slot key -> canonical wearable ID. Only user overrides are saved,
/// including separates temporarily hidden by a full outfit.
pub type OutfitOverrides = BTreeMap<String, BTreeMap<String, String>>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ClothingPreviewKey {
    pub preset: String,
    pub item: String,
}
/// Immutable source for background item previews. It never changes equipment.
#[derive(Clone)]
pub struct ClothingPreviewSource {
    root: PathBuf,
    wardrobe: Arc<WardrobeCatalog>,
    rigs: BTreeMap<String, (Option<CharacterBody>, Arc<CharacterSkeleton>)>,
}
impl ClothingPreviewSource {
    pub(crate) fn load(&self, key: &ClothingPreviewKey) -> Result<CharacterAssets> {
        let item = self.wardrobe.item(&key.item)?;
        let (body, hierarchy) = self
            .rigs
            .get(&key.preset)
            .context("Unknown preview character")?;
        ensure!(item.fits(*body), "Item does not fit this character");
        let data = self
            .root
            .parent()
            .context("Character root has no assets parent")?;
        let path = resolve_resource(data, item.preview_resource())?;
        ensure!(
            path.starts_with(&self.root),
            "Preview resource is outside assets/character"
        );
        let mut store = TextureStore::from_root(&self.root)?;
        let part = Arc::new(
            parse_mesh(&path, &fs::read(&path)?, hierarchy, &mut store)
                .with_context(|| format!("Cannot preview {}", path.display()))?,
        );
        // Use the mesh's authored bind geometry with the curated bone hierarchy.
        // A thumbnail is framed around that item, not around a whole character.
        let (rig, parts) = assemble_bind_pose(hierarchy, &[part])?;
        let bounds = geometry_bounds(&parts)?;
        Ok(CharacterAssets {
            root: self.root.clone(),
            presets: vec![CharacterPreset {
                choice: UiChoice {
                    id: item.id.clone(),
                    name: item.name.clone(),
                },
                entry: path,
                skeleton: rig,
                parts,
                bounds,
                idle: None,
                walk: None,
                miss: None,
                body: *body,
                equipment: BTreeMap::new(),
            }],
            textures: Arc::new(store.textures),
            resources: store.resources.into_iter().collect(),
            issues: vec![],
            wardrobe: self.wardrobe.clone(),
            outfits: Outfits::new(),
        })
    }
}

fn geometry_bounds(parts: &[Arc<CharacterPart>]) -> Result<[[f32; 3]; 2]> {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for v in parts
        .iter()
        .flat_map(|p| &p.surfaces)
        .flat_map(|s| &s.vertices)
    {
        let p = Vec3::from_array(v.position);
        min = min.min(p);
        max = max.max(p);
    }
    ensure!(
        min.is_finite() && max.is_finite() && (max - min).max_element() > 0.01,
        "Empty character geometry"
    );
    Ok([min.to_array(), max.to_array()])
}
#[derive(Clone)]
pub struct CharacterAssets {
    pub root: PathBuf,
    pub presets: Vec<CharacterPreset>,
    pub textures: Arc<Vec<Texture>>,
    pub resources: Vec<PathBuf>,
    pub issues: Vec<AssetIssue>,
    pub wardrobe: Arc<WardrobeCatalog>,
    outfits: Outfits,
}
#[derive(Serialize)]
pub struct CharacterReport {
    pub root: PathBuf,
    pub presets: Vec<UiChoice>,
    pub mesh_parts: usize,
    pub surfaces: usize,
    pub triangles: usize,
    pub decoded_textures: usize,
    pub decoded_bytes: usize,
    pub resource_files: usize,
    pub issues: Vec<AssetIssue>,
    pub clothing_items: usize,
    pub clothing_companions: usize,
    pub unavailable_clothing_items: usize,
    pub unclassified_meshes: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PresetDefinition {
    schema_version: u32,
    name: String,
    skeleton: String,
    meshes: BTreeMap<String, String>,
    #[serde(default)]
    idle: Option<CharacterAnimation>,
    #[serde(default)]
    walk: Option<CharacterAnimation>,
    #[serde(default)]
    miss: Option<CharacterAnimation>,
}
impl CharacterAssets {
    /// Content identity is independent of catalog IDs, package names and paths.
    pub fn preset_sha256(&self, id: &str) -> Result<String> {
        use sha2::{Digest, Sha256};
        let preset = self.preset(id)?;
        let mut hash = Sha256::new();
        let mut floats = |values: &[f32]| {
            for value in values {
                hash.update(value.to_le_bytes());
            }
        };
        for bone in &preset.skeleton.bones {
            floats(&bone.local.to_cols_array());
            floats(&bone.global.to_cols_array());
        }
        for part in &preset.parts {
            for surface in &part.surfaces {
                floats(&surface.diffuse);
                for v in &surface.vertices {
                    floats(&v.position);
                    floats(&v.normal);
                    floats(&v.uv);
                }
                for matrix in &surface.skinning.offsets {
                    floats(&matrix.to_cols_array());
                }
                for vertex in &surface.skinning.vertices {
                    floats(&vertex.position.to_array());
                    floats(&vertex.normal.to_array());
                    floats(&vertex.weights);
                }
            }
        }
        for bone in &preset.skeleton.bones {
            hash.update((bone.name.len() as u64).to_le_bytes());
            hash.update(bone.name.as_bytes());
            hash.update((bone.parent.unwrap_or(usize::MAX) as u64).to_le_bytes());
        }
        for part in &preset.parts {
            for surface in &part.surfaces {
                for vertex in &surface.skinning.vertices {
                    for bone in vertex.bones {
                        hash.update((bone as u64).to_le_bytes());
                    }
                }
                for index in &surface.indices {
                    hash.update(index.to_le_bytes());
                }
                let texture = &self.textures[surface.texture.0];
                hash.update(texture.width.to_le_bytes());
                hash.update(texture.height.to_le_bytes());
                hash.update(&texture.rgba);
            }
        }
        Ok(format!("{:x}", hash.finalize()))
    }
    /// Build a normalized character collection without a filesystem or preset JSON.
    pub fn new(
        presets: Vec<CharacterPreset>,
        textures: Arc<Vec<Texture>>,
        wardrobe: Arc<WardrobeCatalog>,
        outfits: &OutfitOverrides,
    ) -> Result<Self> {
        let mut assets = Self {
            root: PathBuf::new(),
            presets,
            textures,
            wardrobe,
            resources: Vec::new(),
            issues: Vec::new(),
            outfits: Outfits::new(),
        };
        assets.outfits = assets.validate_outfits(outfits)?;
        Ok(assets)
    }
    pub(crate) fn preview_source(&self) -> ClothingPreviewSource {
        ClothingPreviewSource {
            root: self.root.clone(),
            wardrobe: self.wardrobe.clone(),
            rigs: self
                .presets
                .iter()
                .map(|p| (p.choice.id.clone(), (p.body, p.skeleton.clone())))
                .collect(),
        }
    }
    pub(crate) fn load(paths: &CollectionPaths) -> Result<Self> {
        let root = paths.assets().join("character");
        if !fs::is_dir(&root) {
            return Ok(Self {
                root: root.clone(),
                presets: Vec::new(),
                textures: Arc::new(Vec::new()),
                resources: Vec::new(),
                wardrobe: Arc::new(WardrobeCatalog::default()),
                outfits: Outfits::new(),
                issues: vec![AssetIssue {
                    component: "Character presets".into(),
                    path: root,
                    message: "Missing assets/character directory".into(),
                }],
            });
        }
        Self::load_root(root)
    }
    pub fn load_root(root: impl AsRef<Path>) -> Result<Self> {
        Self::load_outfits(root.as_ref(), Outfits::new(), None)
    }
    fn load_outfits(
        root: &Path,
        outfits: Outfits,
        wardrobe: Option<Arc<WardrobeCatalog>>,
    ) -> Result<Self> {
        let root = fs::canonicalize(root).context("Cannot open assets/character")?;
        let wardrobe = match wardrobe {
            Some(catalog) => catalog,
            None => Arc::new(WardrobeCatalog::discover(&root)?),
        };
        let data = root
            .parent()
            .context("Character root has no assets parent")?;
        let mut store = TextureStore::from_root(&root)?;
        let mut rigs = HashMap::<PathBuf, Arc<CharacterSkeleton>>::new();
        let mut meshes = HashMap::<(PathBuf, PathBuf), Arc<CharacterPart>>::new();
        let mut assembled = HashMap::new();
        let mut presets = Vec::new();
        let mut issues = Vec::new();
        fn entries(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
            let mut children = fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
            children.sort_by_key(|p| p.file_name());
            for p in children {
                let t = p.file_type()?;
                ensure!(!t.is_symlink(), "Symlinked character preset");
                if t.is_dir() {
                    entries(&p.path(), out)?;
                } else if p
                    .path()
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("json"))
                {
                    out.push(p.path());
                }
            }
            Ok(())
        }
        let folder = root.join("presets");
        let mut paths = Vec::new();
        if let Err(e) = entries(&folder, &mut paths) {
            issues.push(AssetIssue {
                component: "Character presets".into(),
                path: folder.clone(),
                message: format!("{e:#}"),
            });
        }
        for path in paths {
            let mut load = || -> Result<CharacterPreset> {
                let mut def: PresetDefinition = serde_json::from_slice(&fs::read(&path)?)
                    .context("Invalid character preset JSON")?;
                ensure!(
                    def.schema_version == 1,
                    "Unsupported character preset schema {}",
                    def.schema_version
                );
                ensure!(!def.name.trim().is_empty(), "Empty character preset name");
                ensure!(!def.meshes.is_empty(), "Character preset has no mesh parts");
                let id = path
                    .strip_prefix(&folder)?
                    .with_extension("")
                    .to_string_lossy()
                    .replace('\\', "/");
                let body = def
                    .meshes
                    .values()
                    .filter_map(|name| {
                        ClothingItem::from_filename(Path::new(name).file_name()?.to_str()?)
                    })
                    .map(|item| item.body)
                    .find(|body| *body != CharacterBody::Unisex);
                if let Some(outfit) = outfits.get(&id) {
                    for (slot, item) in outfit {
                        // Preset labels are user-defined; replace by the mesh's slot.
                        def.meshes.retain(|_, name| {
                            Path::new(name)
                                .file_name()
                                .and_then(|name| name.to_str())
                                .and_then(ClothingItem::from_filename)
                                .is_none_or(|part| part.slot != *slot)
                        });
                        // Native equipped avatars use the primary DRESS resource.
                        // Underscore companions belong to standalone shop previews;
                        // changing pants or an outfit does not select that mesh.
                        def.meshes
                            .insert(slot.key().into(), wardrobe.item(item)?.resource.clone());
                    }
                    if outfit.contains_key(&CharacterSlot::Outfit) {
                        def.meshes.retain(|_, name| {
                            Path::new(name)
                                .file_name()
                                .and_then(|name| name.to_str())
                                .and_then(ClothingItem::from_filename)
                                .is_none_or(|part| {
                                    !matches!(part.slot, CharacterSlot::Top | CharacterSlot::Pants)
                                })
                        });
                    }
                }
                let equipment = def
                    .meshes
                    .values()
                    .filter_map(|name| {
                        ClothingItem::from_filename(Path::new(name).file_name()?.to_str()?)
                    })
                    .map(|item| (item.slot, item.id))
                    .collect();
                let file = |name: &str| -> Result<PathBuf> {
                    let p = resolve_resource(data, name)?;
                    ensure!(
                        p.starts_with(&root),
                        "Preset resource is outside assets/character"
                    );
                    Ok(p)
                };
                let skeleton_path = file(&def.skeleton)?;
                let rig = if let Some(rig) = rigs.get(&skeleton_path) {
                    rig.clone()
                } else {
                    let rig = Arc::new(parse_skeleton(&skeleton_path, &fs::read(&skeleton_path)?)?);
                    rigs.insert(skeleton_path.clone(), rig.clone());
                    rig
                };
                store.resources.insert(skeleton_path.clone());
                store.resources.insert(path.clone());
                let mut parts = Vec::new();
                for (label, name) in def.meshes {
                    ensure!(!label.trim().is_empty(), "Empty mesh slot name");
                    let source = file(&name)?;
                    let key = (source.clone(), skeleton_path.clone());
                    let part = if let Some(part) = meshes.get(&key) {
                        part.clone()
                    } else {
                        let part = Arc::new(
                            parse_mesh(&source, &fs::read(&source)?, &rig, &mut store)
                                .with_context(|| {
                                    format!("Cannot load {label}: {}", source.display())
                                })?,
                        );
                        meshes.insert(key, part.clone());
                        part
                    };
                    parts.push(part);
                }
                let key = (
                    skeleton_path,
                    parts.iter().map(|p| p.source.clone()).collect::<Vec<_>>(),
                );
                let (rig, parts) = if let Some(cached) = assembled.get(&key) {
                    let cached: &(Arc<CharacterSkeleton>, Vec<Arc<CharacterPart>>) = cached;
                    cached.clone()
                } else {
                    let bound = assemble_bind_pose(&rig, &parts)?;
                    assembled.insert(key, bound.clone());
                    bound
                };
                let bounds = geometry_bounds(&parts)?;
                Ok(CharacterPreset {
                    choice: UiChoice { id, name: def.name },
                    entry: path.clone(),
                    skeleton: rig,
                    parts,
                    bounds,
                    idle: def.idle,
                    walk: def.walk,
                    miss: def.miss,
                    body,
                    equipment,
                })
            };
            match load() {
                Ok(p) => presets.push(p),
                Err(e) => issues.push(AssetIssue {
                    component: "Character preset".into(),
                    path,
                    message: format!("{e:#}"),
                }),
            }
        }
        if presets.is_empty() {
            issues.push(AssetIssue {
                component: "Character presets".into(),
                path: folder,
                message: "No usable character presets".into(),
            });
        }
        Ok(Self {
            root,
            presets,
            textures: Arc::new(store.textures),
            resources: store.resources.into_iter().collect(),
            issues,
            wardrobe,
            outfits,
        })
    }
    pub fn choices(&self) -> Vec<UiChoice> {
        self.presets.iter().map(|p| p.choice.clone()).collect()
    }
    pub fn outfit_overrides(&self) -> OutfitOverrides {
        self.outfits
            .iter()
            .filter(|(_, outfit)| !outfit.is_empty())
            .map(|(id, outfit)| {
                (
                    id.clone(),
                    outfit
                        .iter()
                        .map(|(slot, item)| (slot.key().into(), item.clone()))
                        .collect(),
                )
            })
            .collect()
    }
    /// Validate every saved item, then rebuild once. Failed restores leave the
    /// current presets intact, just like a failed equip operation.
    pub(crate) fn with_outfit_overrides(&self, saved: &OutfitOverrides) -> Result<Self> {
        self.rebuild_outfits(self.validate_outfits(saved)?)
    }
    fn validate_outfits(&self, saved: &OutfitOverrides) -> Result<Outfits> {
        let mut outfits = Outfits::new();
        for (id, outfit) in saved {
            let preset = self.preset(id)?;
            let mut restored = BTreeMap::new();
            for (slot, id) in outfit {
                let item = self.wardrobe.item(id)?;
                item.ensure_available()?;
                ensure!(
                    item.slot.key() == slot,
                    "{} belongs in the {} slot, not {slot}",
                    item.name,
                    item.slot.key()
                );
                ensure!(
                    self.wardrobe
                        .is_compatible(&preset.choice.id, preset.body, item),
                    "{} does not fit {}",
                    item.name,
                    preset.choice.name
                );
                restored.insert(item.slot, item.id.clone());
            }
            outfits.insert(preset.choice.id.clone(), restored);
        }
        Ok(outfits)
    }
    /// Build and validate a replacement before changing the live outfit or GPU data.
    #[cfg(test)]
    fn with_equipment(&self, id: &str, slot: CharacterSlot, item: Option<&str>) -> Result<Self> {
        self.with_outfit_overrides(&self.equipment_overrides(id, slot, item)?)
    }
    /// Validate a wardrobe change without performing any I/O. The asset provider
    /// builds the replacement model from the returned outfit request.
    pub fn equipment_overrides(
        &self,
        id: &str,
        slot: CharacterSlot,
        item: Option<&str>,
    ) -> Result<OutfitOverrides> {
        let preset = self.preset(id)?;
        let mut outfits = self.outfit_overrides();
        let outfit = outfits.entry(preset.choice.id.clone()).or_default();
        if let Some(id) = item {
            let item = self.wardrobe.item(id)?;
            item.ensure_available()?;
            ensure!(
                item.slot == slot,
                "{} belongs in the {} slot",
                item.name,
                item.slot.label()
            );
            ensure!(
                self.wardrobe
                    .is_compatible(&preset.choice.id, preset.body, item),
                "{} does not fit this character",
                item.name
            );
            outfit.insert(slot.key().into(), item.id.clone());
        } else {
            outfit.remove(slot.key());
        }
        if matches!(slot, CharacterSlot::Top | CharacterSlot::Pants) {
            outfit.remove(CharacterSlot::Outfit.key());
        }
        Ok(outfits)
    }
    #[cfg(test)]
    fn with_default_outfit(&self, id: &str) -> Result<Self> {
        let mut outfits = self.outfits.clone();
        outfits.remove(&self.preset(id)?.choice.id);
        self.rebuild_outfits(outfits)
    }
    fn rebuild_outfits(&self, outfits: Outfits) -> Result<Self> {
        let next = Self::load_outfits(&self.root, outfits, Some(self.wardrobe.clone()))?;
        ensure!(
            self.presets
                .iter()
                .all(|p| next.preset(&p.choice.id).is_ok()),
            "{}",
            next.issues
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        );
        Ok(next)
    }
    pub fn preset(&self, id: &str) -> Result<&CharacterPreset> {
        self.presets
            .iter()
            .find(|p| {
                p.choice.id.eq_ignore_ascii_case(id) || p.choice.name.eq_ignore_ascii_case(id)
            })
            .with_context(|| format!("Unknown or unavailable character preset {id:?}"))
    }
    pub fn report(&self) -> CharacterReport {
        CharacterReport {
            root: self.root.clone(),
            presets: self.choices(),
            mesh_parts: self.presets.iter().map(|p| p.parts.len()).sum(),
            surfaces: self
                .presets
                .iter()
                .flat_map(|p| &p.parts)
                .map(|p| p.surfaces.len())
                .sum(),
            triangles: self
                .presets
                .iter()
                .flat_map(|p| &p.parts)
                .flat_map(|p| &p.surfaces)
                .map(|s| s.indices.len() / 3)
                .sum(),
            decoded_textures: self.textures.len(),
            decoded_bytes: self.textures.iter().map(|t| t.rgba.len()).sum(),
            resource_files: self.resources.len(),
            issues: self.issues.clone(),
            clothing_items: self.wardrobe.items.len(),
            clothing_companions: self
                .wardrobe
                .items
                .iter()
                .filter(|item| item.companion_resource.is_some())
                .count(),
            unavailable_clothing_items: self
                .wardrobe
                .items
                .iter()
                .filter(|item| item.unavailable_reason.is_some())
                .count(),
            unclassified_meshes: self.wardrobe.unclassified_meshes,
        }
    }
}
pub(crate) struct Reader<'a> {
    pub(crate) raw: &'a [u8],
    pub(crate) at: usize,
}
impl<'a> Reader<'a> {
    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(
            n <= self.raw.len().saturating_sub(self.at),
            "Truncated character asset at byte {}",
            self.at
        );
        let p = &self.raw[self.at..self.at + n];
        self.at += n;
        Ok(p)
    }
    pub(crate) fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub(crate) fn f32(&mut self) -> Result<f32> {
        let v = f32::from_le_bytes(self.take(4)?.try_into().unwrap());
        ensure!(v.is_finite(), "Nonfinite character value");
        Ok(v)
    }
    pub(crate) fn matrix(&mut self) -> Result<Mat4> {
        let mut f = [0.; 16];
        for v in &mut f {
            *v = self.f32()?;
        }
        let m = Mat4::from_cols_array(&f);
        ensure!(
            m.is_finite() && m.determinant().abs() > 1e-8,
            "Invalid character matrix"
        );
        Ok(m)
    }
    pub(crate) fn done(&self) -> Result<()> {
        ensure!(
            self.at == self.raw.len(),
            "Unparsed character asset bytes at {}",
            self.at
        );
        Ok(())
    }
}
fn name(bytes: &[u8]) -> Result<String> {
    Ok(
        std::str::from_utf8(bytes.split(|b| *b == 0).next().unwrap_or_default())
            .context("Unsupported non-UTF-8 material/bone name")?
            .trim()
            .to_owned(),
    )
}
impl CharacterSkeleton {
    pub fn parse(source: impl Into<PathBuf>, raw: &[u8]) -> Result<Self> {
        parse_skeleton(&source.into(), raw)
    }
}
fn parse_skeleton(path: &Path, raw: &[u8]) -> Result<CharacterSkeleton> {
    let mut r = Reader { raw, at: 0 };
    ensure!(r.take(12)? == b"Hierachy0020", "Unsupported HRC format");
    let count = r.u32()? as usize;
    ensure!((1..=512).contains(&count), "Invalid skeleton bone count");
    let mut bones = Vec::new();
    let mut links = Vec::new();
    for _ in 0..count {
        let local = r.matrix()?;
        let child = r.u32()? as usize;
        let sibling = r.u32()? as usize;
        r.take(12)?;
        let name = name(r.take(28)?)?;
        ensure!(child < count && sibling < count, "Bone link out of range");
        links.push((child, sibling));
        bones.push(CharacterBone {
            name,
            local,
            global: Mat4::IDENTITY,
            parent: None,
        });
    }
    r.done()?;
    fn walk(
        i: usize,
        parent: Option<usize>,
        links: &[(usize, usize)],
        bones: &mut [CharacterBone],
        seen: &mut [bool],
    ) -> Result<()> {
        ensure!(!seen[i], "Skeleton cycle or multiple parents at bone {i}");
        seen[i] = true;
        bones[i].parent = parent;
        bones[i].global =
            parent.map(|p| bones[p].global).unwrap_or(Mat4::IDENTITY) * bones[i].local;
        let (c, s) = links[i];
        if c != 0 {
            walk(c, Some(i), links, bones, seen)?;
        }
        if s != 0 {
            walk(s, parent, links, bones, seen)?;
        }
        Ok(())
    }
    let mut seen = vec![false; count];
    walk(0, None, &links, &mut bones, &mut seen)?;
    ensure!(seen.into_iter().all(|v| v), "Unreachable skeleton bones");
    Ok(CharacterSkeleton {
        source: path.into(),
        bones,
    })
}
#[derive(Clone)]
struct Material {
    texture: String,
    diffuse: [f32; 4],
}
fn merge_bind_local(existing: &mut Option<Mat4>, local: Mat4, bone: &str) -> Result<()> {
    ensure!(local.is_finite(), "Nonfinite mesh bind for {bone}");
    if let Some(previous) = existing {
        ensure!(
            previous.abs_diff_eq(local, 0.001),
            "Mesh parts disagree on bind joint {bone}"
        );
    } else {
        *existing = Some(local);
    }
    Ok(())
}

/// HRC supplies bone names and hierarchy, but its stored reference proportions
/// need not be the mesh's rest pose. Assemble that pose from the mesh inverse
/// binds; retain HRC only for joints with no authored mesh constraint.
fn assemble_bind_pose(
    hierarchy: &CharacterSkeleton,
    parts: &[Arc<CharacterPart>],
) -> Result<(Arc<CharacterSkeleton>, Vec<Arc<CharacterPart>>)> {
    let mut locals = vec![None; hierarchy.bones.len()];
    for part in parts {
        for (i, local) in part.bind_locals.iter().enumerate() {
            if let Some(local) = local {
                merge_bind_local(&mut locals[i], *local, &hierarchy.bones[i].name).with_context(
                    || format!("Incompatible character mesh {}", part.source.display()),
                )?;
            }
        }
    }
    let mut rig = hierarchy.clone();
    for (bone, local) in rig.bones.iter_mut().zip(locals) {
        if let Some(local) = local {
            bone.local = local;
        }
    }
    fn global(i: usize, rig: &CharacterSkeleton, out: &mut [Option<Mat4>]) -> Mat4 {
        if let Some(m) = out[i] {
            return m;
        }
        let m = rig.bones[i]
            .parent
            .map(|p| global(p, rig, out))
            .unwrap_or(Mat4::IDENTITY)
            * rig.bones[i].local;
        out[i] = Some(m);
        m
    }
    let mut globals = vec![None; rig.bones.len()];
    for i in 0..globals.len() {
        global(i, &rig, &mut globals);
    }
    let mut globals = globals.into_iter().map(Option::unwrap).collect::<Vec<_>>();
    // Assemble the rest mesh on the stage floor. The HRC reference root height
    // belongs to its old proportions and cannot frame this mesh or its motions.
    let floor = parts
        .iter()
        .flat_map(|p| &p.surfaces)
        .flat_map(|s| s.posed_vertices(&globals))
        .map(|v| v.position[1])
        .fold(f32::INFINITY, f32::min);
    if floor.is_finite() {
        let shift = Mat4::from_translation(Vec3::new(0., -floor, 0.));
        for global in &mut globals {
            *global = shift * *global;
        }
        for bone in &mut rig.bones {
            if bone.parent.is_none() {
                bone.local = shift * bone.local;
            }
        }
    }
    for (bone, m) in rig.bones.iter_mut().zip(&globals) {
        bone.global = *m;
    }
    let parts = parts
        .iter()
        .map(|part| {
            let mut part = (**part).clone();
            for surface in &mut part.surfaces {
                surface.vertices = surface.posed_vertices(&globals);
            }
            Arc::new(part)
        })
        .collect();
    Ok((Arc::new(rig), parts))
}

fn parse_mesh(
    path: &Path,
    raw: &[u8],
    rig: &CharacterSkeleton,
    store: &mut TextureStore,
) -> Result<CharacterPart> {
    let mut r = Reader { raw, at: 0 };
    ensure!(r.take(12)? == b"Mesh00000030", "Unsupported MSH format");
    let count = r.u32()? as usize;
    ensure!((1..=10000).contains(&count), "Invalid mesh section count");
    store.resources.insert(path.into());
    let parent = path.parent().context("Mesh has no parent")?;
    let mut surfaces = Vec::new();
    let mut bind_locals = vec![None; rig.bones.len()];
    for _ in 0..count {
        let fvf = r.u32()?;
        let index_bytes = r.u32()? as usize;
        let fmt = r.u32()?;
        let index_size = match fmt {
            101 => 2,
            102 => 4,
            _ => bail!("Unsupported MSH index format {fmt}"),
        };
        ensure!(
            index_bytes.is_multiple_of(index_size),
            "Invalid index buffer size"
        );
        let indices = r
            .take(index_bytes)?
            .chunks_exact(index_size)
            .map(|b| {
                if index_size == 2 {
                    u16::from_le_bytes(b.try_into().unwrap()) as u32
                } else {
                    u32::from_le_bytes(b.try_into().unwrap())
                }
            })
            .collect::<Vec<_>>();
        ensure!(indices.len().is_multiple_of(3), "Incomplete MSH triangle");
        let vertex_bytes = r.u32()? as usize;
        let stride = r.u32()? as usize;
        let nweights = match fvf & 0xe {
            6 => 0,
            8 => 1,
            10 => 2,
            12 => 3,
            14 => 4,
            _ => bail!("Unsupported MSH position format {fvf:#x}"),
        };
        ensure!(
            nweights <= 3 && fvf == 0x1150 + (fvf & 0xe) && stride == 40 + nweights * 4,
            "Unsupported MSH FVF/stride {fvf:#x}/{stride}"
        );
        ensure!(
            vertex_bytes.is_multiple_of(stride),
            "Invalid MSH vertex buffer"
        );
        let bytes = r.take(vertex_bytes)?;
        r.take(24)?;
        let nm = r.u32()? as usize;
        ensure!((1..=10000).contains(&nm), "Invalid material count");
        let mut materials = Vec::new();
        for _ in 0..nm {
            let b = r.take(408)?;
            let mut m = Reader { raw: b, at: 0 };
            let mut diffuse = [0.; 4];
            for c in &mut diffuse {
                *c = m.f32()?;
            }
            materials.push(Material {
                texture: name(&b[68..404])?,
                diffuse,
            });
        }
        let kind = r.u32()?;
        let influences = r.u32()?;
        let group_bytes = r.u32()? as usize;
        let ng = r.u32()? as usize;
        ensure!(
            kind == 0,
            "Unsupported MSH section kind {kind}; expected skinned geometry"
        );
        ensure!(
            (1..=4).contains(&influences) && ng > 0 && group_bytes.is_multiple_of(ng),
            "Invalid mesh group table"
        );
        let entry_size = group_bytes / ng;
        ensure!(
            entry_size >= 24 && (entry_size - 24).is_multiple_of(4),
            "Invalid bone palette size"
        );
        let mut groups = Vec::new();
        for _ in 0..ng {
            let mut g = [0u32; 6];
            for v in &mut g {
                *v = r.u32()?;
            }
            groups.push(g);
        }
        let mut palettes = Vec::new();
        for _ in 0..ng {
            let mut p = Vec::new();
            for _ in 0..(entry_size - 24) / 4 {
                p.push(r.u32()?);
            }
            palettes.push(p);
        }
        let bone_count = r.u32()? as usize;
        ensure!(
            bone_count == rig.bones.len(),
            "Mesh and preset skeleton bone counts differ"
        );
        let matrix_count = r.u32()? as usize;
        ensure!(matrix_count <= bone_count, "Invalid inverse-bind count");
        let mut offsets = Vec::new();
        for _ in 0..matrix_count {
            offsets.push(r.matrix()?);
        }
        let mut mapping = Vec::new();
        for _ in 0..bone_count {
            mapping.push(r.u32()?);
        }
        // Exported mesh parts can have different world origins. Adjacent inverse
        // binds recover the same local joints without depending on those origins.
        for (i, bone) in rig.bones.iter().enumerate() {
            if let Some(parent) = bone.parent
                && let (Some(child_offset), Some(parent_offset)) = (
                    offsets.get(mapping[i] as usize),
                    offsets.get(mapping[parent] as usize),
                )
            {
                let local = *parent_offset * child_offset.inverse();
                merge_bind_local(&mut bind_locals[i], local, &bone.name)?;
            }
        }
        for (g, palette) in groups.into_iter().zip(palettes) {
            let material = materials
                .get(g[0] as usize)
                .context("Mesh group material out of range")?;
            let start = g[1] as usize * 3;
            let end = start + g[2] as usize * 3;
            ensure!(
                start < end && end <= indices.len(),
                "Invalid triangle group range"
            );
            let vstart = g[3] as usize;
            let vend = vstart + g[4] as usize;
            ensure!(
                vstart < vend && vend <= vertex_bytes / stride,
                "Invalid vertex group range"
            );
            let mut matrices = Vec::new();
            for &bone in &palette {
                if bone == u32::MAX {
                    matrices.push(None);
                    continue;
                }
                let b = bone as usize;
                ensure!(b < bone_count, "Palette bone out of range");
                let offset = offsets
                    .get(mapping[b] as usize)
                    .context("Missing inverse-bind matrix")?;
                matrices.push(Some(rig.bones[b].global * *offset));
            }
            let mut vertices = Vec::new();
            let mut bindings = Vec::new();
            let bone_offsets = mapping
                .iter()
                .map(|i| offsets.get(*i as usize).copied().unwrap_or(Mat4::IDENTITY))
                .collect::<Vec<_>>();
            for b in bytes[vstart * stride..vend * stride].chunks_exact(stride) {
                let mut v = Reader { raw: b, at: 0 };
                let pos = Vec3::new(v.f32()?, v.f32()?, v.f32()?);
                let mut weights = [0.; 4];
                let mut sum = 0.;
                for w in weights.iter_mut().take(nweights) {
                    *w = v.f32()?;
                    ensure!(*w >= -0.001 && *w <= 1.001, "Invalid skin weight");
                    sum += *w;
                }
                weights[nweights] = 1. - sum;
                ensure!(weights[nweights] >= -0.002, "Skin weights exceed one");
                let ids = v.take(4)?;
                let normal = Vec3::new(v.f32()?, v.f32()?, v.f32()?);
                v.take(4)?;
                let uv = [v.f32()?, v.f32()?];
                let (mut p, mut n) = (Vec3::ZERO, Vec3::ZERO);
                for i in 0..=nweights {
                    if weights[i].abs() < 1e-6 {
                        continue;
                    }
                    let m = matrices
                        .get(ids[i] as usize)
                        .and_then(Option::as_ref)
                        .context("Vertex references missing palette entry")?;
                    p += m.transform_point3(pos) * weights[i];
                    n += m.transform_vector3(normal) * weights[i];
                }
                ensure!(p.is_finite() && n.is_finite(), "Invalid skinned vertex");
                let mut bones = [0usize; 4];
                for i in 0..=nweights {
                    if weights[i].abs() >= 1e-6 {
                        bones[i] = palette[ids[i] as usize] as usize;
                    }
                }
                bindings.push(CharacterBinding {
                    position: pos,
                    normal,
                    bones,
                    weights,
                });
                vertices.push(CharacterVertex {
                    position: p.to_array(),
                    normal: n.normalize_or_zero().to_array(),
                    uv,
                });
            }
            let mut out_indices = Vec::new();
            for &i in &indices[start..end] {
                ensure!(
                    (vstart..vend).contains(&(i as usize)),
                    "Triangle index escapes its group"
                );
                out_indices.push(i - vstart as u32);
            }
            ensure!(
                !material.texture.starts_with('_'),
                "Animated/procedural material {:?} needs a supported playback definition",
                material.texture
            );
            let texture = if material.texture.is_empty() {
                TextureId(0)
            } else {
                store.image(resolve_resource(parent, &material.texture)?)?
            };
            surfaces.push(CharacterSurface {
                vertices,
                indices: out_indices,
                texture,
                diffuse: material.diffuse,
                skinning: Arc::new(CharacterSkinning {
                    vertices: bindings,
                    offsets: bone_offsets,
                }),
            });
        }
    }
    r.done()?;
    ensure!(!surfaces.is_empty(), "Mesh has no renderable surfaces");
    Ok(CharacterPart {
        source: path.into(),
        surfaces,
        bind_locals,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn word(b: &mut Vec<u8>, v: u32) {
        b.extend(v.to_le_bytes());
    }
    fn float(b: &mut Vec<u8>, v: f32) {
        b.extend(v.to_le_bytes());
    }
    pub(crate) fn matrix(b: &mut Vec<u8>, m: Mat4) {
        for f in m.to_cols_array() {
            float(b, f);
        }
    }
    fn skeleton() -> Vec<u8> {
        let mut b = b"Hierachy0020".to_vec();
        word(&mut b, 2);
        for (x, child, label) in [(10., 1, "Root"), (4., 0, "Child")] {
            matrix(&mut b, Mat4::from_translation(Vec3::new(x, 0., 0.)));
            word(&mut b, child);
            word(&mut b, 0);
            b.extend([0; 12]);
            let mut name = [0u8; 28];
            name[..label.len()].copy_from_slice(label.as_bytes());
            b.extend(name);
        }
        b
    }
    fn mesh() -> Vec<u8> {
        let mut b = b"Mesh00000030".to_vec();
        word(&mut b, 1);
        for v in [0x1158, 6, 101] {
            word(&mut b, v);
        }
        for i in [0u16, 1, 2] {
            b.extend(i.to_le_bytes());
        }
        word(&mut b, 3 * 44);
        word(&mut b, 44);
        for pos in [[0., 0., 0.], [1., 0., 0.], [0., 2., 0.]] {
            for f in pos {
                float(&mut b, f);
            }
            float(&mut b, 1.);
            word(&mut b, 0);
            for f in [0., 0., 1.] {
                float(&mut b, f);
            }
            word(&mut b, 0);
            float(&mut b, 0.);
            float(&mut b, 0.);
        }
        b.extend([0; 24]);
        word(&mut b, 1);
        let mut material = vec![0; 408];
        for i in 0..4 {
            material[i * 4..i * 4 + 4].copy_from_slice(&1f32.to_le_bytes());
        }
        material[68..73].copy_from_slice(b"a.PNG");
        b.extend(material);
        for v in [0, 2, 28, 1, 0, 0, 1, 0, 3, 24, 1, 2, 1] {
            word(&mut b, v);
        }
        matrix(&mut b, Mat4::from_translation(Vec3::new(-4., 0., 0.)));
        word(&mut b, u32::MAX);
        word(&mut b, 0);
        b
    }
    #[test]
    fn hierarchy_palette_weights_and_preset_cache_work_without_source_files() -> Result<()> {
        let fixture = Fixture(std::env::temp_dir().join(format!(
            "sdo-character-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        let root = fixture.0.join("assets/character");
        fs::create_dir_all(root.join("presets"))?;
        fs::create_dir_all(root.join("skin"))?;
        fs::write(root.join("rig.HRC"), skeleton())?;
        fs::write(root.join("skin/basic.MSH"), mesh())?;
        image::RgbaImage::from_pixel(1, 1, image::Rgba([255u8, 0, 0, 255]))
            .save(root.join("skin/a.PNG"))?;
        for id in ["first", "second"] {
            fs::write(
                root.join(format!("presets/{id}.json")),
                serde_json::to_vec(
                    &serde_json::json!({"schema_version":1,"name":id,"skeleton":"character/rig.HRC","meshes":{"body":"character/skin/basic.MSH"}}),
                )?,
            )?;
        }
        let assets = CharacterAssets::load_root(&root)?;
        assert!(assets.issues.is_empty(), "{:?}", assets.issues);
        assert_eq!(assets.presets.len(), 2);
        let a = assets.preset("first")?;
        let b = assets.preset("second")?;
        assert!(Arc::ptr_eq(&a.parts[0], &b.parts[0]));
        assert!(Arc::ptr_eq(&a.skeleton, &b.skeleton));
        assert_eq!(a.skeleton.bones[1].parent, Some(0));
        assert_eq!(a.parts[0].surfaces[0].vertices[0].position, [10., 0., 0.]);
        assert_eq!(a.parts[0].surfaces[0].indices, [0, 1, 2]);
        assert_eq!(assets.textures.len(), 2);
        let surface = &a.parts[0].surfaces[0];
        let globals = a
            .skeleton
            .bones
            .iter()
            .map(|b| b.global)
            .collect::<Vec<_>>();
        let restored = surface.posed_vertices(&globals);
        for (a, b) in restored.iter().zip(&surface.vertices) {
            assert!(Vec3::from_array(a.position).distance(Vec3::from_array(b.position)) < 0.0001);
        }
        let offline = fixture.0.with_extension("offline");
        fs::rename(&fixture.0, &offline)?;
        assert_eq!(
            assets.preset("second")?.bounds,
            [[10., 0., 0.], [11., 2., 0.]]
        );
        fs::rename(offline, &fixture.0)?;
        fs::write(root.join("presets/broken.json"),br#"{"schema_version":1,"name":"Broken","skeleton":"../source.HRC","meshes":{"body":"character/skin/basic.MSH"}}"#)?;
        let assets = CharacterAssets::load_root(&root)?;
        assert_eq!(assets.presets.len(), 2);
        assert_eq!(assets.issues.len(), 1);
        assert!(assets.issues[0].message.contains("Invalid resource path"));
        Ok(())
    }
    #[test]
    fn wardrobe_equips_atomically_preserves_other_presets_and_restores_slots() -> Result<()> {
        let fixture = Fixture(std::env::temp_dir().join(format!(
            "sdo-wardrobe-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        let root = fixture.0.join("assets/character");
        fs::create_dir_all(root.join("presets"))?;
        fs::create_dir_all(root.join("skin"))?;
        fs::write(root.join("rig.HRC"), skeleton())?;
        image::RgbaImage::from_pixel(1, 1, image::Rgba([255u8, 0, 0, 255]))
            .save(root.join("skin/a.PNG"))?;
        for body in ["MAN", "WOMAN"] {
            let meshes = [
                ("hair", "HAIR"),
                ("upper_body", "COAT"),
                ("lower_body", "PANT"),
            ]
            .into_iter()
            .map(|(slot, kind)| (slot, format!("character/skin/000001_{body}_{kind}.MSH")))
            .collect::<BTreeMap<_, _>>();
            for path in meshes.values() {
                fs::write(root.parent().unwrap().join(path), mesh())?;
            }
            fs::write(
                root.join(format!("presets/{body}.json")),
                serde_json::to_vec(&serde_json::json!({
                "schema_version":1,"name":body,"skeleton":"character/rig.HRC","meshes":meshes}))?,
            )?;
        }
        for name in [
            "000002_MAN_HAIR.MSH",
            "000002_MAN_COAT.MSH",
            "000002_MAN_ONE.MSH",
            "000002_MAN_SHOES.MSH",
            "000002_WOMAN_HAIR.MSH",
        ] {
            fs::write(root.join("skin").join(name), mesh())?;
        }
        fs::write(root.join("skin/000003_MAN_HAIR.MSH"), b"broken mesh")?;
        // A companion must never be loaded as an independent wearable, even if
        // requested through its old browser ID. Orphans cannot be equipped.
        fs::write(root.join("skin/000002_MAN_COAT_.MSH"), b"partial companion")?;
        fs::write(root.join("skin/000002_MAN_SHOES_.MSH"), b"shop companion")?;
        fs::write(root.join("skin/000003_MAN_SHOES_.MSH"), mesh())?;
        fs::write(root.join("skin/000004_MAN_SHOES.MSH"), b"invalid primary")?;
        fs::write(root.join("skin/000004_MAN_SHOES_.MSH"), mesh())?;
        let original_json = fs::read(root.join("presets/MAN.json"))?;
        let initial = CharacterAssets::load_root(&root)?;
        assert!(initial.issues.is_empty()); // Discovery doesn't decode unselected meshes.
        let source = initial.preview_source();
        let preview_key = |item: &str| ClothingPreviewKey {
            preset: "MAN".into(),
            item: item.into(),
        };
        let preview = source.load(&preview_key("000004_MAN_SHOES.MSH"))?;
        assert_eq!(preview.presets[0].parts.len(), 1);
        assert!(
            preview.presets[0].parts[0]
                .source
                .ends_with("000004_MAN_SHOES_.MSH")
        );
        // A missing primary can still have a shop preview, but cannot be equipped.
        assert!(source.load(&preview_key("000003_MAN_SHOES.MSH")).is_ok());
        // A broken designated preview is reported, never silently replaced by the primary.
        assert!(source.load(&preview_key("000002_MAN_SHOES.MSH")).is_err());
        let hair_preview = source.load(&preview_key("000002_MAN_HAIR.MSH"))?;
        assert!(
            hair_preview.presets[0].parts[0]
                .source
                .ends_with("000002_MAN_HAIR.MSH")
        );
        assert_eq!(initial.preset("MAN")?.parts.len(), 3);
        let male =
            initial.with_equipment("MAN", CharacterSlot::Hair, Some("000002_MAN_HAIR.MSH"))?;
        assert_eq!(
            male.preset("MAN")?.equipment[&CharacterSlot::Hair],
            "000002_MAN_HAIR.MSH"
        );
        assert!(
            male.preset("MAN")?
                .parts
                .iter()
                .any(|p| p.source.ends_with("000002_MAN_HAIR.MSH"))
        );
        assert_eq!(
            male.preset("WOMAN")?.equipment,
            initial.preset("WOMAN")?.equipment
        );
        assert!(
            male.with_equipment("MAN", CharacterSlot::Top, Some("000002_MAN_HAIR.MSH"))
                .is_err()
        );
        assert!(
            male.with_equipment("MAN", CharacterSlot::Hair, Some("000002_WOMAN_HAIR.MSH"))
                .is_err()
        );
        assert!(
            male.with_equipment("MAN", CharacterSlot::Hair, Some("../outside.MSH"))
                .is_err()
        );
        let failure = male
            .with_equipment("MAN", CharacterSlot::Hair, Some("000003_MAN_HAIR.MSH"))
            .err()
            .expect("Invalid mesh must fail");
        assert!(format!("{failure:#}").contains("Cannot load hair"));
        assert_eq!(
            male.preset("MAN")?.equipment[&CharacterSlot::Hair],
            "000002_MAN_HAIR.MSH"
        );
        let both =
            male.with_equipment("WOMAN", CharacterSlot::Hair, Some("000002_WOMAN_HAIR.MSH"))?;
        assert_eq!(both.preset("MAN")?.equipment, male.preset("MAN")?.equipment);
        let top = both.with_equipment("MAN", CharacterSlot::Top, Some("000002_MAN_COAT_.MSH"))?;
        assert_eq!(
            top.preset("MAN")?.equipment[&CharacterSlot::Top],
            "000002_MAN_COAT.MSH"
        );
        let orphan_error = top
            .with_equipment("MAN", CharacterSlot::Shoes, Some("000003_MAN_SHOES_.MSH"))
            .err()
            .expect("Missing complete mesh must be rejected");
        assert!(
            orphan_error
                .to_string()
                .contains("Complete mesh 000003_MAN_SHOES.MSH is missing")
        );
        assert_eq!(
            top.preset("MAN")?.parts.len(),
            3,
            "The previous top must be replaced, not layered"
        );
        let shoes =
            top.with_equipment("MAN", CharacterSlot::Shoes, Some("000002_MAN_SHOES_.MSH"))?;
        let outfit =
            shoes.with_equipment("MAN", CharacterSlot::Outfit, Some("000002_MAN_ONE.MSH"))?;
        assert_eq!(outfit.preset("MAN")?.parts.len(), 3);
        assert!(
            !outfit
                .preset("MAN")?
                .equipment
                .contains_key(&CharacterSlot::Top)
        );
        assert!(
            !outfit
                .preset("MAN")?
                .equipment
                .contains_key(&CharacterSlot::Pants)
        );
        let separated = outfit.with_equipment("MAN", CharacterSlot::Outfit, None)?;
        // Reloading saved overrides keeps both presets and the separates hidden
        // under an outfit. Removing that outfit restores the customized top.
        let serialized = serde_json::to_vec(&outfit.outfit_overrides())?;
        let saved: OutfitOverrides = serde_json::from_slice(&serialized)?;
        let reloaded = initial.with_outfit_overrides(&saved)?;
        assert_eq!(
            reloaded.preset("MAN")?.equipment,
            outfit.preset("MAN")?.equipment
        );
        assert_eq!(
            reloaded.preset("WOMAN")?.equipment,
            outfit.preset("WOMAN")?.equipment
        );
        assert_eq!(
            reloaded
                .with_equipment("MAN", CharacterSlot::Outfit, None)?
                .preset("MAN")?
                .equipment,
            separated.preset("MAN")?.equipment
        );
        let mut invalid = saved.clone();
        invalid
            .get_mut("MAN")
            .unwrap()
            .insert("shoes".into(), "000003_MAN_SHOES_.MSH".into());
        assert!(initial.with_outfit_overrides(&invalid).is_err());
        let reset_saved = reloaded.with_default_outfit("MAN")?.outfit_overrides();
        let reset_reloaded = initial.with_outfit_overrides(&reset_saved)?;
        assert_eq!(
            reset_reloaded.preset("MAN")?.equipment,
            initial.preset("MAN")?.equipment
        );
        assert_eq!(
            reset_reloaded.preset("WOMAN")?.equipment,
            outfit.preset("WOMAN")?.equipment
        );
        for assets in [&shoes, &outfit, &separated] {
            let preset = assets.preset("MAN")?;
            assert_eq!(
                preset.equipment[&CharacterSlot::Shoes],
                "000002_MAN_SHOES.MSH"
            );
            assert!(
                preset
                    .parts
                    .iter()
                    .any(|part| part.source.ends_with("000002_MAN_SHOES.MSH"))
            );
            assert!(
                !preset
                    .parts
                    .iter()
                    .any(|part| part.source.ends_with("000002_MAN_SHOES_.MSH"))
            );
        }
        assert_eq!(
            separated.preset("MAN")?.equipment[&CharacterSlot::Top],
            "000002_MAN_COAT.MSH"
        );
        let restored_top = outfit.with_equipment("MAN", CharacterSlot::Top, None)?;
        assert!(
            !restored_top
                .preset("MAN")?
                .equipment
                .contains_key(&CharacterSlot::Outfit)
        );
        assert_eq!(
            restored_top.preset("MAN")?.equipment[&CharacterSlot::Top],
            "000001_MAN_COAT.MSH"
        );
        let reset = separated.with_default_outfit("MAN")?;
        assert_eq!(
            reset.preset("MAN")?.equipment,
            initial.preset("MAN")?.equipment
        );
        assert_eq!(
            reset.preset("WOMAN")?.equipment,
            both.preset("WOMAN")?.equipment
        );
        assert_eq!(fs::read(root.join("presets/MAN.json"))?, original_json);
        Ok(())
    }
    #[test]
    fn mesh_bind_proportions_override_reference_pose_and_incompatible_parts_fail() -> Result<()> {
        let hierarchy = CharacterSkeleton {
            source: "reference.HRC".into(),
            bones: vec![
                CharacterBone {
                    name: "root".into(),
                    local: Mat4::IDENTITY,
                    global: Mat4::IDENTITY,
                    parent: None,
                },
                CharacterBone {
                    name: "hand".into(),
                    local: Mat4::from_translation(Vec3::X * 10.),
                    global: Mat4::from_translation(Vec3::X * 10.),
                    parent: Some(0),
                },
            ],
        };
        let part = Arc::new(CharacterPart {
            source: "body.MSH".into(),
            surfaces: vec![],
            bind_locals: vec![None, Some(Mat4::from_translation(Vec3::X * 3.5))],
        });
        let (bind, _) = assemble_bind_pose(&hierarchy, std::slice::from_ref(&part))?;
        assert!((bind.bones[1].global.w_axis.x - 3.5).abs() < 0.0001);
        assert_eq!(
            hierarchy.bones[1].global.w_axis.x, 10.,
            "Source hierarchy stays unchanged"
        );
        let mut incompatible = (*part).clone();
        incompatible.bind_locals[1] = Some(Mat4::from_translation(Vec3::X * 7.));
        assert!(assemble_bind_pose(&hierarchy, &[part, Arc::new(incompatible)]).is_err());
        Ok(())
    }

    #[test]
    fn truncated_or_cyclic_skeletons_are_rejected() {
        let p = Path::new("test.HRC");
        let mut raw = skeleton();
        assert!(parse_skeleton(p, &raw[..50]).is_err());
        raw[16 + 112 + 64..16 + 112 + 68].copy_from_slice(&1u32.to_le_bytes());
        assert!(parse_skeleton(p, &raw).is_err());
    }
}
