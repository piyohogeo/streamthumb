import init, {
  planThumbnailPng,
  streamthumbVersion,
  thumbnailPng,
  thumbnailPngToChunks,
  wasmMemoryBytes,
} from "./vendor/streamthumb_wasm.js";

try {
  await init();
  self.postMessage({ type: "ready", version: streamthumbVersion() });
} catch (error) {
  self.postMessage({ type: "failure", requestId: 0, stage: "initialization", error: String(error) });
}

function joinChunks(chunks, length) {
  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.length;
  }
  if (offset !== length) throw new Error(`Chunk length mismatch: expected ${length}, received ${offset}.`);
  return bytes;
}

self.addEventListener("message", ({ data }) => {
  const requestId = Number.isSafeInteger(data.requestId) ? data.requestId : 0;
  try {
    const input = new Uint8Array(data.input);
    if (data.type === "inspect") {
      const plan = planThumbnailPng(input, null, "chunks");
      self.postMessage({ type: "inspected", requestId, input: plan.input });
      return;
    }
    if (data.type !== "run") return;

    const delivery = data.options.output === "rgba" ? "buffered" : "chunks";
    const plan = planThumbnailPng(input, data.options, delivery);
    const wasmAfterPlanBytes = wasmMemoryBytes();
    self.postMessage({ type: "planned", requestId, plan, wasmAfterPlanBytes });
    if (!plan.withinMemoryLimit) {
      self.postMessage({
        type: "failure",
        requestId,
        stage: "configured limit rejection",
        plan,
        required: plan.memory.totalBytes,
        limit: plan.configuredMaxMemoryBytes,
      });
      return;
    }

    const before = wasmMemoryBytes();
    const started = performance.now();
    let outputBytes;
    let metadata;
    if (data.options.output === "rgba") {
      const result = thumbnailPng(input, data.options);
      try {
        outputBytes = result.bytes;
        metadata = {
          width: result.width,
          height: result.height,
          mimeType: result.mimeType,
          format: result.format,
          bytesWritten: outputBytes.length,
        };
      } finally {
        result.free();
      }
    } else {
      const chunks = [];
      const result = thumbnailPngToChunks(input, (chunk) => chunks.push(chunk), data.options);
      try {
        metadata = {
          width: result.width,
          height: result.height,
          mimeType: result.mimeType,
          format: result.format,
          bytesWritten: result.bytesWritten,
          chunkCount: result.chunkCount,
        };
      } finally {
        result.free();
      }
      outputBytes = joinChunks(chunks, metadata.bytesWritten);
    }
    const processingMs = performance.now() - started;
    const after = wasmMemoryBytes();
    self.postMessage(
      {
        type: "success",
        requestId,
        metadata,
        bytes: outputBytes.buffer,
        timings: { processingMs },
        wasm: { before, after, growth: Math.max(0, after - before) },
      },
      [outputBytes.buffer],
    );
  } catch (error) {
    self.postMessage({ type: "failure", requestId, stage: data.type === "inspect" ? "input inspection / planning" : "decode / encode / allocation", error: String(error) });
  }
});
