import { spawn, spawnSync } from "node:child_process";
import { cp, mkdir, readFile, rm } from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const tarball = path.resolve(root, process.argv[2] ?? "");
const chrome = process.argv[3];

if (!process.argv[2] || !chrome) {
  throw new Error(
    "Usage: node scripts/test-npm-package-browser.mjs <tarball> <chrome>",
  );
}

const target = path.join(root, "target");
const consumer = path.join(target, "npm-browser-consumer");
const chromeProfile = path.join(target, "npm-browser-chrome-profile");
await Promise.all([
  rm(consumer, { recursive: true, force: true }),
  rm(chromeProfile, { recursive: true, force: true }),
]);
await mkdir(consumer, { recursive: true });
await cp(path.join(root, "tests", "npm-browser-consumer"), consumer, {
  recursive: true,
});

const npmArguments = [
  "install",
  "--ignore-scripts",
  "--no-audit",
  "--no-fund",
  "--no-package-lock",
  "--no-save",
  tarball,
];
let npmCommand = "npm";
let commandArguments = npmArguments;
if (process.platform === "win32") {
  npmCommand = process.execPath;
  commandArguments = [
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

const install = spawnSync(npmCommand, commandArguments, {
  cwd: consumer,
  encoding: "utf8",
  env: {
    ...process.env,
    npm_config_cache: path.join(target, "npm-cache"),
  },
});
if (install.error) {
  throw install.error;
}
if (install.status !== 0) {
  process.stderr.write(install.stdout);
  process.stderr.write(install.stderr);
  throw new Error(`npm install exited with status ${install.status}`);
}

const packageRoot = path.join(
  consumer,
  "node_modules",
  "@streamthumb",
  "wasm",
);
const routes = new Map([
  ["/", [path.join(consumer, "smoke.html"), "text/html; charset=utf-8"]],
  [
    "/smoke.mjs",
    [path.join(consumer, "smoke.mjs"), "text/javascript; charset=utf-8"],
  ],
  [
    "/fixture.png",
    [
      path.join(
        root,
        "fuzz",
        "corpus",
        "thumbnail_png",
        "pngsuite_basn6a08.png",
      ),
      "image/png",
    ],
  ],
  [
    "/node_modules/@streamthumb/wasm/streamthumb_wasm.js",
    [path.join(packageRoot, "streamthumb_wasm.js"), "text/javascript; charset=utf-8"],
  ],
  [
    "/node_modules/@streamthumb/wasm/streamthumb_wasm_bg.wasm",
    [path.join(packageRoot, "streamthumb_wasm_bg.wasm"), "application/wasm"],
  ],
]);

const server = http.createServer(async (request, response) => {
  try {
    const pathname = new URL(request.url, "http://localhost").pathname;
    const route = routes.get(pathname);
    if (!route) {
      response.writeHead(404).end("Not found");
      return;
    }
    const [file, contentType] = route;
    const body = await readFile(file);
    response.writeHead(200, {
      "Cache-Control": "no-store",
      "Content-Length": body.length,
      "Content-Type": contentType,
    });
    response.end(body);
  } catch (error) {
    response.writeHead(500).end(String(error));
  }
});

await new Promise((resolve, reject) => {
  server.once("error", reject);
  server.listen(0, "127.0.0.1", resolve);
});

try {
  const address = server.address();
  if (!address || typeof address === "string") {
    throw new Error("Could not determine the smoke-test server address.");
  }
  const browser = spawn(
    chrome,
    [
      "--headless=new",
      "--disable-dev-shm-usage",
      "--no-sandbox",
      `--user-data-dir=${chromeProfile}`,
      "--virtual-time-budget=20000",
      "--dump-dom",
      `http://127.0.0.1:${address.port}/`,
    ],
    { stdio: ["ignore", "pipe", "pipe"] },
  );
  let stdout = "";
  let stderr = "";
  browser.stdout.setEncoding("utf8").on("data", (chunk) => {
    stdout += chunk;
  });
  browser.stderr.setEncoding("utf8").on("data", (chunk) => {
    stderr += chunk;
  });

  const exitCode = await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      browser.kill();
      reject(new Error("Chrome smoke test timed out."));
    }, 30000);
    browser.once("error", (error) => {
      clearTimeout(timeout);
      reject(error);
    });
    browser.once("exit", (code) => {
      clearTimeout(timeout);
      resolve(code);
    });
  });
  if (exitCode !== 0 || !stdout.includes('data-result="pass"')) {
    process.stderr.write(stderr);
    process.stderr.write(stdout);
    throw new Error(`Chrome package smoke test failed with status ${exitCode}`);
  }
  const result = stdout.match(/PASS: @streamthumb\/wasm[^<]+/)?.[0];
  console.log(result ?? "PASS: npm package browser smoke test completed");
} finally {
  await new Promise((resolve) => server.close(resolve));
  await rm(chromeProfile, { recursive: true, force: true });
}
