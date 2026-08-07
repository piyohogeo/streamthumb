const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");
const { performance } = require("node:perf_hooks");

function measureOne(packageDirectory, inputPath, maxDimensionText, format, fit, delivery) {
  const streamthumb = require(path.resolve(packageDirectory));
  const wasmBinaryBytes = fs
    .readdirSync(packageDirectory)
    .filter((name) => name.endsWith("_bg.wasm"))
    .reduce(
      (total, name) => total + fs.statSync(path.join(packageDirectory, name)).size,
      0,
    );
  const maxDimension = Number(maxDimensionText);
  const input = fs.readFileSync(inputPath);
  const memoryBefore = streamthumb.wasmMemoryBytes();
  const started = performance.now();
  const options = {
    maxWidth: maxDimension,
    maxHeight: maxDimension,
    fit,
    output: format,
    maxMemoryBytes: 512 * 1024 * 1024,
  };
  const result = delivery === "chunks"
    ? streamthumb.thumbnailPngToChunks(input, () => {}, options)
    : streamthumb.thumbnailPng(input, options);
  const runtimeMs = performance.now() - started;
  const outputBytes = delivery === "chunks" ? result.bytesWritten : result.bytes.length;
  const memoryAfter = streamthumb.wasmMemoryBytes();
  const dimensions = path.basename(inputPath).match(/-(\d+)x(\d+)\.png$/);
  const record = {
    method: `streamthumb-wasm-${delivery}-${fit}-${format}`,
    input: inputPath,
    encoded_input_bytes: input.length,
    source_width: dimensions ? Number(dimensions[1]) : null,
    source_height: dimensions ? Number(dimensions[2]) : null,
    output_width: result.width,
    output_height: result.height,
    runtime_ms: runtimeMs,
    output_bytes: outputBytes,
    wasm_memory_before_bytes: memoryBefore,
    wasm_memory_high_water_bytes: memoryAfter,
    wasm_memory_growth_bytes: memoryAfter - memoryBefore,
    node_rss_bytes: process.memoryUsage().rss,
    node_max_rss_bytes: process.resourceUsage().maxRSS * 1024,
    wasm_binary_bytes: wasmBinaryBytes,
  };
  result.free();
  process.stdout.write(`${JSON.stringify(record)}\n`);
}

function runParent(packageDirectory, corpusDirectory, resultFile, maxDimensionText) {
  const files = fs.readdirSync(corpusDirectory).filter((name) => name.endsWith(".png")).sort();
  fs.mkdirSync(path.dirname(resultFile), { recursive: true });
  const output = fs.openSync(resultFile, "w");
  try {
    for (const file of files) {
      const inputPath = path.join(corpusDirectory, file);
      for (const delivery of ["buffered", "chunks"]) {
        for (const [format, fit] of [["png", "contain"], ["jpeg", "contain"], ["png", "cover"], ["jpeg", "cover"]]) {
          const child = spawnSync(process.execPath, [__filename, packageDirectory, inputPath, maxDimensionText, format, fit, delivery], {
            encoding: "utf8",
            env: { ...process.env, STREAMTHUMB_BENCHMARK_CHILD: "1" },
          });
          if (child.status !== 0) {
            throw new Error(`WASM ${delivery} ${fit} ${format} benchmark failed for ${inputPath}: ${child.stderr}`);
          }
          fs.writeSync(output, child.stdout);
        }
      }
    }
  } finally {
    fs.closeSync(output);
  }
}

if (process.env.STREAMTHUMB_BENCHMARK_CHILD === "1") {
  const [packageDirectory, inputPath, maxDimensionText = "512", format = "png", fit = "contain", delivery = "buffered"] = process.argv.slice(2);
  if (format !== "png" && format !== "jpeg") {
    throw new Error("format must be png or jpeg");
  }
  if (fit !== "contain" && fit !== "cover") {
    throw new Error("fit must be contain or cover");
  }
  if (delivery !== "buffered" && delivery !== "chunks") {
    throw new Error("delivery must be buffered or chunks");
  }
  measureOne(packageDirectory, inputPath, maxDimensionText, format, fit, delivery);
} else {
  const [packageDirectory, corpusDirectory, resultFile, maxDimensionText = "512"] = process.argv.slice(2);
  if (!packageDirectory || !corpusDirectory || !resultFile) {
    throw new Error("usage: node run-wasm.cjs <package-directory> <corpus-directory> <result-file> [max-dimension]");
  }
  runParent(packageDirectory, corpusDirectory, resultFile, maxDimensionText);
}
