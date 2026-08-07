import init, {
  streamthumbVersion,
  thumbnailPng,
  thumbnailPngFromSeekable,
  thumbnailPngFromSeekableToChunks,
  thumbnailPngToChunks,
  type SeekableReadAt,
  type ThumbnailChunkCallback,
  type ThumbnailOptions,
  type ThumbnailOutputFormat,
  type PngColorMode,
  type PngCompression,
  type PngFilter,
  type JpegOptions,
  type JpegSubsampling,
} from "@streamthumb/wasm";

const statusElement = document.querySelector<HTMLPreElement>("#status");
if (!statusElement) {
  throw new Error("The smoke-test status element is missing.");
}
const status: HTMLPreElement = statusElement;

async function finish(result: "pass" | "fail", message: string) {
  document.documentElement.dataset.result = result;
  status.textContent = message;
  await fetch("/result", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ result, message }),
  });
}

function hasPngSignature(bytes: Uint8Array) {
  const signature = [137, 80, 78, 71, 13, 10, 26, 10];
  return signature.every((value, index) => bytes[index] === value);
}

function equalBytes(left: Uint8Array, right: Uint8Array) {
  return left.length === right.length
    && left.every((value, index) => value === right[index]);
}

const output: ThumbnailOutputFormat = "png";
const pngColor: PngColorMode = "auto";
const pngCompression: PngCompression = "fast";
const pngFilter: PngFilter = "adaptive";
const options: ThumbnailOptions = {
  maxWidth: 32,
  maxHeight: 32,
  fit: "contain",
  filter: "area",
  allowUpscale: false,
  output,
  png: {
    color: pngColor,
    compression: pngCompression,
    filter: pngFilter,
  },
  maxMemoryBytes: 32 * 1024 * 1024,
};

// These assertions fail compilation if the public literal types become broad.
const jpegSubsampling: JpegSubsampling = "420";
const jpegOptions: JpegOptions = {
  quality: 85,
  background: [255, 255, 255],
  subsampling: jpegSubsampling,
};
const jpegOutput: ThumbnailOptions = { output: "jpeg", jpeg: jpegOptions };
// @ts-expect-error WebP output is not supported.
const invalidOutput: ThumbnailOptions = { output: "webp" };
// @ts-expect-error Width must be numeric.
const invalidWidth: ThumbnailOptions = { maxWidth: "32" };
// @ts-expect-error Indexed PNG output is not supported.
const invalidPngColor: ThumbnailOptions = { png: { color: "indexed8" } };
void invalidOutput;
void invalidWidth;
void invalidPngColor;
void jpegOutput;
void thumbnailPngFromSeekable;
void thumbnailPngFromSeekableToChunks;
const typedReadAt: SeekableReadAt = (_offset, length) => new Uint8Array(length);
void typedReadAt;

function runSeekableWorker(file: Blob): Promise<{
  bytes: Uint8Array;
  chunkBytes: Uint8Array;
  metadata: { width: number; height: number; format: string; chunkCount: number };
}> {
  return new Promise((resolve, reject) => {
    const worker = new Worker("/seekable-worker.js", { type: "module" });
    worker.onerror = (event) => {
      worker.terminate();
      reject(new Error(event.message));
    };
    worker.onmessage = ({ data }) => {
      worker.terminate();
      if (!data?.ok) {
        reject(new Error(data?.error ?? "seekable worker failed"));
        return;
      }
      resolve(data);
    };
    worker.postMessage(file);
  });
}

try {
  await init();
  const response = await fetch("/fixture.png");
  if (!response.ok) {
    throw new Error(`fixture request failed with HTTP ${response.status}`);
  }

  const inputBytes = new Uint8Array(await response.arrayBuffer());
  const result = thumbnailPng(inputBytes, options);
  const bytes = result.bytes;

  if (result.width !== 32 || result.height !== 32) {
    throw new Error(`unexpected dimensions ${result.width}x${result.height}`);
  }
  if (result.mimeType !== "image/png" || !hasPngSignature(bytes)) {
    throw new Error("thumbnail output is not a PNG");
  }

  const pngBuffer = new Uint8Array(bytes).buffer;
  const bitmap = await createImageBitmap(
    new Blob([pngBuffer], { type: "image/png" }),
  );
  const decodedDimensions = `${bitmap.width}x${bitmap.height}`;
  bitmap.close();
  if (decodedDimensions !== "32x32") {
    throw new Error(`decoded PNG is ${decodedDimensions}`);
  }
  result.free();

  const seekable = await runSeekableWorker(
    new Blob([inputBytes], { type: "image/png" }),
  );
  if (
    seekable.metadata.width !== 32
    || seekable.metadata.height !== 32
    || seekable.metadata.format !== "png"
    || seekable.metadata.chunkCount < 1
    || !equalBytes(seekable.bytes, bytes)
    || !equalBytes(seekable.chunkBytes, bytes)
  ) {
    throw new Error("seekable File worker output differs from slice output");
  }

  const chunkOptions: ThumbnailOptions = {
    maxWidth: 256,
    maxHeight: 256,
    allowUpscale: true,
    output: "png",
    png: { color: "rgba8", compression: "none", filter: "none" },
    maxMemoryBytes: 32 * 1024 * 1024,
  };
  const expectedResult = thumbnailPng(inputBytes, chunkOptions);
  const expectedBytes = expectedResult.bytes;
  expectedResult.free();
  const chunks: Uint8Array[] = [];
  const onChunk: ThumbnailChunkCallback = (chunk) => chunks.push(chunk);
  const chunkedResult = thumbnailPngToChunks(inputBytes, onChunk, chunkOptions);
  if (chunks.length <= 1 || chunks.some((chunk) => chunk.length > 64 * 1024)) {
    throw new Error("chunked PNG did not produce multiple bounded chunks");
  }
  const joined = new Uint8Array(chunkedResult.bytesWritten);
  let offset = 0;
  for (const chunk of chunks) {
    joined.set(chunk, offset);
    offset += chunk.length;
  }
  if (
    chunkedResult.chunkCount !== chunks.length
    || !equalBytes(joined, expectedBytes)
  ) {
    throw new Error("chunked PNG differs from buffered output");
  }
  chunkedResult.free();

  const sentinel = { source: "chunk callback" };
  try {
    thumbnailPngToChunks(inputBytes, () => { throw sentinel; }, chunkOptions);
    throw new Error("throwing chunk callback unexpectedly succeeded");
  } catch (error) {
    if (error !== sentinel) {
      throw new Error("chunk callback exception identity was not preserved");
    }
  }

  await finish(
    "pass",
    `PASS: @streamthumb/wasm ${streamthumbVersion()} verified buffered, multi-chunk, and seekable File output`,
  );
} catch (error) {
  await finish("fail", `FAIL: ${error instanceof Error ? error.stack : error}`);
}
