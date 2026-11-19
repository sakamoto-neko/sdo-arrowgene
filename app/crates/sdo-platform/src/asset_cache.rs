//! Browser transport and persistent storage for verified content-addressed objects.
use crate::{assets, remote::AssetFile};
use anyhow::{Context, Result, ensure};
use js_sys::{Array, Promise, Uint8Array};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use url::Url;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::{JsFuture, future_to_promise};
use web_sys::{
    Blob, Cache, IdbDatabase, IdbTransactionMode, ReadableStreamDefaultReader,
    ReadableStreamReadResult, RequestCache, RequestCredentials, RequestInit, Response,
};

const DATABASE: &str = "sdo-assets-v1";
const STORE: &str = "files";

fn js(value: impl Into<JsValue>) -> anyhow::Error {
    let value = value.into();
    anyhow::anyhow!("Browser asset cache: {value:?}")
}
async fn wait(promise: Promise) -> Result<JsValue> {
    JsFuture::from(promise).await.map_err(js)
}

enum Storage {
    Cache(Cache),
    IndexedDb(IdbDatabase),
}
impl Storage {
    async fn open() -> Result<Self> {
        let window = web_sys::window().context("Missing browser window")?;
        if window.is_secure_context()
            && let Ok(storage) = window.caches()
            && !JsValue::from(storage.clone()).is_undefined()
        {
            return Ok(Self::Cache(
                wait(storage.open(DATABASE)).await?.dyn_into().map_err(js)?,
            ));
        }
        let factory = window
            .indexed_db()
            .map_err(js)?
            .context("IndexedDB unavailable")?;
        let open = factory.open_with_u32(DATABASE, 1).map_err(js)?;
        let request = open.clone();
        let upgrade =
            Closure::<dyn FnMut() -> Result<(), JsValue>>::new(move || -> Result<(), JsValue> {
                request
                    .result()?
                    .dyn_into::<IdbDatabase>()?
                    .create_object_store(STORE)?;
                Ok(())
            });
        open.set_onupgradeneeded(Some(upgrade.as_ref().unchecked_ref()));
        let done = Promise::new(&mut |resolve, reject| {
            open.set_onsuccess(Some(&resolve));
            open.set_onerror(Some(&reject));
        });
        let result = wait(done).await;
        open.set_onupgradeneeded(None);
        result?;
        Ok(Self::IndexedDb(
            open.result().map_err(js)?.dyn_into().map_err(js)?,
        ))
    }

    async fn read(&self, url: &str) -> Result<Option<Blob>> {
        let value = match self {
            Self::Cache(cache) => {
                let response = wait(cache.match_with_str(url)).await?;
                if response.is_undefined() {
                    return Ok(None);
                }
                wait(
                    response
                        .dyn_into::<Response>()
                        .map_err(js)?
                        .blob()
                        .map_err(js)?,
                )
                .await?
            }
            Self::IndexedDb(db) => {
                let tx = db.transaction_with_str(STORE).map_err(js)?;
                let request = tx
                    .object_store(STORE)
                    .map_err(js)?
                    .get(&url.into())
                    .map_err(js)?;
                wait(Promise::new(&mut |resolve, reject| {
                    request.set_onsuccess(Some(&resolve));
                    request.set_onerror(Some(&reject));
                }))
                .await?;
                request.result().map_err(js)?
            }
        };
        Ok(value.dyn_into::<Blob>().ok())
    }
    async fn write(&self, url: &str, blob: &Blob) -> Result<()> {
        match self {
            Self::Cache(cache) => {
                let response = Response::new_with_opt_blob(Some(blob)).map_err(js)?;
                wait(cache.put_with_str(url, &response)).await?;
            }
            Self::IndexedDb(db) => {
                let tx = db
                    .transaction_with_str_and_mode(STORE, IdbTransactionMode::Readwrite)
                    .map_err(js)?;
                tx.object_store(STORE)
                    .map_err(js)?
                    .put_with_key(blob, &url.into())
                    .map_err(js)?;
                wait(Promise::new(&mut |resolve, reject| {
                    tx.set_oncomplete(Some(&resolve));
                    tx.set_onabort(Some(&reject));
                }))
                .await?;
            }
        }
        Ok(())
    }
}
impl Drop for Storage {
    fn drop(&mut self) {
        if let Self::IndexedDb(db) = self {
            db.close();
        }
    }
}

async fn request(url: &Url, cache: RequestCache) -> Result<Response> {
    let init = RequestInit::new();
    init.set_cache(cache);
    init.set_credentials(RequestCredentials::Omit);
    wait(
        web_sys::window()
            .context("Missing browser window")?
            .fetch_with_str_and_init(url.as_str(), &init),
    )
    .await?
    .dyn_into()
    .map_err(js)
}

// Read chunk by chunk, including responses with no Content-Length.
async fn bounded(response: Response, limit: u64) -> Result<Blob> {
    ensure!(
        response.ok(),
        "Asset request failed (HTTP {})",
        response.status()
    );
    if let Some(size) = response
        .headers()
        .get("Content-Length")
        .map_err(js)?
        .and_then(|s| s.parse::<u64>().ok())
    {
        ensure!(size <= limit, "Asset exceeds declared size");
    }
    let reader: ReadableStreamDefaultReader = response
        .body()
        .context("Missing asset body")?
        .get_reader()
        .dyn_into()
        .map_err(js)?;
    let chunks = Array::new();
    let mut size = 0u64;
    loop {
        let result: ReadableStreamReadResult = wait(reader.read()).await?.unchecked_into();
        if result.get_done() == Some(true) {
            break;
        }
        let chunk: Uint8Array = result.get_value().dyn_into().map_err(js)?;
        size += u64::from(chunk.length());
        if size > limit {
            let _ = wait(reader.cancel()).await;
            anyhow::bail!("Asset exceeds declared size");
        }
        chunks.push(&chunk);
    }
    Blob::new_with_u8_array_sequence(&chunks).map_err(js)
}
pub async fn index(url: &Url, limit: u64, optional: bool) -> Result<Option<Vec<u8>>> {
    let response = request(url, RequestCache::NoCache).await?;
    if optional && response.status() == 404 {
        return Ok(None);
    }
    Ok(Some(bytes(&bounded(response, limit).await?).await?))
}
async fn bytes(blob: &Blob) -> Result<Vec<u8>> {
    Ok(Uint8Array::new(&wait(blob.array_buffer()).await?).to_vec())
}
async fn matches(blob: &Blob, file: &AssetFile) -> Result<bool> {
    if blob.size() != file.bytes as f64 {
        return Ok(false);
    }
    let mut hash = Sha256::new();
    hash.update(bytes(blob).await?);
    Ok(file.matches(file.bytes, hash))
}

pub struct AssetCache {
    endpoint: Url,
    files: BTreeMap<String, AssetFile>,
    storage: Storage,
    pending: RefCell<BTreeMap<String, Promise>>,
}
thread_local! {static ACTIVE:RefCell<Option<Rc<AssetCache>>>=const {RefCell::new(None)};}
impl AssetCache {
    pub async fn open(endpoint: Url, files: BTreeMap<String, AssetFile>) -> Result<Rc<Self>> {
        Ok(Rc::new(Self {
            endpoint,
            files,
            storage: Storage::open().await?,
            pending: Default::default(),
        }))
    }
    pub async fn activate(self: Rc<Self>, initial: impl IntoIterator<Item = String>) -> Result<()> {
        let mut mounted = Vec::new();
        for id in initial {
            let file = self.files.get(&id).context("Unknown initial asset")?;
            mounted.push((&file.path, bytes(&self.object(file).await?).await?));
        }
        assets::clear();
        for file in self.files.values() {
            assets::declare(&file.path)?;
        }
        for (path, bytes) in mounted {
            assets::mount(path, bytes)?;
        }
        ACTIVE.with_borrow_mut(|slot| *slot = Some(self));
        Ok(())
    }
    async fn load(&self, file: &AssetFile) -> Result<Blob> {
        let url = file.url(&self.endpoint)?;
        if let Some(blob) = self.storage.read(url.as_str()).await?
            && matches(&blob, file).await?
        {
            return Ok(blob);
        }
        let blob = bounded(request(&url, RequestCache::NoStore).await?, file.bytes).await?;
        ensure!(
            matches(&blob, file).await?,
            "Asset verification failed for {}",
            file.path
        );
        self.storage.write(url.as_str(), &blob).await?;
        Ok(blob)
    }
    async fn object(self: &Rc<Self>, file: &AssetFile) -> Result<Blob> {
        let promise = self
            .pending
            .borrow_mut()
            .entry(file.sha256.clone())
            .or_insert_with(|| {
                let cache = self.clone();
                let file = file.clone();
                future_to_promise(async move {
                    let result = cache.load(&file).await;
                    cache.pending.borrow_mut().remove(&file.sha256);
                    result
                        .map(JsValue::from)
                        .map_err(|err| JsValue::from_str(&format!("{err:#}")))
                })
            })
            .clone();
        wait(promise).await?.dyn_into().map_err(js)
    }
}
pub async fn blob(name: &str) -> Result<Blob> {
    let cache = ACTIVE
        .with_borrow(Clone::clone)
        .context("Asset collection is not open")?;
    let file = cache
        .files
        .get(&name.replace('\\', "/").to_ascii_lowercase())
        .with_context(|| format!("Asset is unavailable: {name}"))?;
    cache.object(file).await
}
pub(super) async fn read(name: &str) -> Result<Vec<u8>> {
    bytes(&blob(name).await?).await
}
