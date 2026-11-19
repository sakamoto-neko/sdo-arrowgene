// Storage adapter for a browser client. Local files stay on the user's device.
// Call initialLoad from Settings, preview on selection, and prepareSong on Play/Ready.
const defaults = [
  "ui/note/skin/classic_1/note_skin.json", "ui/note/board/note_board.json",
  "ui/note/effect/sparkle/note_effect.json", "ui/score/score.json", "ui/health/health.json",
];
const key = path => path.replaceAll("\\", "/").replace(/[A-Z]/g, c => c.toLowerCase());
const maxFile = 256 * 1024 * 1024;
const maxManifest = 16 * 1024 * 1024;

function essential(path) {
  if (path === "music.json" || defaults.includes(path)) return true;
  const dir = path.slice(0, path.lastIndexOf("/") + 1);
  return /\.(png|bmp|dds|tga)$/.test(path) && defaults.some(def => def.slice(0, def.lastIndexOf("/") + 1) === dir);
}

async function readBounded(response, limit) {
  if (!response.ok) throw new Error(`Asset request failed (HTTP ${response.status})`);
  if (Number(response.headers.get("Content-Length")) > limit) throw new Error("Asset exceeds declared size");
  const reader = response.body.getReader();
  const chunks = [];
  let size = 0;
  while (true) {
    const {done, value} = await reader.read();
    if (done) break;
    size += value.length;
    if (size > limit) {
      await reader.cancel();
      throw new Error("Asset exceeds declared size");
    }
    chunks.push(value);
  }
  return new Blob(chunks, {type: response.headers.get("Content-Type") || ""});
}

async function matches(blob, entry) {
  if (blob.size !== entry.bytes) return false;
  const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", await blob.arrayBuffer()));
  return Array.from(hash, byte => byte.toString(16).padStart(2, "0")).join("") === entry.sha256;
}

export class RemoteAssets {
  #local = new Map();
  #pending = new Map();
  #endpoint;
  #index;
  #cache;
  constructor(endpoint, localFiles = []) {
    this.#endpoint = new URL(endpoint);
    if (!["https:", "http:"].includes(this.#endpoint.protocol) || this.#endpoint.search ||
        this.#endpoint.hash || this.#endpoint.username || this.#endpoint.password) {
      throw new Error("Expected an HTTP(S) asset endpoint");
    }
    if (!this.#endpoint.pathname.endsWith("/")) this.#endpoint.pathname += "/";
    for (const file of localFiles) {
      const path = key(file.webkitRelativePath).split("/").slice(1).join("/");
      const logical = path.replace(/^games\/[^/]+\//, "");
      if (!this.#local.has(logical)) this.#local.set(logical, []);
      this.#local.get(logical).push(file);
    }
  }

  async initialLoad() {
    const url = new URL("manifest.json", this.#endpoint);
    const cache = await caches.open("sdo-assets-v1");
    const blob = await readBounded(await fetch(url, {cache:"no-cache", credentials:"omit"}), maxManifest);
    const catalog = await this.#activate(JSON.parse(await blob.text()), cache);
    await cache.put(url, new Response(blob));
    return catalog;
  }

  async openCached() {
    const cache = await caches.open("sdo-assets-v1");
    const saved = await cache.match(new URL("manifest.json", this.#endpoint));
    if (!saved) return null;
    return this.#activate(await saved.json(), cache, false);
  }

  async #activate(index, cache, fetchMissing = true) {
    if (index.version !== 1 || !index.files || !index.charts || !index.dance_motions) throw new Error("Unsupported asset manifest");
    for (const id of ["music.json", ...defaults]) {
      if (!Object.hasOwn(index.files, id)) throw new Error(`Missing essential asset: ${id}`);
    }
    for (const [id, entry] of Object.entries(index.files)) {
      if (id !== key(entry.path) || id.split("/").some(part => !part || part.startsWith(".")) ||
          !Number.isSafeInteger(entry.bytes) || entry.bytes < 0 || entry.bytes > maxFile ||
          !/^[0-9a-f]{64}$/.test(entry.sha256) ||
          !(essential(id) || /^music\/.+\.(ogg|gn|dps)$/.test(id) || /^character\/motion\/[^/]+\.mot$/.test(id))) {
        throw new Error(`Invalid manifest entry: ${id}`);
      }
    }
    for (const id of Object.keys(index.charts)) {
      if (!id.startsWith("music/") || !id.endsWith(".gn") || !Object.hasOwn(index.files, id)) throw new Error(`Unknown chart: ${id}`);
    }
    for (const [id, motions] of Object.entries(index.dance_motions)) {
      if (!id.startsWith("music/") || !id.endsWith(".dps") || !Object.hasOwn(index.files, id) ||
          !Array.isArray(motions) || motions.some(m => !m.startsWith("character/motion/") || !Object.hasOwn(index.files, m))) {
        throw new Error(`Invalid choreography: ${id}`);
      }
    }
    const readEssential = async entry => {
      const blob = await this.#read(entry, cache, fetchMissing);
      // Directory-picker files are only available for the current browser session.
      if (blob instanceof File) await cache.put(new URL("files/" + entry.sha256, this.#endpoint), new Response(blob));
      return blob;
    };
    const catalog = JSON.parse(await (await readEssential(index.files["music.json"])).text());
    for (const [id, entry] of Object.entries(index.files)) {
      if (id !== "music.json" && essential(id)) await readEssential(entry);
    }
    for (const song of catalog.songs) {
      song.patterns = song.patterns.filter(pattern => pattern.mode === 4 && Object.hasOwn(index.charts, key(pattern.file)));
      for (const pattern of song.patterns) {
        pattern.levels = index.charts[key(pattern.file)];
        pattern.sha256 = index.files[key(pattern.file)].sha256;
      }
      if (song.preview && !Object.hasOwn(index.files, key(song.preview.path))) song.preview = null;
      if (song.dance_moves_paths && !Object.hasOwn(index.dance_motions, key(song.dance_moves_paths))) song.dance_moves_paths = null;
    }
    catalog.songs = catalog.songs.filter(song => song.patterns.length && Object.hasOwn(index.files, key(song.audio)));
    this.#cache = cache;
    this.#index = index;
    return catalog;
  }

  async #read(entry, cache, fetchMissing, signal) {
    signal?.throwIfAborted();
    for (const file of this.#local.get(key(entry.path)) ?? []) {
      if (await matches(file, entry)) return file;
    }
    const url = new URL("files/" + entry.sha256, this.#endpoint);
    const saved = await cache.match(url);
    if (saved) {
      const blob = await saved.blob();
      if (await matches(blob, entry)) return blob;
    }
    if (!fetchMissing) throw new Error(`Initial load required for ${entry.path}`);
    if (!signal && this.#pending.has(url.href)) return this.#pending.get(url.href);
    const download = (async () => {
      const blob = await readBounded(await fetch(url, {cache:"no-store",credentials:"omit",signal}), entry.bytes);
      if (!await matches(blob, entry)) throw new Error(`Hash mismatch for ${entry.path}`);
      await cache.put(url, new Response(blob));
      return blob;
    })();
    if (signal) return download;
    this.#pending.set(url.href, download);
    try { return await download; } finally { this.#pending.delete(url.href); }
  }

  async file(path, {signal} = {}) {
    const entry = this.#index?.files[key(path)];
    if (!entry) throw new Error(`Asset is unavailable: ${path}`);
    return this.#read(entry, this.#cache, true, signal);
  }

  async preview(song, options) { return song.preview ? this.file(song.preview.path, options) : null; }

  async prepareSong(song, pattern, {dancer = false, multiplayer = false} = {}) {
    const files = [song.audio, pattern.file];
    if ((dancer || multiplayer) && song.dance_moves_paths) {
      files.push(song.dance_moves_paths, ...this.#index.dance_motions[key(song.dance_moves_paths)]);
    }
    const loaded = new Map();
    for (const name of new Set(files)) loaded.set(name, await this.file(name));
    return loaded;
  }
}
