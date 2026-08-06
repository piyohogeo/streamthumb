import init, { thumbnailPng } from "../../pkg/streamthumb_wasm.js";

await init();

self.addEventListener("message", ({ data }) => {
  try {
    const result = thumbnailPng(new Uint8Array(data), {
      maxWidth: 512,
      maxHeight: 512,
      output: "png",
      maxMemoryBytes: 32 * 1024 * 1024,
    });
    const bytes = result.bytes;
    self.postMessage(
      {
        bytes: bytes.buffer,
        width: result.width,
        height: result.height,
        mimeType: result.mimeType,
      },
      [bytes.buffer],
    );
  } catch (error) {
    self.postMessage({ error: String(error) });
  }
});

