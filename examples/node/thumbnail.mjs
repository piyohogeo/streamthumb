import { readFile, writeFile } from "node:fs/promises";
import init, { thumbnailPng } from "@streamthumb/wasm";

const [inputPath, outputPath] = process.argv.slice(2);
if (!inputPath || !outputPath) {
  throw new Error("Usage: node thumbnail.mjs <input.png> <output.png>");
}

const packageModule = import.meta.resolve("@streamthumb/wasm");
const wasmUrl = new URL("streamthumb_wasm_bg.wasm", packageModule);
await init({ module_or_path: await readFile(wasmUrl) });

const result = thumbnailPng(await readFile(inputPath), {
  maxWidth: 512,
  maxHeight: 512,
  output: "png",
});
const bytes = result.bytes;
const summary = `${result.width}x${result.height} ${result.format}`;
result.free();
await writeFile(outputPath, bytes);
console.log(`Wrote ${summary} to ${outputPath}`);
