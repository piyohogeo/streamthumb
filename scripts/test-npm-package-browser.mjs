import { spawn, spawnSync } from "node:child_process";
import { cp, mkdir, readFile, rm } from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

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

function runNpm(arguments_) {
  let command = "npm";
  let commandArguments = arguments_;
  if (process.platform === "win32") {
    command = process.execPath;
    commandArguments = [
      path.join(
        path.dirname(process.execPath),
        "node_modules",
        "npm",
        "bin",
        "npm-cli.js",
      ),
      ...arguments_,
    ];
  }

  const result = spawnSync(command, commandArguments, {
    cwd: consumer,
    encoding: "utf8",
    env: {
      ...process.env,
      npm_config_cache: path.join(target, "npm-cache"),
    },
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    process.stderr.write(result.stdout);
    process.stderr.write(result.stderr);
    throw new Error(`npm ${arguments_[0]} exited with status ${result.status}`);
  }
}

runNpm(["ci", "--ignore-scripts", "--no-audit", "--no-fund"]);
runNpm([
  "install",
  "--ignore-scripts",
  "--no-audit",
  "--no-fund",
  "--no-package-lock",
  "--no-save",
  tarball,
]);

const typeCheck = spawnSync(
  process.execPath,
  [path.join(consumer, "node_modules", "typescript", "bin", "tsc")],
  { cwd: consumer, encoding: "utf8" },
);
if (typeCheck.error) {
  throw typeCheck.error;
}
if (typeCheck.status !== 0) {
  process.stderr.write(typeCheck.stdout);
  process.stderr.write(typeCheck.stderr);
  throw new Error(`TypeScript exited with status ${typeCheck.status}`);
}

const esbuild = await import(
  pathToFileURL(
    path.join(consumer, "node_modules", "esbuild", "lib", "main.js"),
  ).href
);
await esbuild.build({
  entryPoints: [path.join(consumer, "smoke.ts")],
  bundle: true,
  format: "esm",
  logLevel: "warning",
  outfile: path.join(consumer, "bundle.js"),
  platform: "browser",
  target: "es2022",
});
await esbuild.build({
  entryPoints: [path.join(consumer, "seekable-worker.js")],
  bundle: true,
  format: "esm",
  logLevel: "warning",
  outfile: path.join(consumer, "seekable-worker.bundle.js"),
  platform: "browser",
  target: "es2022",
});

const packageRoot = path.join(
  consumer,
  "node_modules",
  "@streamthumb",
  "wasm",
);
const routes = new Map([
  ["/", [path.join(consumer, "smoke.html"), "text/html; charset=utf-8"]],
  [
    "/bundle.js",
    [path.join(consumer, "bundle.js"), "text/javascript; charset=utf-8"],
  ],
  [
    "/seekable-worker.js",
    [
      path.join(consumer, "seekable-worker.bundle.js"),
      "text/javascript; charset=utf-8",
    ],
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
    "/streamthumb_wasm_bg.wasm",
    [path.join(packageRoot, "streamthumb_wasm_bg.wasm"), "application/wasm"],
  ],
]);

let resolveReport;
const reportPromise = new Promise((resolve) => {
  resolveReport = resolve;
});
let reportReceived = false;

const server = http.createServer(async (request, response) => {
  try {
    const pathname = new URL(request.url, "http://localhost").pathname;
    if (pathname === "/result" && request.method === "POST") {
      let body = "";
      for await (const chunk of request) {
        body += chunk;
        if (body.length > 65_536) {
          throw new Error("Browser result exceeds 65,536 bytes.");
        }
      }
      const report = JSON.parse(body);
      if (
        !reportReceived
        && (report.result === "pass" || report.result === "fail")
        && typeof report.message === "string"
      ) {
        reportReceived = true;
        response.writeHead(204);
        response.end(() => resolveReport(report));
        return;
      }
      response.writeHead(204).end();
      return;
    }
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
      `http://127.0.0.1:${address.port}/`,
    ],
    { stdio: ["ignore", "pipe", "pipe"] },
  );
  let stderr = "";
  browser.stdout.resume();
  browser.stderr.setEncoding("utf8").on("data", (chunk) => {
    stderr += chunk;
  });

  let exitCode;
  const browserExit = new Promise((resolve, reject) => {
    browser.once("error", reject);
    browser.once("exit", (code) => {
      exitCode = code;
      resolve(code);
    });
  });
  let timeout;
  const timeoutPromise = new Promise((_, reject) => {
    timeout = setTimeout(
      () => reject(new Error("Chrome smoke test timed out.")),
      30000,
    );
  });

  try {
    const report = await Promise.race([
      reportPromise,
      timeoutPromise,
      browserExit.then((code) => {
        throw new Error(`Chrome exited before reporting with status ${code}`);
      }),
    ]);
    if (report.result !== "pass") {
      throw new Error(report.message);
    }
    console.log(report.message);
  } catch (error) {
    process.stderr.write(stderr);
    throw error;
  } finally {
    clearTimeout(timeout);
    if (exitCode === undefined) {
      browser.kill();
      await Promise.race([
        browserExit.catch(() => undefined),
        new Promise((resolve) => setTimeout(resolve, 5000)),
      ]);
    }
  }
} finally {
  await new Promise((resolve) => server.close(resolve));
  try {
    await rm(chromeProfile, {
      recursive: true,
      force: true,
      maxRetries: 10,
      retryDelay: 200,
    });
  } catch (error) {
    console.warn(`Could not remove the temporary Chrome profile: ${error}`);
  }
}
