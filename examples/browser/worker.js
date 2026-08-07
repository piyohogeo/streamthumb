import init, {
  thumbnailPng,
} from "../../target/npm-package/streamthumb_wasm.js";

self.postMessage({ initializing: true });
try {
  await init();
  self.postMessage({ ready: true });
} catch (error) {
  self.postMessage({ error: `WebAssembly initialization failed: ${error}` });
}

self.addEventListener("message", ({ data }) => {
  try {
    const result = thumbnailPng(new Uint8Array(data), {
      maxWidth: 512,
      maxHeight: 512,
      output: "png",
      maxMemoryBytes: 32 * 1024 * 1024,
    });
    const bytes = result.bytes;
    const width = result.width;
    const height = result.height;
    const mimeType = result.mimeType;
    result.free();
    self.postMessage(
      {
        bytes: bytes.buffer,
        width,
        height,
        mimeType,
      },
      [bytes.buffer],
    );
  } catch (error) {
    self.postMessage({ error: String(error) });
  }
});
