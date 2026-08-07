import init, {
  streamthumbVersion,
  thumbnailPng,
  type ThumbnailOptions,
  type ThumbnailOutputFormat,
  type PngColorMode,
  type PngCompression,
  type PngFilter,
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
// @ts-expect-error JPEG output is not supported.
const invalidOutput: ThumbnailOptions = { output: "jpeg" };
// @ts-expect-error Width must be numeric.
const invalidWidth: ThumbnailOptions = { maxWidth: "32" };
// @ts-expect-error Indexed PNG output is not supported.
const invalidPngColor: ThumbnailOptions = { png: { color: "indexed8" } };
void invalidOutput;
void invalidWidth;
void invalidPngColor;

try {
  await init();
  const response = await fetch("/fixture.png");
  if (!response.ok) {
    throw new Error(`fixture request failed with HTTP ${response.status}`);
  }

  const result = thumbnailPng(
    new Uint8Array(await response.arrayBuffer()),
    options,
  );
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

  await finish(
    "pass",
    `PASS: @streamthumb/wasm ${streamthumbVersion()} created and decoded a 32x32 PNG`,
  );
} catch (error) {
  await finish("fail", `FAIL: ${error instanceof Error ? error.stack : error}`);
}
