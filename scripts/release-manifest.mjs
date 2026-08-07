import { createHash } from "node:crypto";
import { readFile, readdir, stat, writeFile } from "node:fs/promises";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const command = process.argv[2];
const arguments_ = process.argv.slice(3);

function option(name, fallback) {
  const index = arguments_.indexOf(name);
  if (index === -1) {
    return fallback;
  }
  const value = arguments_[index + 1];
  if (!value || value.startsWith("--")) {
    throw new Error(`${name} requires a value.`);
  }
  return value;
}

if (command !== "create" && command !== "check") {
  throw new Error(
    "Usage: node scripts/release-manifest.mjs <create|check> [options]",
  );
}

const packageDirectory = path.resolve(
  root,
  option("--package-directory", "target/npm-package"),
);
const artifactsDirectory = path.resolve(
  root,
  option("--artifacts-directory", "target/npm-artifacts"),
);
const artifactOnly = arguments_.includes("--artifact-only");

function run(commandName, commandArguments = []) {
  const effectiveArguments = commandName === "git"
    ? ["-c", `safe.directory=${root.replaceAll("\\", "/")}`, ...commandArguments]
    : commandArguments;
  const result = spawnSync(commandName, effectiveArguments, {
    cwd: root,
    encoding: "utf8",
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    process.stderr.write(result.stderr);
    throw new Error(`${commandName} exited with status ${result.status}`);
  }
  return result.stdout.trim();
}

function npmVersion() {
  if (process.platform !== "win32") {
    return run("npm", ["--version"]);
  }
  return run(process.execPath, [
    path.join(
      path.dirname(process.execPath),
      "node_modules",
      "npm",
      "bin",
      "npm-cli.js",
    ),
    "--version",
  ]);
}

function workspaceVersion(cargoToml) {
  const section = cargoToml.match(
    /^\[workspace\.package\]\s*$([\s\S]*?)(?=^\[|(?![\s\S]))/m,
  )?.[1];
  const version = section?.match(/^version\s*=\s*"([^"]+)"\s*$/m)?.[1];
  if (!version) {
    throw new Error("Could not read workspace.package.version from Cargo.toml.");
  }
  return version;
}

function escapeRegularExpression(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

async function sha256(file) {
  return createHash("sha256").update(await readFile(file)).digest("hex");
}

const cargoToml = await readFile(path.join(root, "Cargo.toml"), "utf8");
const changelog = await readFile(path.join(root, "CHANGELOG.md"), "utf8");
const version = workspaceVersion(cargoToml);
const versionHeading = new RegExp(
  `^## \\[${escapeRegularExpression(version)}\\] - (?:Unreleased|\\d{4}-\\d{2}-\\d{2})$`,
  "m",
);
if (!versionHeading.test(changelog)) {
  throw new Error(
    `CHANGELOG.md must contain a release heading for workspace version ${version}.`,
  );
}

const tarballName = `streamthumb-wasm-${version}.tgz`;
const tarballPath = path.join(artifactsDirectory, tarballName);
const manifestPath = path.join(artifactsDirectory, "release-manifest.json");
const checksumPath = path.join(artifactsDirectory, `${tarballName}.sha256`);

async function verifyPackageDirectory() {
  const packageManifest = JSON.parse(
    await readFile(path.join(packageDirectory, "package.json"), "utf8"),
  );
  if (
    packageManifest.name !== "@streamthumb/wasm"
    || packageManifest.version !== version
  ) {
    throw new Error(
      `Generated package identity does not match @streamthumb/wasm@${version}.`,
    );
  }
}

if (command === "create") {
  await verifyPackageDirectory();
  const sourceRevision = option(
    "--source-revision",
    run("git", ["rev-parse", "HEAD"]),
  );
  if (!/^[0-9a-f]{40}$/.test(sourceRevision)) {
    throw new Error(
      "The source revision must be a full lowercase Git commit SHA.",
    );
  }

  const tarball = await stat(tarballPath);
  const digest = await sha256(tarballPath);
  const manifest = {
    schemaVersion: 1,
    package: {
      name: "@streamthumb/wasm",
      version,
    },
    source: {
      repository: "https://github.com/piyohogeo/streamthumb",
      revision: sourceRevision,
    },
    tools: {
      node: process.version.slice(1),
      npm: npmVersion(),
      rustc: run("rustc", ["--version"]),
      wasmPack: run("wasm-pack", ["--version"]).replace(/^wasm-pack\s+/, ""),
    },
    artifact: {
      file: tarballName,
      bytes: tarball.size,
      sha256: digest,
    },
  };

  await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
  await writeFile(checksumPath, `${digest}  ${tarballName}\n`);
  console.log(`Created release manifest for @streamthumb/wasm@${version}`);
} else {
  if (!artifactOnly) {
    await verifyPackageDirectory();
  }
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  const expectedRevision = option(
    "--source-revision",
    run("git", ["rev-parse", "HEAD"]),
  );
  const digest = await sha256(tarballPath);
  const tarball = await stat(tarballPath);
  const checksum = await readFile(checksumPath, "utf8");

  if (
    manifest.schemaVersion !== 1
    || manifest.package?.name !== "@streamthumb/wasm"
    || manifest.package?.version !== version
    || manifest.source?.repository !== "https://github.com/piyohogeo/streamthumb"
    || manifest.source?.revision !== expectedRevision
    || manifest.artifact?.file !== tarballName
    || manifest.artifact?.bytes !== tarball.size
    || manifest.artifact?.sha256 !== digest
  ) {
    throw new Error("release-manifest.json does not match the source or tarball.");
  }
  if (checksum !== `${digest}  ${tarballName}\n`) {
    throw new Error("The SHA-256 checksum file does not match the tarball.");
  }
  if (
    manifest.tools?.node !== "24.14.1"
    || manifest.tools?.npm !== "11.11.0"
    || manifest.tools?.wasmPack !== "0.15.0"
    || !/^rustc 1\.85\.0 /.test(manifest.tools?.rustc ?? "")
  ) {
    throw new Error("The release candidate was not built with the pinned tools.");
  }

  const files = (await readdir(artifactsDirectory)).sort();
  const expectedFiles = [
    "release-manifest.json",
    tarballName,
    `${tarballName}.sha256`,
  ].sort();
  if (JSON.stringify(files) !== JSON.stringify(expectedFiles)) {
    throw new Error(`Unexpected release artifact files: ${files.join(", ")}`);
  }
  console.log(
    `Verified @streamthumb/wasm@${version}: ${tarball.size} bytes, sha256 ${digest}`,
  );
}
