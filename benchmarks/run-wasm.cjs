const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");
const { performance } = require("node:perf_hooks");

function measureOne(packageDirectory, inputPath, maxDimensionText) {
  const streamthumb = require(path.resolve(packageDirectory));
  const maxDimension = Number(maxDimensionText);
  const input = fs.readFileSync(inputPath);
  const memoryBefore = streamthumb.wasmMemoryBytes();
  const started = performance.now();
  const result = streamthumb.thumbnailPng(input, {
    maxWidth: maxDimension,
    maxHeight: maxDimension,
    output: "png",
  });
  const runtimeMs = performance.now() - started;
  const bytes = result.bytes;
  const memoryAfter = streamthumb.wasmMemoryBytes();
  const dimensions = path.basename(inputPath).match(/-(\d+)x(\d+)\.png$/);
  const record = {
    method: "streamthumb-wasm",
    input: inputPath,
    encoded_input_bytes: input.length,
    source_width: dimensions ? Number(dimensions[1]) : null,
    source_height: dimensions ? Number(dimensions[2]) : null,
    output_width: result.width,
    output_height: result.height,
    runtime_ms: runtimeMs,
    output_bytes: bytes.length,
    wasm_memory_before_bytes: memoryBefore,
    wasm_memory_high_water_bytes: memoryAfter,
    wasm_memory_growth_bytes: memoryAfter - memoryBefore,
    node_rss_bytes: process.memoryUsage().rss,
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
      const child = spawnSync(process.execPath, [__filename, packageDirectory, inputPath, maxDimensionText], {
        encoding: "utf8",
        env: { ...process.env, STREAMTHUMB_BENCHMARK_CHILD: "1" },
      });
      if (child.status !== 0) {
        throw new Error(`WASM benchmark failed for ${inputPath}: ${child.stderr}`);
      }
      fs.writeSync(output, child.stdout);
    }
  } finally {
    fs.closeSync(output);
  }
}

if (process.env.STREAMTHUMB_BENCHMARK_CHILD === "1") {
  const [packageDirectory, inputPath, maxDimensionText = "512"] = process.argv.slice(2);
  measureOne(packageDirectory, inputPath, maxDimensionText);
} else {
  const [packageDirectory, corpusDirectory, resultFile, maxDimensionText = "512"] = process.argv.slice(2);
  if (!packageDirectory || !corpusDirectory || !resultFile) {
    throw new Error("usage: node run-wasm.cjs <package-directory> <corpus-directory> <result-file> [max-dimension]");
  }
  runParent(packageDirectory, corpusDirectory, resultFile, maxDimensionText);
}
