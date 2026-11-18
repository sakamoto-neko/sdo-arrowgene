use crate::{Texture, TextureId};
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::PathBuf,
};
pub(crate) struct TextureStore {
    pub(crate) root: PathBuf,
    pub(crate) textures: Vec<Texture>,
    pub(crate) cache: HashMap<PathBuf, TextureId>,
    pub(crate) content_cache: HashMap<[u8; 32], TextureId>,
    pub(crate) resources: BTreeSet<PathBuf>,
}
impl TextureStore {
    pub(crate) fn from_root(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root
            .into()
            .canonicalize()
            .context("Cannot open UI asset directory")?;
        ensure!(root.is_dir(), "UI root is not a directory");
        Ok(Self {
            root,
            textures: vec![Texture {
                source: PathBuf::from("<white>"),
                width: 1,
                height: 1,
                rgba: vec![255; 4],
            }],
            cache: HashMap::new(),
            content_cache: HashMap::new(),
            resources: BTreeSet::new(),
        })
    }
    pub(crate) fn image(&mut self, path: PathBuf) -> Result<TextureId> {
        ensure!(
            path.starts_with(&self.root),
            "Image is outside the curated asset directory"
        );
        self.resources.insert(path.clone());
        if let Some(id) = self.cache.get(&path) {
            return Ok(*id);
        }
        let raw = fs::read(&path)?;
        let fingerprint: [u8; 32] = Sha256::digest(&raw).into();
        if let Some(id) = self.content_cache.get(&fingerprint).copied() {
            self.cache.insert(path, id);
            return Ok(id);
        }
        let img = if raw.starts_with(b"DDS ") {
            crate::dds::decode(&raw)
        } else {
            let format = path
                .extension()
                .and_then(|e| e.to_str())
                .filter(|e| e.eq_ignore_ascii_case("tga"))
                .map(|_| image::ImageFormat::Tga);
            match format {
                Some(format) => image::load_from_memory_with_format(&raw, format),
                None => image::load_from_memory(&raw),
            }
            .map(image::DynamicImage::into_rgba8)
            .map_err(Into::into)
        }
        .with_context(|| format!("Cannot decode {}", path.display()))?;
        let (width, height) = img.dimensions();
        ensure!(width > 0 && height > 0, "Empty texture {}", path.display());
        let id = TextureId(self.textures.len());
        self.textures.push(Texture {
            source: path.clone(),
            width,
            height,
            rgba: img.into_raw(),
        });
        self.cache.insert(path, id);
        self.content_cache.insert(fingerprint, id);
        Ok(id)
    }
    pub(crate) fn utility_font(&mut self) -> TextureId {
        use font8x8::UnicodeFonts;
        // Utility text only; score, combo and judgment artwork above are original assets.
        let mut rgba = vec![0; 128 * 64 * 4];
        for code in 0..128u32 {
            if let Some(rows) = font8x8::BASIC_FONTS.get(char::from_u32(code).unwrap()) {
                for (y, row) in rows.into_iter().enumerate() {
                    for x in 0..8usize {
                        if row & (1 << x) != 0 {
                            let pixel = (((code as usize / 16 * 8 + y) * 128)
                                + (code as usize % 16 * 8 + x))
                                * 4;
                            rgba[pixel..pixel + 4].copy_from_slice(&[255; 4]);
                        }
                    }
                }
            }
        }
        let font = TextureId(self.textures.len());
        self.textures.push(Texture {
            source: PathBuf::from("<utility-font>"),
            width: 128,
            height: 64,
            rgba,
        });
        font
    }
}
