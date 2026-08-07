import init, {
  thumbnailPngToChunks,
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
    const chunks = [];
    const result = thumbnailPngToChunks(
      new Uint8Array(data),
      (chunk) => chunks.push(chunk),
      {
        maxWidth: 512,
        maxHeight: 512,
        output: "png",
        png: { color: "auto", compression: "balanced", filter: "default" },
        maxMemoryBytes: 32 * 1024 * 1024,
      },
    );
    const bytes = new Uint8Array(result.bytesWritten);
    let offset = 0;
    for (const chunk of chunks) {
      bytes.set(chunk, offset);
      offset += chunk.length;
    }
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
