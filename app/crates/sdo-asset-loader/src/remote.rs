//! Verify existing collection files first. Downloaded objects and the last accepted
//! manifest are private to the endpoint; only validated game files enter a collection.
use anyhow::{Context, Result, bail, ensure};
use reqwest::{Url, blocking::Client};
use sdo_assets::{
    curated::resolve_resource,
    distribution::{self, AssetFile, Manifest},
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tempfile::NamedTempFile;

pub(crate) struct RemoteCache {
    pub manifest: Manifest,
    endpoint: Url,
    client: Client,
    dir: PathBuf,
    pub local: Option<PathBuf>,
    libraries: Vec<PathBuf>,
    defaults: PathBuf,
    locks: Mutex<BTreeMap<String, Weak<Mutex<()>>>>,
    pub downloads: AtomicUsize,
}
fn matches(path: &Path, file: &AssetFile) -> Result<bool> {
    matches_file(&mut fs::File::open(path)?, file)
}
fn matches_file(input: &mut fs::File, file: &AssetFile) -> Result<bool> {
    if input.metadata()?.len() != file.bytes {
        return Ok(false);
    }
    let mut hash = Sha256::new();
    let n = io::copy(&mut input.take(file.bytes + 1), &mut hash)?;
    Ok(n == file.bytes && format!("{:x}", hash.finalize()) == file.sha256)
}
fn write_atomic(path: &Path, write: impl FnOnce(&mut fs::File) -> Result<()>) -> Result<()> {
    let parent = path.parent().context("Missing destination directory")?;
    fs::create_dir_all(parent)?;
    let mut tmp = NamedTempFile::new_in(parent)?;
    write(tmp.as_file_mut())?;
    tmp.as_file_mut().flush()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}
// Never follow directory symlinks when materializing a remote logical name.
fn destination(root: &Path, file: &AssetFile) -> Result<PathBuf> {
    let mut path = root.to_owned();
    for part in file.path.split('/') {
        path.push(part);
        if let Ok(meta) = fs::symlink_metadata(&path) {
            ensure!(
                !meta.is_symlink(),
                "Symlink in asset destination {}",
                path.display()
            );
        }
    }
    Ok(path)
}
fn install(root: &Path, file: &AssetFile, from: &Path) -> Result<PathBuf> {
    let requested = destination(root, file)?;
    let target = match resolve_resource(root, &file.path) {
        Ok(existing) => {
            let relative = existing
                .strip_prefix(root.canonicalize()?)?
                .to_str()
                .context("Non UTF-8 destination")?;
            ensure!(
                distribution::key(relative) == distribution::key(&file.path),
                "Aliased asset destination"
            );
            existing
        }
        Err(_) => requested,
    };
    if target == from {
        return Ok(target);
    }
    write_atomic(&target, |out| {
        io::copy(&mut fs::File::open(from)?.take(file.bytes + 1), out)?;
        out.rewind()?;
        ensure!(
            matches_file(out, file)?,
            "Asset changed during copy: {}",
            file.path
        );
        Ok(())
    })?;
    Ok(target)
}

impl RemoteCache {
    pub fn open(url: &str, folder: &str, refresh: bool) -> Result<Option<(Arc<Self>, PathBuf)>> {
        if let Some(base) = std::env::var_os("SDO_ASSET_CACHE_DIR") {
            return Self::in_directory(url, folder, Path::new(&base), refresh);
        }
        let dirs = directories::ProjectDirs::from("net", "Arrowgene", "SDO Arrowgene")
            .context("Cannot locate user cache directory")?;
        Self::in_directory(url, folder, dirs.cache_dir(), refresh)
    }
    fn in_directory(
        url: &str,
        folder: &str,
        base: &Path,
        refresh: bool,
    ) -> Result<Option<(Arc<Self>, PathBuf)>> {
        let mut endpoint = Url::parse(url.trim()).context("Invalid asset server URL")?;
        ensure!(
            matches!(endpoint.scheme(), "http" | "https")
                && endpoint.host().is_some()
                && endpoint.query().is_none()
                && endpoint.fragment().is_none()
                && endpoint.username().is_empty()
                && endpoint.password().is_none(),
            "Use an HTTP(S) asset URL without credentials, query, or fragment"
        );
        if !endpoint.path().ends_with('/') {
            endpoint.path_segments_mut().unwrap().push("");
        }
        let dir = base.join(format!("{:x}", Sha256::digest(endpoint.as_str())));
        let saved = dir.join("manifest.json");
        if !refresh && !saved.is_file() {
            return Ok(None);
        }
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(90))
            .build()?;
        let mut raw = Vec::new();
        if refresh {
            client
                .get(endpoint.join("manifest.json")?)
                .send()?
                .error_for_status()?
                .take(distribution::MAX_MANIFEST + 1)
                .read_to_end(&mut raw)?;
        } else {
            fs::File::open(&saved)?
                .take(distribution::MAX_MANIFEST + 1)
                .read_to_end(&mut raw)?;
        }
        ensure!(
            raw.len() as u64 <= distribution::MAX_MANIFEST,
            "Asset manifest too large"
        );
        let manifest: Manifest = serde_json::from_slice(&raw)?;
        manifest.validate()?;
        let local = if folder.trim().is_empty() {
            None
        } else {
            let root = Path::new(folder);
            fs::create_dir_all(root).context("Cannot open selected asset directory")?;
            Some(root.canonicalize()?)
        };
        let mut libraries: Vec<_> = local.iter().cloned().collect();
        if let Some(root) = &local {
            let games = root.join("games");
            if games.is_dir() && !games.is_symlink() {
                for entry in fs::read_dir(games)? {
                    let entry = entry?;
                    if entry.file_type()?.is_dir() {
                        libraries.push(entry.path());
                    }
                }
            }
        }
        let view = dir
            .join("defaults")
            .join(format!("{:x}", Sha256::digest(&raw)));
        libraries.sort();

        let cache = Arc::new(Self {
            manifest,
            endpoint,
            client,
            dir,
            local,
            libraries,
            defaults: view.clone(),
            locks: Default::default(),
            downloads: AtomicUsize::new(0),
        });
        let mut local_defaults = cache.local.is_some();
        for (key, file) in &cache.manifest.files {
            if key == "music.json" || key.starts_with("ui/") {
                let verified = cache.resolve(key, refresh, &mut |_| true)?;
                if !cache.local.as_ref().is_some_and(|root| {
                    verified.starts_with(root)
                        && resolve_resource(root, &file.path).ok().as_ref() == Some(&verified)
                }) {
                    local_defaults = false
                }
                let target = destination(&view, file)?;
                if !matches(&target, file).unwrap_or(false) {
                    install(&view, file, &verified)?;
                }
            }
        }
        // Validate the catalog and default UI before activating a refreshed index.
        cache.catalog(&view)?;
        let defaults = sdo_assets::UiAssets::distribution_defaults(&view.join("ui"))?;
        ensure!(
            defaults.issues.is_empty(),
            "Invalid default UI: {:?}",
            defaults.issues
        );
        if refresh {
            write_atomic(&saved, |f| Ok(f.write_all(&raw)?))?;
        }
        let root = if local_defaults {
            cache.local.clone().unwrap()
        } else {
            view
        };
        Ok(Some((cache, root)))
    }
    pub fn local_libraries(&self) -> impl Iterator<Item = &Path> {
        self.libraries.iter().map(PathBuf::as_path)
    }
    pub fn catalog(&self, root: &Path) -> Result<sdo_song::Catalog> {
        let mut catalog =
            sdo_song::Catalog::parse(&fs::read(resolve_resource(root, "music.json")?)?)?;
        let listed = |path: &str| self.manifest.files.contains_key(&distribution::key(path));
        catalog.songs.retain_mut(|song| {
            if !listed(&song.audio) {
                return false;
            }
            song.preview = song.preview.take().filter(|p| listed(&p.path));
            song.dance_moves_paths = song.dance_moves_paths.take().filter(|p| listed(p));
            song.patterns.retain_mut(|p| {
                let key = distribution::key(&p.file);
                let Some(levels) = self.manifest.charts.get(&key) else {
                    return false;
                };
                let Some(file) = self.manifest.files.get(&key) else {
                    return false;
                };
                p.levels = *levels;
                p.sha256 = file.sha256.clone();
                p.mode == 4
                    && ["k", "t"].iter().any(|k| p.kind.eq_ignore_ascii_case(k))
                    && levels.iter().any(Option::is_some)
            });
            !song.patterns.is_empty()
        });
        Ok(catalog)
    }
    pub fn resolve(
        &self,
        id: &str,
        fetch: bool,
        progress: &mut dyn FnMut(f32) -> bool,
    ) -> Result<PathBuf> {
        let file = self
            .manifest
            .files
            .get(&distribution::key(id))
            .with_context(|| format!("Asset is not published: {id}"))?;
        let lock = {
            let mut locks = self.locks.lock().unwrap();
            let lock = locks
                .get(&file.sha256)
                .and_then(Weak::upgrade)
                .unwrap_or_default();
            locks.insert(file.sha256.clone(), Arc::downgrade(&lock));
            lock
        };
        let _guard = lock.lock().unwrap();
        let mut verified = None;
        for root in self.libraries.iter().chain(std::iter::once(&self.defaults)) {
            if let Ok(path) = resolve_resource(root, &file.path)
                && matches(&path, file).unwrap_or(false)
            {
                verified = Some(path);
                break;
            }
        }
        let object = self.dir.join("objects").join(&file.sha256);
        let verified = match verified {
            Some(path) => path,
            None => {
                if !matches(&object, file).unwrap_or(false) {
                    ensure!(
                        fetch,
                        "Use Initial load to repair missing or changed asset: {id}"
                    );
                    ensure!(progress(0.), "Asset request cancelled");
                    let mut response = self
                        .client
                        .get(self.endpoint.join(&format!("files/{}", file.sha256))?)
                        .send()?
                        .error_for_status()?;
                    write_atomic(&object, |out| {
                        let mut hash = Sha256::new();
                        let mut n = 0u64;
                        let mut buf = [0u8; 64 * 1024];
                        loop {
                            let count = response.read(&mut buf)?;
                            if count == 0 {
                                break;
                            }
                            n += count as u64;
                            ensure!(n <= file.bytes, "Oversized download for {id}");
                            ensure!(
                                progress(n as f32 / file.bytes.max(1) as f32),
                                "Asset request cancelled"
                            );
                            hash.update(&buf[..count]);
                            out.write_all(&buf[..count])?;
                        }
                        ensure!(
                            n == file.bytes && format!("{:x}", hash.finalize()) == file.sha256,
                            "Asset verification failed for {id}"
                        );
                        Ok(())
                    })?;
                    self.downloads.fetch_add(1, Ordering::Relaxed);
                }
                object
            }
        };
        if let Some(root) = &self.local {
            if let Ok(existing) = resolve_resource(root, &file.path)
                && existing == verified
            {
                return Ok(verified);
            }
            match install(root, file, &verified) {
                Ok(path) => return Ok(path),
                Err(e)
                    if e.downcast_ref::<io::Error>().is_some_and(|e| {
                        matches!(
                            e.kind(),
                            io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
                        )
                    }) => {}
                Err(e) => bail!("Cannot update {}: {e:#}", file.path),
            }
        }
        Ok(verified)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn verified_download_is_atomic_and_shared_by_concurrent_readers() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let local = tmp.path().canonicalize()?.join("library");
        let path = local.join("music/test.ogg");
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(&path, b"OLD")?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let endpoint = Url::parse(&format!("http://{}/", listener.local_addr()?))?;
        // First response has the right size but wrong digest. The retry succeeds.
        let server = thread::spawn(move || {
            for body in ["BAD", "new"] {
                let (mut socket, _) = listener.accept().unwrap();
                let mut buf = [0; 4096];
                let _ = socket.read(&mut buf).unwrap();
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\n{body}"
                )
                .unwrap();
            }
        });
        let file = AssetFile {
            path: "music/test.ogg".into(),
            bytes: 3,
            sha256: format!("{:x}", Sha256::digest(b"new")),
        };
        let cache = RemoteCache {
            manifest: Manifest {
                version: 1,
                files: BTreeMap::from([(file.path.clone(), file)]),
                charts: Default::default(),
                dance_motions: Default::default(),
            },
            endpoint,
            client: Client::new(),
            dir: tmp.path().join("cache"),
            local: Some(local.clone()),
            libraries: vec![local.clone()],
            defaults: tmp.path().join("defaults"),
            locks: Default::default(),
            downloads: AtomicUsize::new(0),
        };
        let resolve = || cache.resolve("music/test.ogg", true, &mut |_| true);
        assert!(resolve().is_err());
        assert_eq!(fs::read(&path)?, b"OLD");
        thread::scope(|s| -> Result<()> {
            let a = s.spawn(resolve);
            let b = s.spawn(resolve);
            assert_eq!(a.join().unwrap()?, path);
            assert_eq!(b.join().unwrap()?, path);
            Ok(())
        })?;
        server.join().unwrap();
        fs::write(&path, b"old")?;
        assert_eq!(fs::read(resolve()?)?, b"new");
        assert_eq!(cache.downloads.load(Ordering::Relaxed), 1);
        assert!(
            cache
                .resolve("ui/note/skin/other/skin.png", true, &mut |_| true)
                .is_err()
        );
        Ok(())
    }
}
