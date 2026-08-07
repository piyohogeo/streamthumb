import { copyFile, cp, mkdir, readFile, rm } from "node:fs/promises";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const tarball = path.resolve(root, process.argv[2] ?? "");
const nodeOnly = process.argv.includes("--node-only");

if (!process.argv[2]) {
  throw new Error(
    "Usage: node scripts/test-npm-package-runtimes.mjs <tarball> [--node-only]",
  );
}

const target = path.join(root, "target");
const consumer = path.join(target, "npm-runtime-consumer");
await rm(consumer, { recursive: true, force: true });
await mkdir(consumer, { recursive: true });
await cp(path.join(root, "tests", "npm-runtime-consumer"), consumer, {
  recursive: true,
});
await copyFile(
  path.join(
    root,
    "fuzz",
    "corpus",
    "thumbnail_png",
    "pngsuite_basn6a08.png",
  ),
  path.join(consumer, "fixture.png"),
);
await copyFile(
  path.join(root, "examples", "node", "thumbnail.mjs"),
  path.join(consumer, "node-example.mjs"),
);
await copyFile(
  path.join(root, "examples", "deno", "thumbnail.ts"),
  path.join(consumer, "deno-example.ts"),
);

async function verifyPng(file, runtime) {
  const bytes = await readFile(file);
  const signature = [137, 80, 78, 71, 13, 10, 26, 10];
  if (!signature.every((value, index) => bytes[index] === value)) {
    throw new Error(`${runtime} example output is not a PNG`);
  }
  const width = bytes.readUInt32BE(16);
  const height = bytes.readUInt32BE(20);
  if (width !== 32 || height !== 32) {
    throw new Error(
      `${runtime} example output is ${width}x${height}, expected 32x32`,
    );
  }
  console.log(`PASS: ${runtime} public example created a 32x32 PNG`);
}

function run(command, arguments_, options = {}) {
  const result = spawnSync(command, arguments_, {
    cwd: consumer,
    encoding: "utf8",
    env: {
      ...process.env,
      npm_config_cache: path.join(target, "npm-cache"),
      DENO_DIR: path.join(target, "deno-cache"),
    },
    ...options,
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    process.stderr.write(result.stdout);
    process.stderr.write(result.stderr);
    throw new Error(`${command} exited with status ${result.status}`);
  }
  process.stdout.write(result.stdout);
}

let npmCommand = "npm";
let npmArguments = [
  "install",
  "--ignore-scripts",
  "--no-audit",
  "--no-fund",
  "--no-package-lock",
  "--save-exact",
  tarball,
];
if (process.platform === "win32") {
  npmCommand = process.execPath;
  npmArguments = [
    path.join(
      path.dirname(process.execPath),
      "node_modules",
      "npm",
      "bin",
      "npm-cli.js",
    ),
    ...npmArguments,
  ];
}
run(npmCommand, npmArguments);
run(process.execPath, [path.join(consumer, "node-smoke.mjs")]);
const nodeOutput = path.join(consumer, "node-example.png");
run(process.execPath, [
  path.join(consumer, "node-example.mjs"),
  path.join(consumer, "fixture.png"),
  nodeOutput,
]);
await verifyPng(nodeOutput, "Node.js");

if (!nodeOnly) {
  run("deno", [
    "run",
    "--check",
    "--node-modules-dir=manual",
    `--allow-read=${consumer}`,
    `--allow-write=${consumer}`,
    path.join(consumer, "deno-smoke.ts"),
  ]);
  const denoOutput = path.join(consumer, "deno-example.png");
  run("deno", [
    "run",
    "--check",
    "--node-modules-dir=manual",
    `--allow-read=${consumer}`,
    `--allow-write=${consumer}`,
    path.join(consumer, "deno-example.ts"),
    path.join(consumer, "fixture.png"),
    denoOutput,
  ]);
  await verifyPng(denoOutput, "Deno");
}
