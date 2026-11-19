//! Public, read-only HTTP interface for explicitly indexed assets.
use anyhow::{Context, Result, ensure};
use axum::{
    Router,
    body::Body,
    extract::{Path as RoutePath, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use sdo_assets::{
    UiAssets,
    curated::{CollectionPaths, resolve_resource},
    distribution::{self, AssetFile, Manifest},
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

struct Index {
    root: PathBuf,
    files: BTreeMap<String, AssetFile>,
}
impl Index {
    fn add(&mut self, logical: &str) -> Result<String> {
        let logical = logical.replace('\\', "/");
        let key = distribution::key(&logical);
        if self.files.contains_key(&key) {
            return Ok(key);
        }
        ensure!(
            distribution::permitted(&logical),
            "Unsupported distribution path: {logical}"
        );
        let path = resolve_resource(&self.root, &logical)?;
        // Do not publish an alias that resolves outside its allowed component.
        let relative = path
            .strip_prefix(&self.root)?
            .to_str()
            .context("Non-UTF-8 asset path")?
            .replace('\\', "/");
        ensure!(
            distribution::key(&relative) == key,
            "Aliased asset: {logical}"
        );
        let file = fs::File::open(path)?;
        ensure!(
            file.metadata()?.len() <= distribution::MAX_FILE,
            "Asset too large: {logical}"
        );
        let mut hash = Sha256::new();
        let bytes = io::copy(&mut file.take(distribution::MAX_FILE + 1), &mut hash)?;
        ensure!(
            bytes <= distribution::MAX_FILE,
            "Asset too large: {logical}"
        );
        self.files.insert(
            key.clone(),
            AssetFile {
                path: relative,
                bytes,
                sha256: format!("{:x}", hash.finalize()),
            },
        );
        Ok(key)
    }
}
struct Published {
    manifest: axum::body::Bytes,
    root: PathBuf,
    objects: BTreeMap<String, AssetFile>,
    reads: tokio::sync::Semaphore,
}
impl Published {
    fn index(path: &Path) -> Result<Self> {
        let paths = CollectionPaths::from_assets(path)?;
        let root = paths.assets().to_owned();
        let mut index = Index {
            root: root.clone(),
            files: BTreeMap::new(),
        };
        let defaults = UiAssets::distribution_defaults(&root.join("ui"))?;
        ensure!(
            defaults.issues.is_empty(),
            "Invalid default UI: {:?}",
            defaults.issues
        );
        for resource in defaults.resources {
            index.add(
                resource
                    .strip_prefix(&root)?
                    .to_str()
                    .context("Non-UTF-8 UI path")?,
            )?;
        }
        index.add("music.json")?;
        let catalog = sdo_song::Catalog::load(&paths)?;
        let mut charts = BTreeMap::new();
        let mut dance_motions = BTreeMap::new();
        let mut missing = BTreeSet::new();
        for song in catalog.songs {
            if let Err(e) = index.add(&song.audio) {
                eprintln!("Skipping song {}: {e:#}", song.id);
                continue;
            }
            if let Some(preview) = song.preview
                && let Err(e) = index.add(&preview.path)
            {
                eprintln!("Preview {}: {e:#}", song.id)
            }
            for pattern in song.patterns {
                if pattern.mode == 4
                    && ["k", "t"]
                        .iter()
                        .any(|k| pattern.kind.eq_ignore_ascii_case(k))
                    && pattern.levels.iter().any(Option::is_some)
                {
                    let id = index.add(&pattern.file)?;
                    ensure!(
                        index.files[&id].sha256 == pattern.sha256,
                        "Chart changed while indexing: {id}"
                    );
                    charts.insert(id, pattern.levels);
                }
            }
            if let Some(dps) = song.dance_moves_paths {
                let indexed = (|| -> Result<()> {
                    let raw = fs::read(resolve_resource(&root, &dps)?)?;
                    let script = sdo_dance::DanceScript::parse(&raw)?;
                    let mut motions = BTreeSet::new();
                    for name in script.motion_names() {
                        let logical = format!("character/motion/{name}");
                        if !distribution::permitted(&logical)
                            || name.contains(['/', '\\'])
                            || missing.contains(&logical)
                        {
                            continue;
                        }
                        match index.add(&logical) {
                            Ok(key) => {
                                motions.insert(key);
                            }
                            Err(error) => {
                                eprintln!("Omitting motion {name}: {error:#}");
                                missing.insert(logical);
                            }
                        }
                    }
                    dance_motions.insert(index.add(&dps)?, motions.into_iter().collect());
                    Ok(())
                })();
                if let Err(e) = indexed {
                    eprintln!("Omitting dance {dps}: {e:#}")
                }
            }
        }
        let manifest = Manifest {
            version: distribution::VERSION,
            files: index.files,
            charts,
            dance_motions,
        };
        manifest.validate()?;
        let json = serde_json::to_vec(&manifest)?;
        ensure!(
            json.len() as u64 <= distribution::MAX_MANIFEST,
            "Asset index exceeds manifest limit"
        );
        let objects = manifest
            .files
            .into_values()
            .map(|f| (f.sha256.clone(), f))
            .collect();
        Ok(Self {
            manifest: json.into(),
            root,
            objects,
            reads: tokio::sync::Semaphore::new(8),
        })
    }
}
async fn object(State(state): State<Arc<Published>>, RoutePath(id): RoutePath<String>) -> Response {
    let Some(file) = state.objects.get(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let _permit = state
        .reads
        .acquire()
        .await
        .expect("asset semaphore stays open");
    let path = state.root.join(&file.path);
    let bytes = file.bytes;
    let mime = match path
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "ogg" => "audio/ogg",
        "json" => "application/json",
        "png" => "image/png",
        "bmp" => "image/bmp",
        _ => "application/octet-stream",
    };
    let data = tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
        ensure!(path.canonicalize()? == path, "Asset path changed");
        let mut body = Vec::new();
        fs::File::open(&path)?
            .take(bytes + 1)
            .read_to_end(&mut body)?;
        ensure!(
            body.len() as u64 == bytes && format!("{:x}", Sha256::digest(&body)) == id,
            "Published asset changed"
        );
        Ok(body)
    })
    .await;
    match data {
        Ok(Ok(body)) => (
            [
                (header::CONTENT_TYPE, mime),
                (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            ],
            Body::from(body),
        )
            .into_response(),
        _ => StatusCode::CONFLICT.into_response(),
    }
}

pub fn serve(root: &Path, addr: &str) -> Result<()> {
    let state = Arc::new(Published::index(root)?);
    let listener = std::net::TcpListener::bind(addr)
        .with_context(|| format!("Cannot listen for asset requests at {addr}"))?;
    listener.set_nonblocking(true)?;
    println!(
        "Asset HTTP listening on {} ({} unique files)",
        listener.local_addr()?,
        state.objects.len()
    );
    let router = Router::new()
        .route(
            "/manifest.json",
            get(|State(state): State<Arc<Published>>| async move {
                (
                    [
                        (header::CONTENT_TYPE, "application/json"),
                        (header::CACHE_CONTROL, "no-cache"),
                    ],
                    state.manifest.clone(),
                )
            }),
        )
        .route("/files/{id}", get(object))
        .layer(axum::middleware::map_response(
            |mut res: Response| async move {
                res.headers_mut().insert(
                    header::ACCESS_CONTROL_ALLOW_ORIGIN,
                    HeaderValue::from_static("*"),
                );
                res
            },
        ))
        .with_state(state);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            axum::serve(tokio::net::TcpListener::from_std(listener)?, router).await
        })?;
    Ok(())
}
