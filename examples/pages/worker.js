import init, {
  planThumbnailPngFromSeekable,
  streamthumbVersion,
  thumbnailPngFromSeekable,
  thumbnailPngFromSeekableToChunks,
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

function seekableSource(input) {
  if (!(input instanceof Blob)) {
    throw new TypeError("Pages worker input must be a File or Blob.");
  }
  const reader = new FileReaderSync();
  let calls = 0;
  let bytes = 0;
  let largestReadBytes = 0;
  return {
    inputLength: input.size,
    readAt(offset, length) {
      const chunk = new Uint8Array(
        reader.readAsArrayBuffer(input.slice(offset, offset + length)),
      );
      calls += 1;
      bytes += chunk.length;
      largestReadBytes = Math.max(largestReadBytes, chunk.length);
      return chunk;
    },
    stats() {
      return { calls, bytes, largestReadBytes };
    },
  };
}

self.addEventListener("message", ({ data }) => {
  const requestId = Number.isSafeInteger(data.requestId) ? data.requestId : 0;
  try {
    const source = seekableSource(data.input);
    if (data.type === "inspect") {
      const plan = planThumbnailPngFromSeekable(
        source.inputLength,
        source.readAt,
        null,
        "chunks",
      );
      self.postMessage({ type: "inspected", requestId, input: plan.input });
      return;
    }
    if (data.type !== "run") return;

    const delivery = data.options.output === "rgba" ? "buffered" : "chunks";
    const plan = planThumbnailPngFromSeekable(
      source.inputLength,
      source.readAt,
      data.options,
      delivery,
    );
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
      const result = thumbnailPngFromSeekable(
        source.inputLength,
        source.readAt,
        data.options,
      );
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
      const result = thumbnailPngFromSeekableToChunks(
        source.inputLength,
        source.readAt,
        (chunk) => chunks.push(chunk),
        data.options,
      );
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
        inputReads: source.stats(),
        wasm: { before, after, growth: Math.max(0, after - before) },
      },
      [outputBytes.buffer],
    );
  } catch (error) {
    self.postMessage({ type: "failure", requestId, stage: data.type === "inspect" ? "input inspection / planning" : "decode / encode / allocation", error: String(error) });
  }
});
