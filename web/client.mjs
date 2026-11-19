import init, {enable_audio, start_client} from "./pkg/sdo_arrowgene.js";

const form = document.getElementById("source");
const endpoint = document.getElementById("endpoint");
const status = document.getElementById("status");
const canvas = document.getElementById("game");

function decode(blob) {
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL("./decode.mjs", import.meta.url), {type: "module"});
    worker.onmessage = ({data}) => {
      worker.terminate();
      if (data.error) reject(new Error(data.error));
      else resolve(data);
    };
    worker.onerror = event => { worker.terminate(); reject(new Error(event.message)); };
    worker.postMessage(blob);
  });
}

async function launch() {
  const button = form.querySelector("button");
  if (button.disabled) return;
  button.disabled = true;
  status.textContent = "Loading game and collection…";
  try {
    await init();
    globalThis.sdoDecodeRecording = decode;
    const socket = new URL("./ws", location.href);
    socket.protocol = socket.protocol === "https:" ? "wss:" : "ws:";
    await start_client(endpoint.value, socket.href);
    const enable = () => enable_audio();
    canvas.addEventListener("pointerdown", enable);
    canvas.addEventListener("keydown", enable);
    canvas.focus();
    document.getElementById("launcher").hidden = true;
  } catch (error) {
    canvas.hidden = true;
    status.textContent = String(error);
  } finally { button.disabled = false; }
}

form.onsubmit = event => { event.preventDefault(); launch(); };
try {
  const config = await (await fetch("./config.json")).json();
  if (config.assets && !new URLSearchParams(location.search).has("choose")) {
    endpoint.value = new URL("./assets/", location.href).href;
    await launch();
  }
} catch (error) { status.textContent = String(error); }
