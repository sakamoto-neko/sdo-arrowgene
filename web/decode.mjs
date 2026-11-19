import init, {Recording} from "./pkg/sdo_web.js";

self.onmessage = async ({data}) => {
  try {
    await init();
    const recording = Recording.decode(new Uint8Array(await data.arrayBuffer()));
    const rate = recording.rate();
    const channels = recording.channels();
    const samples = recording.samples();
    self.postMessage([rate, channels, samples], [samples.buffer]);
  } catch (error) { self.postMessage({error: String(error)}); }
};
