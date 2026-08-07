import { copyFile, cp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { generatePagesSample } from "./generate-pages-sample.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const target = path.join(root, "target");
const output = path.resolve(root, process.argv[2] ?? "target/pages");
const source = path.join(root, "examples", "pages");
const packageOutput = path.join(target, "npm-package");
const relativeTarget = path.relative(target, output);
const revisionToken = "__STREAMTHUMB_REVISION__";

if (!relativeTarget || relativeTarget.startsWith(`..${path.sep}`) || path.isAbsolute(relativeTarget)) {
  throw new Error("The Pages output directory must be a child of the repository target directory.");
}

await rm(output, { recursive: true, force: true });
await mkdir(output, { recursive: true });
await cp(source, output, { recursive: true });
await mkdir(path.join(output, "vendor"), { recursive: true });
await mkdir(path.join(output, "samples"), { recursive: true });

await Promise.all([
  copyFile(path.join(packageOutput, "streamthumb_wasm.js"), path.join(output, "vendor", "streamthumb_wasm.js")),
  copyFile(path.join(packageOutput, "streamthumb_wasm_bg.wasm"), path.join(output, "vendor", "streamthumb_wasm_bg.wasm")),
  generatePagesSample(path.join(output, "samples", "large-rgba.png")),
  writeFile(path.join(output, ".nojekyll"), ""),
]);

const git = spawnSync("git", ["-c", `safe.directory=${root.replaceAll("\\", "/")}`, "rev-parse", "HEAD"], { cwd: root, encoding: "utf8" });
const revision = process.env.GITHUB_SHA?.trim() || (git.status === 0 ? git.stdout.trim() : "unknown");
const cacheKey = /^[0-9a-f]{40}$/.test(revision) ? revision : "development";

async function stampRevision(relativePath) {
  const file = path.join(output, relativePath);
  const contents = await readFile(file, "utf8");
  if (!contents.includes(revisionToken)) throw new Error(`${relativePath} is missing the revision cache token.`);
  await writeFile(file, contents.replaceAll(revisionToken, cacheKey));
}

await Promise.all([
  stampRevision("index.html"),
  stampRevision("main.js"),
  stampRevision("smoke.html"),
  stampRevision("smoke.js"),
  stampRevision("worker.js"),
]);

const wasmGluePath = path.join(output, "vendor", "streamthumb_wasm.js");
const wasmGlue = await readFile(wasmGluePath, "utf8");
const wasmPath = "streamthumb_wasm_bg.wasm";
if (!wasmGlue.includes(`new URL('${wasmPath}', import.meta.url)`)) {
  throw new Error("The generated WebAssembly glue has an unexpected module URL.");
}
await writeFile(
  wasmGluePath,
  wasmGlue.replace(`new URL('${wasmPath}', import.meta.url)`, `new URL('${wasmPath}?v=${cacheKey}', import.meta.url)`),
);
await writeFile(path.join(output, "build-metadata.json"), `${JSON.stringify({ revision }, null, 2)}\n`);

const [index, main, worker, manifest] = await Promise.all([
  readFile(path.join(output, "index.html"), "utf8"),
  readFile(path.join(output, "main.js"), "utf8"),
  readFile(path.join(output, "worker.js"), "utf8"),
  readFile(path.join(output, "sample-manifest.json"), "utf8").then(JSON.parse),
]);

for (const [name, contents] of [["index.html", index], ["main.js", main], ["worker.js", worker]]) {
  if (/\b(?:src|href)=["']\//.test(contents) || /(?:from|new URL\()[\s\S]{0,20}["']\//.test(contents)) {
    throw new Error(`${name} contains a root-relative asset URL.`);
  }
}
if (!worker.includes(`from "./vendor/streamthumb_wasm.js?v=${cacheKey}"`)) throw new Error("The Pages worker must import the revisioned WebAssembly module.");
for (const required of ["planThumbnailPngFromSeekable", "thumbnailPngFromSeekable", "thumbnailPngFromSeekableToChunks", "FileReaderSync"]) {
  if (!worker.includes(required)) throw new Error(`The Pages worker must use ${required}.`);
}
if (main.includes(".arrayBuffer()")) throw new Error("The Pages main thread must retain File and Blob input without materializing an ArrayBuffer.");
for (const [name, contents] of [["index.html", index], ["main.js", main], ["worker.js", worker]]) {
  if (contents.includes(revisionToken) || !contents.includes(cacheKey)) {
    throw new Error(`${name} must contain only resolved revision cache keys.`);
  }
}
if (!Array.isArray(manifest.samples) || manifest.samples.length === 0) throw new Error("The sample manifest must contain at least one sample.");
for (const sample of manifest.samples) {
  if (typeof sample.path !== "string" || !sample.path.startsWith("./samples/")) throw new Error("Every sample must use a Pages-relative samples path.");
  await readFile(path.resolve(output, sample.path));
}

const sampleBytes = await readFile(path.join(output, "samples", "large-rgba.png"));
if (sampleBytes.length >= 256 * 1024) throw new Error("The large RGBA sample must remain smaller than 256 KiB.");
if (sampleBytes.readUInt32BE(16) !== 2048 || sampleBytes.readUInt32BE(20) !== 2048) {
  throw new Error("The large RGBA sample must remain 2048 x 2048 pixels.");
}
if (sampleBytes[24] !== 8 || sampleBytes[25] !== 6) {
  throw new Error("The large RGBA sample must remain an 8-bit RGBA PNG.");
}

console.log(`Built Pages demo for ${revision.slice(0, 12)} in ${path.relative(root, output)}`);
