import { spawn } from "node:child_process";
import { readFile, rm } from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const chrome = process.argv[2];
if (!chrome) {
  throw new Error("Usage: node scripts/test-browser-example.mjs <chrome>");
}

const chromeProfile = path.join(root, "target", "browser-example-chrome-profile");
await rm(chromeProfile, { recursive: true, force: true });

const routes = new Map([
  [
    "/examples/browser/smoke.html",
    [path.join(root, "examples", "browser", "smoke.html"), "text/html; charset=utf-8"],
  ],
  [
    "/examples/browser/smoke.js",
    [path.join(root, "examples", "browser", "smoke.js"), "text/javascript; charset=utf-8"],
  ],
  [
    "/examples/browser/worker.js",
    [path.join(root, "examples", "browser", "worker.js"), "text/javascript; charset=utf-8"],
  ],
  [
    "/target/npm-package/streamthumb_wasm.js",
    [path.join(root, "target", "npm-package", "streamthumb_wasm.js"), "text/javascript; charset=utf-8"],
  ],
  [
    "/target/npm-package/streamthumb_wasm_bg.wasm",
    [path.join(root, "target", "npm-package", "streamthumb_wasm_bg.wasm"), "application/wasm"],
  ],
  [
    "/fuzz/corpus/thumbnail_png/pngsuite_basn6a08.png",
    [path.join(root, "fuzz", "corpus", "thumbnail_png", "pngsuite_basn6a08.png"), "image/png"],
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
    if (pathname === "/example-result" && request.method === "POST") {
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
    throw new Error("Could not determine the browser example server address.");
  }

  const browser = spawn(
    chrome,
    [
      "--headless=new",
      "--disable-dev-shm-usage",
      "--no-sandbox",
      `--user-data-dir=${chromeProfile}`,
      `http://127.0.0.1:${address.port}/examples/browser/smoke.html`,
    ],
    { stdio: ["ignore", "pipe", "pipe"] },
  );
  browser.stdout.resume();
  let stderr = "";
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
      () => reject(new Error("Browser example smoke test timed out.")),
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
