import { copyFile, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const crate = path.join(root, "crates", "streamthumb-wasm");
const target = path.join(root, "target");
const output = path.resolve(root, process.argv[2] ?? "target/npm-package");
const relativeTarget = path.relative(target, output);
const relativeOutput = path.relative(crate, output);

if (
  !relativeTarget
  || relativeTarget.startsWith(`..${path.sep}`)
  || path.isAbsolute(relativeTarget)
) {
  throw new Error("The output directory must be a child of the repository target directory.");
}

await rm(output, { recursive: true, force: true });
await mkdir(path.dirname(output), { recursive: true });

const result = spawnSync(
  "wasm-pack",
  [
    "build",
    crate,
    "--release",
    "--target",
    "web",
    "--out-dir",
    relativeOutput,
  ],
  { cwd: root, stdio: "inherit" },
);

if (result.error) {
  throw result.error;
}
if (result.status !== 0) {
  throw new Error(`wasm-pack exited with status ${result.status}`);
}

await Promise.all([
  copyFile(path.join(root, "LICENSE-MIT"), path.join(output, "LICENSE-MIT")),
  copyFile(
    path.join(root, "LICENSE-APACHE"),
    path.join(output, "LICENSE-APACHE"),
  ),
  rm(path.join(output, "LICENSE"), { force: true }),
]);

const packagePath = path.join(output, "package.json");
const manifest = JSON.parse(await readFile(packagePath, "utf8"));
const generatedFiles = [
  "streamthumb_wasm_bg.wasm",
  "streamthumb_wasm.js",
  "streamthumb_wasm.d.ts",
  "streamthumb_wasm_bg.wasm.d.ts",
];

Object.assign(manifest, {
  name: "@streamthumb/wasm",
  description:
    "Memory-bounded streaming PNG thumbnail generation for WebAssembly",
  repository: {
    type: "git",
    url: "git+https://github.com/piyohogeo/streamthumb.git",
    directory: "crates/streamthumb-wasm",
  },
  homepage: "https://github.com/piyohogeo/streamthumb#readme",
  bugs: "https://github.com/piyohogeo/streamthumb/issues",
  keywords: ["png", "thumbnail", "webassembly", "wasm", "image"],
  files: [...generatedFiles, "README.md", "LICENSE-MIT", "LICENSE-APACHE"],
  main: "./streamthumb_wasm.js",
  module: "./streamthumb_wasm.js",
  types: "./streamthumb_wasm.d.ts",
  exports: {
    ".": {
      types: "./streamthumb_wasm.d.ts",
      import: "./streamthumb_wasm.js",
    },
  },
  publishConfig: { access: "public" },
});

await writeFile(packagePath, `${JSON.stringify(manifest, null, 2)}\n`);
console.log(`Built ${manifest.name}@${manifest.version} in ${path.relative(root, output)}`);
