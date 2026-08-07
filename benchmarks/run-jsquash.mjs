import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

import decodePng, { init as initPngDecode } from "@jsquash/png/decode.js";
import encodePng, { init as initPngEncode } from "@jsquash/png/encode.js";
import resize, { initResize } from "@jsquash/resize";

const benchmarkDirectory = path.dirname(fileURLToPath(import.meta.url));
const pngWasmPath = path.join(
  benchmarkDirectory,
  "node_modules/@jsquash/png/codec/pkg/squoosh_png_bg.wasm",
);
const resizeWasmPath = path.join(
  benchmarkDirectory,
  "node_modules/@jsquash/resize/lib/resize/pkg/squoosh_resize_bg.wasm",
);

function readArrayBuffer(filePath) {
  const bytes = fs.readFileSync(filePath);
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
}

function containDimensions(sourceWidth, sourceHeight, maxDimension) {
  if (!Number.isSafeInteger(maxDimension) || maxDimension <= 0) {
    throw new Error("max dimension must be a positive safe integer");
  }

  const boundWidth = Math.min(sourceWidth, maxDimension);
  const boundHeight = Math.min(sourceHeight, maxDimension);
  if (boundWidth * sourceHeight <= boundHeight * sourceWidth) {
    return {
      width: boundWidth,
      height: Math.max(1, Math.floor((sourceHeight * boundWidth) / sourceWidth)),
    };
  }
  return {
    width: Math.max(1, Math.floor((sourceWidth * boundHeight) / sourceHeight)),
    height: boundHeight,
  };
}

async function initializeModules() {
  const pngWasm = readArrayBuffer(pngWasmPath);
  const resizeWasm = readArrayBuffer(resizeWasmPath);
  const pngModule = await initPngDecode(pngWasm);
  await initPngEncode(pngWasm);
  const resizeModule = await initResize(resizeWasm);
  return { pngModule, resizeModule };
}

function wasmMemoryBytes(modules) {
  return (
    modules.pngModule.memory.buffer.byteLength +
    modules.resizeModule.memory.buffer.byteLength
  );
}

async function measureOne(inputPath, maxDimensionText) {
  const maxDimension = Number(maxDimensionText);
  const input = readArrayBuffer(inputPath);
  const modules = await initializeModules();
  const memoryBefore = wasmMemoryBytes(modules);
  const started = performance.now();
  const decoded = await decodePng(input);
  const dimensions = containDimensions(
    decoded.width,
    decoded.height,
    maxDimension,
  );
  const resized = await resize(decoded, {
    width: dimensions.width,
    height: dimensions.height,
    method: "triangle",
    fitMethod: "stretch",
    premultiply: true,
    linearRGB: false,
  });
  const encoded = await encodePng(resized);
  const encodedBytes = new Uint8Array(encoded);
  const pngSignature = [137, 80, 78, 71, 13, 10, 26, 10];
  if (
    resized.width !== dimensions.width ||
    resized.height !== dimensions.height ||
    !pngSignature.every((value, index) => encodedBytes[index] === value)
  ) {
    throw new Error("jSquash returned an invalid benchmark output");
  }
  const runtimeMs = performance.now() - started;
  const memoryAfter = wasmMemoryBytes(modules);
  const wasmBinaryBytes =
    fs.statSync(pngWasmPath).size + fs.statSync(resizeWasmPath).size;
  const record = {
    method: "jsquash-png-resize",
    input: inputPath,
    encoded_input_bytes: input.byteLength,
    source_width: decoded.width,
    source_height: decoded.height,
    output_width: resized.width,
    output_height: resized.height,
    runtime_ms: runtimeMs,
    output_bytes: encoded.byteLength,
    wasm_memory_before_bytes: memoryBefore,
    wasm_memory_high_water_bytes: memoryAfter,
    wasm_memory_growth_bytes: memoryAfter - memoryBefore,
    wasm_binary_bytes: wasmBinaryBytes,
    node_rss_bytes: process.memoryUsage().rss,
    node_max_rss_bytes: process.resourceUsage().maxRSS * 1024,
  };
  process.stdout.write(`${JSON.stringify(record)}\n`);
}

function runParent(corpusDirectory, resultFile, maxDimensionText) {
  const files = fs
    .readdirSync(corpusDirectory)
    .filter((name) => name.endsWith(".png"))
    .sort();
  fs.mkdirSync(path.dirname(resultFile), { recursive: true });
  const output = fs.openSync(resultFile, "w");
  try {
    for (const file of files) {
      const inputPath = path.join(corpusDirectory, file);
      const child = spawnSync(
        process.execPath,
        [fileURLToPath(import.meta.url), inputPath, maxDimensionText],
        {
          encoding: "utf8",
          env: { ...process.env, STREAMTHUMB_BENCHMARK_CHILD: "1" },
        },
      );
      if (child.status !== 0) {
        throw new Error(
          `jSquash benchmark failed for ${inputPath}: ${child.stderr}`,
        );
      }
      fs.writeSync(output, child.stdout);
    }
  } finally {
    fs.closeSync(output);
  }
}

if (process.env.STREAMTHUMB_BENCHMARK_CHILD === "1") {
  const [inputPath, maxDimensionText = "512"] = process.argv.slice(2);
  await measureOne(inputPath, maxDimensionText);
} else {
  const [corpusDirectory, resultFile, maxDimensionText = "512"] =
    process.argv.slice(2);
  if (!corpusDirectory || !resultFile) {
    throw new Error(
      "usage: node run-jsquash.mjs <corpus-directory> <result-file> [max-dimension]",
    );
  }
  runParent(corpusDirectory, resultFile, maxDimensionText);
}
