//! Lightweight discovery of the curated clothing library. Meshes load on equip.
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum CharacterBody {
    Male,
    Female,
    Unisex,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ClothingCategory {
    #[default]
    Hair,
    Tops,
    Pants,
    Shoes,
    Outfits,
    Faces,
    Hands,
    Accessories,
}
impl ClothingCategory {
    pub const ALL: [Self; 8] = [
        Self::Hair,
        Self::Tops,
        Self::Pants,
        Self::Shoes,
        Self::Outfits,
        Self::Faces,
        Self::Hands,
        Self::Accessories,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Hair => "Hair",
            Self::Tops => "Tops",
            Self::Pants => "Pants",
            Self::Shoes => "Shoes",
            Self::Outfits => "Outfits",
            Self::Faces => "Faces",
            Self::Hands => "Hands",
            Self::Accessories => "Accessories",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum CharacterSlot {
    Hair,
    Face,
    Top,
    Pants,
    Hands,
    Shoes,
    Outfit,
    Glasses,
    Wings,
    Prop,
    Ornament,
    Ring,
    Mask,
    Headwear,
    UpperArm,
    Waist,
    Knee,
    Shoulder,
    Tail,
    Expression,
}
impl CharacterSlot {
    pub const BASE: [Self; 7] = [
        Self::Hair,
        Self::Face,
        Self::Top,
        Self::Pants,
        Self::Hands,
        Self::Shoes,
        Self::Outfit,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Hair => "hair",
            Self::Face => "face",
            Self::Top => "coat",
            Self::Pants => "pants",
            Self::Hands => "hands",
            Self::Shoes => "shoes",
            Self::Outfit => "outfit",
            Self::Glasses => "glasses",
            Self::Wings => "wings",
            Self::Prop => "prop",
            Self::Ornament => "ornament",
            Self::Ring => "ring",
            Self::Mask => "mask",
            Self::Headwear => "headwear",
            Self::UpperArm => "upper_arm",
            Self::Waist => "waist",
            Self::Knee => "knee",
            Self::Shoulder => "shoulder",
            Self::Tail => "tail",
            Self::Expression => "expression",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Top => "Top",
            Self::UpperArm => "Upper arm",
            Self::Hair => "Hair",
            Self::Face => "Face",
            Self::Pants => "Pants",
            Self::Hands => "Hands",
            Self::Shoes => "Shoes",
            Self::Outfit => "Outfit",
            Self::Glasses => "Glasses",
            Self::Wings => "Wings",
            Self::Prop => "Prop",
            Self::Ornament => "Ornament",
            Self::Ring => "Ring",
            Self::Mask => "Mask",
            Self::Headwear => "Headwear",
            Self::Waist => "Waist",
            Self::Knee => "Knee",
            Self::Shoulder => "Shoulder",
            Self::Tail => "Tail",
            Self::Expression => "Expression",
        }
    }
    pub fn category(self) -> ClothingCategory {
        match self {
            Self::Hair => ClothingCategory::Hair,
            Self::Face => ClothingCategory::Faces,
            Self::Top => ClothingCategory::Tops,
            Self::Pants => ClothingCategory::Pants,
            Self::Shoes => ClothingCategory::Shoes,
            Self::Hands => ClothingCategory::Hands,
            Self::Outfit => ClothingCategory::Outfits,
            _ => ClothingCategory::Accessories,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClothingItem {
    /// Original primary mesh filename. Internal clothing companions share this ID.
    pub id: String,
    pub name: String,
    pub body: CharacterBody,
    pub slot: CharacterSlot,
    /// Complete mesh for an equipped character, independent of pants/dress.
    pub resource: String,
    /// Filename companion of this wearable. Only categories verified in the
    /// native client select this resource for their standalone shop thumbnail.
    pub companion_resource: Option<String>,
    pub unavailable_reason: Option<String>,
    pub search: String,
}
impl ClothingItem {
    pub fn from_filename(filename: &str) -> Option<Self> {
        let path = Path::new(filename);
        if path.file_name()?.to_str()? != filename || !path.extension()?.eq_ignore_ascii_case("msh")
        {
            return None;
        }
        let stem = path.file_stem()?.to_str()?;
        // The native loader inserts one underscore. Multiple trailing underscores
        // are ambiguous source resources, not another verified wearable variant.
        if stem.ends_with("__") {
            return None;
        }
        let mut words = stem.split('_');
        let id = words.next()?;
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let body = match words.next()?.to_ascii_uppercase().as_str() {
            "MAN" | "MANSTAR" => CharacterBody::Male,
            "WOMAN" | "WOMANSTAR" => CharacterBody::Female,
            "ALL" => CharacterBody::Unisex,
            _ => return None,
        };
        let slot = match words.next()?.trim().to_ascii_uppercase().as_str() {
            "HAIR" => CharacterSlot::Hair,
            "FACE" | "STARFACE" => CharacterSlot::Face,
            "COAT" => CharacterSlot::Top,
            "PANT" => CharacterSlot::Pants,
            "HAND" => CharacterSlot::Hands,
            "SHOES" | "SHOSE" => CharacterSlot::Shoes,
            "ONE" => CharacterSlot::Outfit,
            "GLASS" | "GLAS" => CharacterSlot::Glasses,
            "CHIBANG" | "CHIBANG1" | "CHIBANG2" | "CHIBANG3" => CharacterSlot::Wings,
            "DAOJU" => CharacterSlot::Prop,
            "LINGDANG" | "LINGDANG1" | "LINGDANG2" | "LINGDANG3" => CharacterSlot::Ornament,
            "RING" | "JIEZHI" => CharacterSlot::Ring,
            "MASK" => CharacterSlot::Mask,
            "PATE" => CharacterSlot::Headwear,
            "UPARM" => CharacterSlot::UpperArm,
            "WAIST" => CharacterSlot::Waist,
            "KNEE" => CharacterSlot::Knee,
            "SHOULDER" => CharacterSlot::Shoulder,
            "TAIL" => CharacterSlot::Tail,
            "EXPRESSION" => CharacterSlot::Expression,
            _ => return None,
        };
        let suffix = words.collect::<Vec<_>>().join("_");
        let name = format!(
            "{} {}{}{}",
            slot.label(),
            id,
            if suffix.trim_matches('_').is_empty() {
                String::new()
            } else {
                format!(" · {}", suffix.trim_matches('_'))
            },
            if stem.ends_with('_') {
                " · Mesh variant"
            } else {
                ""
            }
        );
        Some(Self {
            id: filename.into(),
            search: format!("{name} {filename}").to_lowercase(),
            name,
            body,
            slot,
            resource: format!("character/skin/{filename}"),
            companion_resource: None,
            unavailable_reason: None,
        })
    }
    fn primary_filename(&self) -> Option<String> {
        // Group filename companions as resources of one wearable. This does not
        // imply that every category's companion is a valid shop-preview mesh.
        let path = Path::new(&self.id);
        let stem = path.file_stem()?.to_str()?.strip_suffix('_')?;
        Some(format!("{stem}.{}", path.extension()?.to_str()?))
    }
    pub fn preview_resource(&self) -> &str {
        // Other categories include unrelated/mislabeled companions in the source data.
        if matches!(
            self.slot,
            CharacterSlot::Top
                | CharacterSlot::Pants
                | CharacterSlot::Shoes
                | CharacterSlot::Outfit
                | CharacterSlot::Prop
        ) && let Some(resource) = &self.companion_resource
        {
            return resource;
        }
        &self.resource
    }
    pub fn ensure_available(&self) -> Result<()> {
        if let Some(reason) = &self.unavailable_reason {
            anyhow::bail!("{} cannot be equipped: {reason}", self.name);
        }
        Ok(())
    }
    pub fn fits(&self, body: Option<CharacterBody>) -> bool {
        body.is_some_and(|body| self.body == body || self.body == CharacterBody::Unisex)
    }
}

#[derive(Debug, Default)]
pub struct WardrobeCatalog {
    pub items: Vec<ClothingItem>,
    pub unclassified_meshes: usize,
    companion_items: BTreeMap<String, String>,
    compatible_presets: BTreeMap<String, Vec<String>>,
}
impl WardrobeCatalog {
    /// Build a shop catalog from a provider's normalized records. Native filename
    /// parsing and companion discovery are specific to the curated adapter.
    pub fn from_items(mut items: Vec<ClothingItem>) -> Result<Self> {
        items.sort_by(|a, b| a.id.cmp(&b.id));
        ensure!(
            items.iter().all(|item| !item.id.is_empty()),
            "Empty wardrobe item ID"
        );
        ensure!(
            items.windows(2).all(|pair| pair[0].id != pair[1].id),
            "Duplicate wardrobe item ID"
        );
        Ok(Self {
            items,
            ..Self::default()
        })
    }
    /// Optional provider-authored associations. An unrestricted catalog retains
    /// the original body compatibility rules.
    pub fn restrict_to_presets(&mut self, item: &str, presets: Vec<String>) -> Result<()> {
        self.item(item)?;
        self.compatible_presets.insert(item.into(), presets);
        Ok(())
    }
    pub fn is_compatible(
        &self,
        preset: &str,
        body: Option<CharacterBody>,
        item: &ClothingItem,
    ) -> bool {
        item.fits(body)
            && self
                .compatible_presets
                .get(&item.id)
                .is_none_or(|ids| ids.iter().any(|id| id == preset))
    }
    pub fn discover(root: &Path) -> Result<Self> {
        let folder = root.join("skin");
        if !folder.is_dir() {
            return Ok(Self::default());
        }
        let mut names = Vec::new();
        for entry in fs::read_dir(&folder)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !Path::new(name)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("msh"))
            {
                continue;
            }
            ensure!(
                entry.file_type()?.is_file(),
                "Clothing mesh must be a regular file: {name}"
            );
            names.push(name.to_owned());
        }
        Self::from_filenames(names)
    }
    fn from_filenames(names: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut catalog = Self::default();
        let mut primary_items = BTreeMap::new();
        let mut companions = Vec::new();
        for name in names {
            let Some(item) = ClothingItem::from_filename(&name) else {
                catalog.unclassified_meshes += 1;
                continue;
            };
            if let Some(primary) = item.primary_filename() {
                companions.push((primary, item));
            } else {
                ensure!(
                    primary_items
                        .insert(item.id.to_ascii_lowercase(), item)
                        .is_none(),
                    "Clothing filenames differ only in case: {name}"
                );
            }
        }
        for (filename, companion) in companions {
            let primary = primary_items.entry(filename.to_ascii_lowercase()).or_insert_with(|| {
                let mut item = ClothingItem::from_filename(&filename).expect("Valid primary filename");
                item.unavailable_reason = Some(format!(
                    "Complete mesh {filename} is missing; only the internal companion {} is available.",
                    companion.id
                ));
                item
            });
            ensure!(
                primary.companion_resource.is_none(),
                "Duplicate companion for {}",
                primary.id
            );
            primary.companion_resource = Some(companion.resource);
            primary.search.push(' ');
            primary.search.push_str(&companion.id.to_lowercase());
            catalog
                .companion_items
                .insert(companion.id.to_ascii_lowercase(), primary.id.clone());
        }
        catalog.items = primary_items.into_values().collect();
        catalog.items.sort_by(|a, b| a.id.cmp(&b.id));
        // Different primary resources can share a label (e.g. GLAS / GLASS).
        // Retain those authored variants with distinct names in filename order.
        let mut labels = BTreeMap::<_, (usize, usize)>::new();
        for item in &catalog.items {
            labels
                .entry((item.body, item.slot, item.name.clone()))
                .or_default()
                .0 += 1;
        }
        for item in &mut catalog.items {
            let (count, index) = labels
                .get_mut(&(item.body, item.slot, item.name.clone()))
                .unwrap();
            if *count > 1 {
                *index += 1;
                item.name = format!("{} {index}", item.name);
                item.search
                    .push_str(&format!(" {}", item.name.to_lowercase()));
            }
        }
        Ok(catalog)
    }
    pub fn item(&self, id: &str) -> Result<&ClothingItem> {
        let id = self
            .companion_items
            .get(&id.to_ascii_lowercase())
            .map_or(id, String::as_str);
        self.items
            .binary_search_by(|item| item.id.as_str().cmp(id))
            .ok()
            .map(|i| &self.items[i])
            .with_context(|| format!("Unknown clothing item {id:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clothing_companions_resolve_to_one_complete_wearable() -> Result<()> {
        let mut names = Vec::new();
        for kind in ["COAT", "PANT", "SHOES", "ONE", "DAOJU"] {
            // Companion comes first: discovery must not depend on directory order.
            names.push(format!("000001_MAN_{kind}_.MSH"));
            names.push(format!("000001_MAN_{kind}.MSH"));
        }
        names.extend([
            "000002_WOMAN_SHOES_.MSH".into(), // Primary missing.
            "000003_MAN_HAIR_.MSH".into(),    // Group, but do not use for its thumbnail.
            "000003_MAN_HAIR.MSH".into(),
            "000004_MAN_SHOES__.MSH".into(), // Not generated by the native rule.
            "000005_WOMAN_GLAS.MSH".into(),  // Separate primary variants remain available.
            "000005_WOMAN_GLASS.MSH".into(),
        ]);
        let catalog = WardrobeCatalog::from_filenames(names)?;
        assert_eq!(catalog.items.len(), 9);
        assert_eq!(catalog.unclassified_meshes, 1);
        for kind in ["COAT", "PANT", "SHOES", "ONE", "DAOJU"] {
            let primary = format!("000001_MAN_{kind}.MSH");
            let companion = format!("000001_MAN_{kind}_.MSH");
            let item = catalog.item(&companion)?;
            assert!(std::ptr::eq(item, catalog.item(&primary)?));
            assert_eq!(item.resource, format!("character/skin/{primary}"));
            assert_eq!(
                item.companion_resource.as_deref(),
                Some(format!("character/skin/{companion}").as_str())
            );
            assert!(!item.name.contains("variant"));
            assert_eq!(
                item.preview_resource(),
                format!("character/skin/{companion}")
            );
            item.ensure_available()?;
        }
        let orphan = catalog.item("000002_WOMAN_SHOES_.MSH")?;
        assert_eq!(orphan.id, "000002_WOMAN_SHOES.MSH");
        assert!(
            orphan
                .ensure_available()
                .unwrap_err()
                .to_string()
                .contains("Complete mesh")
        );
        assert_eq!(
            catalog.item("000003_MAN_HAIR_.MSH")?.id,
            "000003_MAN_HAIR.MSH"
        );
        assert_eq!(
            catalog.item("000003_MAN_HAIR_.MSH")?.preview_resource(),
            "character/skin/000003_MAN_HAIR.MSH"
        );
        assert!(catalog.item("../000001_MAN_SHOES_.MSH").is_err());
        assert_eq!(
            catalog.item("000005_WOMAN_GLAS.MSH")?.name,
            "Glasses 000005 1"
        );
        assert_eq!(
            catalog.item("000005_WOMAN_GLASS.MSH")?.name,
            "Glasses 000005 2"
        );
        Ok(())
    }
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;
    #[test]
    fn catalog_associations_keep_identical_items_with_their_compatible_presets() -> Result<()> {
        let item = ClothingItem::from_filename("000001_MAN_HAIR.MSH").unwrap();
        let mut catalog = WardrobeCatalog::from_items(vec![item])?;
        catalog.restrict_to_presets("000001_MAN_HAIR.MSH", vec!["first".into()])?;
        let item = catalog.item("000001_MAN_HAIR.MSH")?;
        assert!(catalog.is_compatible("first", Some(CharacterBody::Male), item));
        assert!(!catalog.is_compatible("second", Some(CharacterBody::Male), item));
        assert!(!catalog.is_compatible("first", Some(CharacterBody::Female), item));
        Ok(())
    }
}
