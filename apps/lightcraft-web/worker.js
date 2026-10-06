// LightCraft render worker: a second instance of the app's wasm module that renders jobs posted
// by the page (see apps/lightcraft-web/src/workers.rs). The first message carries the compiled
// WebAssembly.Module and the storage backend kind; the module then takes over `onmessage`.
import init, { worker_main } from "./lightcraft_web.js";

self.onmessage = async (e) => {
  self.onmessage = null;
  try {
    await init({ module_or_path: e.data.module });
    worker_main(e.data.store);
  } catch (err) {
    // surfaces as the Worker's `error` event on the page, which falls back to inline rendering
    setTimeout(() => { throw err; });
  }
};
