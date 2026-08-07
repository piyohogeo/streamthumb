import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

async function source(relativePath) {
  return readFile(path.join(root, relativePath), "utf8");
}

function requireText(text, expected, file) {
  if (!text.includes(expected)) {
    throw new Error(`${file} must contain: ${expected}`);
  }
}

const optionDefaults = [
  ["maxWidth", "`512`"],
  ["maxHeight", "`512`"],
  ["fit", "`\"contain\"`"],
  ["filter", "`\"area\"`"],
  ["allowUpscale", "`false`"],
  ["output", "`\"png\"`"],
  ["png", "RGBA8, balanced compression, default filter"],
  ["jpeg", "quality 85, white background, 4:2:0"],
  ["maxInputBytes", "`67,108,864` (64 MiB)"],
  ["maxInputWidth", "`100,000`"],
  ["maxInputHeight", "`100,000`"],
  ["maxInputPixels", "`500,000,000`"],
  ["maxOutputWidth", "`8,192`"],
  ["maxOutputHeight", "`8,192`"],
  ["maxOutputPixels", "`16,777,216`"],
  ["maxMemoryBytes", "`33,554,432` (32 MiB)"],
];

const api = await source("docs/WASM_API.md");
for (const [name, defaultValue] of optionDefaults) {
  requireText(api, `| \`${name}\` | ${defaultValue} |`, "docs/WASM_API.md");
}
for (const publicExport of [
  "initSync({ module })",
  "`Symbol.dispose`",
  "function streamthumbVersion(): string;",
  "function wasmMemoryBytes(): number;",
  "function planThumbnailPng(",
  'type OutputDelivery = "buffered" | "chunks";',
  "withinMemoryLimit: boolean;",
  "This result owns no WebAssembly allocation",
  "function thumbnailPngToChunks(",
  "Chunks contain at most 64 KiB",
  "does not\nconvert sRGB values to linear light",
]) {
  requireText(api, publicExport, "docs/WASM_API.md");
}

const options = await source("crates/streamthumb-core/src/options.rs");
for (const implementationDefault of [
  "max_width: 512",
  "max_height: 512",
  "fit: Fit::Contain",
  "allow_upscale: false",
  "filter: Filter::Area",
  "output: OutputFormat::Png",
]) {
  requireText(options, implementationDefault, "crates/streamthumb-core/src/options.rs");
}

const limits = await source("crates/streamthumb-core/src/limits.rs");
for (const implementationDefault of [
  "max_input_bytes: 64 * 1024 * 1024",
  "max_width: 100_000",
  "max_height: 100_000",
  "max_pixels: 500_000_000",
  "max_output_width: 8_192",
  "max_output_height: 8_192",
  "max_output_pixels: 16_777_216",
  "max_working_memory_bytes: 32 * 1024 * 1024",
]) {
  requireText(limits, implementationDefault, "crates/streamthumb-core/src/limits.rs");
}

const pngOptions = await source("crates/streamthumb-png/src/options.rs");
for (const implementationDefault of [
  "color: PngColorMode::Rgba8",
  "compression: PngCompression::Balanced",
  "filter: PngFilter::Default",
]) {
  requireText(
    pngOptions,
    implementationDefault,
    "crates/streamthumb-png/src/options.rs",
  );
}

const jpegOptions = await source("crates/streamthumb-encode/src/jpeg.rs");
for (const implementationDefault of [
  "quality: 85",
  "background: [255, 255, 255]",
  "subsampling: JpegSubsampling::S420",
]) {
  requireText(
    jpegOptions,
    implementationDefault,
    "crates/streamthumb-encode/src/jpeg.rs",
  );
}

const publicExampleFiles = [
  "examples/browser/README.md",
  "examples/browser/worker.js",
  "examples/cloudflare-worker/README.md",
  "examples/cloudflare-worker/package.json",
  "examples/node/README.md",
  "examples/node/thumbnail.mjs",
  "examples/deno/README.md",
  "examples/deno/thumbnail.ts",
];
for (const file of publicExampleFiles) {
  const text = await source(file);
  if (text.includes("../../pkg") || text.includes("repository-level `pkg`")) {
    throw new Error(`${file} contains a retired package path`);
  }
}

const browserWorker = await source("examples/browser/worker.js");
requireText(
  browserWorker,
  'from "../../target/npm-package/streamthumb_wasm.js"',
  "examples/browser/worker.js",
);
requireText(browserWorker, "thumbnailPngToChunks", "examples/browser/worker.js");

console.log("PASS: public WebAssembly API documentation and examples are aligned");
