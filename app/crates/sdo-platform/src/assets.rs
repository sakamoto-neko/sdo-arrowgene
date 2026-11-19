//! Read-only collection storage. The browser mounts verified files under /assets.
#[cfg(not(target_arch = "wasm32"))]
pub use std::fs::{canonicalize, read, read_dir};

#[cfg(not(target_arch = "wasm32"))]
pub fn is_dir(path: impl AsRef<std::path::Path>) -> bool {
    path.as_ref().is_dir()
}
#[cfg(not(target_arch = "wasm32"))]
pub fn is_file(path: impl AsRef<std::path::Path>) -> bool {
    path.as_ref().is_file()
}
#[cfg(not(target_arch = "wasm32"))]
pub fn exists(path: impl AsRef<std::path::Path>) -> bool {
    path.as_ref().exists()
}

#[cfg(target_arch = "wasm32")]
pub use memory::*;
#[cfg(target_arch = "wasm32")]
mod memory {
    use std::{
        cell::RefCell,
        collections::{BTreeMap, BTreeSet},
        ffi::OsString,
        io,
        path::{Component, Path, PathBuf},
    };

    #[derive(Default)]
    struct Files {
        known: BTreeSet<PathBuf>,
        missing: BTreeSet<PathBuf>,
        data: BTreeMap<PathBuf, Vec<u8>>,
        dirs: BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    }
    thread_local! { static FILES: RefCell<Files> = RefCell::default(); }

    pub fn declare(name: &str) -> io::Result<()> {
        let name = Path::new(name);
        if name.as_os_str().is_empty()
            || name
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(io::Error::other("Invalid mounted asset path"));
        }
        let path = Path::new("/assets").join(name);
        FILES.with_borrow_mut(|files| {
            for child in path.ancestors() {
                if let Some(parent) = child.parent() {
                    files
                        .dirs
                        .entry(parent.into())
                        .or_default()
                        .insert(child.into());
                }
            }
            files.known.insert(path);
        });
        Ok(())
    }
    pub fn mount(name: &str, data: Vec<u8>) -> io::Result<()> {
        declare(name)?;
        FILES.with_borrow_mut(|f| f.data.insert(Path::new("/assets").join(name), data));
        Ok(())
    }
    pub fn is_loaded(path: impl AsRef<Path>) -> bool {
        FILES.with_borrow(|f| f.data.contains_key(path.as_ref()))
    }
    pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
        let path = path.as_ref();
        FILES.with_borrow_mut(|f| {
            if let Some(data) = f.data.get(path) {
                return Ok(data.clone());
            }
            if f.known.contains(path) {
                f.missing.insert(path.into());
            }
            Err(missing(path))
        })
    }
    pub(crate) fn take_missing() -> BTreeSet<PathBuf> {
        FILES.with_borrow_mut(|f| std::mem::take(&mut f.missing))
    }
    pub(crate) fn clear() {
        FILES.with_borrow_mut(|files| *files = Files::default());
    }
    pub async fn fetch(name: &str) -> anyhow::Result<()> {
        if is_loaded(Path::new("/assets").join(name)) {
            return Ok(());
        }
        mount(name, crate::asset_cache::read(name).await?)?;
        Ok(())
    }
    pub async fn hydrate<T>(mut load: impl FnMut() -> T) -> T {
        loop {
            take_missing();
            let result = load();
            let missing = take_missing();
            if missing.is_empty() {
                return result;
            }
            for path in missing {
                let name = path.strip_prefix("/assets").unwrap().to_str().unwrap();
                if fetch(name).await.is_err() {
                    return result;
                }
            }
        }
    }
    fn missing(path: &Path) -> io::Error {
        io::Error::new(io::ErrorKind::NotFound, path.display().to_string())
    }
    pub fn is_dir(path: impl AsRef<Path>) -> bool {
        FILES.with_borrow(|f| f.dirs.contains_key(path.as_ref()))
    }
    pub fn is_file(path: impl AsRef<Path>) -> bool {
        FILES.with_borrow(|f| f.known.contains(path.as_ref()))
    }
    pub fn exists(path: impl AsRef<Path>) -> bool {
        is_dir(&path) || is_file(path)
    }
    pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
        let path = path.as_ref();
        if exists(path) {
            Ok(path.to_owned())
        } else {
            Err(missing(path))
        }
    }
    pub struct DirEntry(PathBuf);
    impl DirEntry {
        pub fn file_name(&self) -> OsString {
            self.0.file_name().unwrap().into()
        }
        pub fn path(&self) -> PathBuf {
            self.0.clone()
        }
        pub fn file_type(&self) -> io::Result<FileType> {
            Ok(FileType(is_dir(&self.0)))
        }
    }
    pub struct FileType(bool);
    impl FileType {
        pub fn is_symlink(&self) -> bool {
            false
        }
        pub fn is_dir(&self) -> bool {
            self.0
        }
        pub fn is_file(&self) -> bool {
            !self.0
        }
    }
    pub fn read_dir(
        path: impl AsRef<Path>,
    ) -> io::Result<impl Iterator<Item = io::Result<DirEntry>>> {
        FILES.with_borrow(|f| {
            Ok(f.dirs
                .get(path.as_ref())
                .ok_or_else(|| missing(path.as_ref()))?
                .clone()
                .into_iter()
                .map(|p| Ok(DirEntry(p))))
        })
    }
}
