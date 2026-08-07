import { spawn } from "node:child_process";
import { readFile, rm, stat } from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const siteRoot = path.join(root, "target", "pages");
const chrome = process.argv[2];
if (!chrome) throw new Error("Usage: node scripts/test-pages-demo.mjs <chrome>");
await stat(path.join(siteRoot, "index.html"));

const mimeTypes = new Map([
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".css", "text/css; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".wasm", "application/wasm"],
  [".png", "image/png"],
  [".txt", "text/plain; charset=utf-8"],
]);
const chromeProfile = path.join(root, "target", "pages-chrome-profile");
await rm(chromeProfile, { recursive: true, force: true });

let resolveReport;
const reportPromise = new Promise((resolve) => { resolveReport = resolve; });
let reportReceived = false;
const server = http.createServer(async (request, response) => {
  try {
    const url = new URL(request.url, "http://localhost");
    if (url.pathname === "/pages-result" && request.method === "POST") {
      let body = "";
      for await (const chunk of request) {
        body += chunk;
        if (body.length > 65_536) throw new Error("Browser result exceeds 65,536 bytes.");
      }
      const report = JSON.parse(body);
      response.writeHead(204).end(() => {
        if (!reportReceived) { reportReceived = true; resolveReport(report); }
      });
      return;
    }

    const pathname = url.pathname === "/streamthumb/" ? "/streamthumb/index.html" : url.pathname;
    if (!pathname.startsWith("/streamthumb/")) {
      response.writeHead(404).end("Not found");
      return;
    }
    const relative = decodeURIComponent(pathname.slice("/streamthumb/".length));
    const file = path.resolve(siteRoot, relative);
    const safeRelative = path.relative(siteRoot, file);
    if (!safeRelative || safeRelative.startsWith(`..${path.sep}`) || path.isAbsolute(safeRelative)) {
      response.writeHead(404).end("Not found");
      return;
    }
    const contentType = mimeTypes.get(path.extname(file));
    if (!contentType) {
      response.writeHead(415).end("Unsupported file type");
      return;
    }
    const body = await readFile(file);
    response.writeHead(200, { "Cache-Control": "no-store", "Content-Length": body.length, "Content-Type": contentType });
    response.end(body);
  } catch (error) {
    response.writeHead(error.code === "ENOENT" ? 404 : 500).end(String(error));
  }
});

await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
try {
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("Could not determine the Pages test server address.");
  const browser = spawn(chrome, ["--headless=new", "--disable-dev-shm-usage", "--no-sandbox", `--user-data-dir=${chromeProfile}`, `http://127.0.0.1:${address.port}/streamthumb/smoke.html`], { stdio: ["ignore", "pipe", "pipe"] });
  browser.stdout.resume();
  let stderr = "";
  browser.stderr.setEncoding("utf8").on("data", (chunk) => { stderr += chunk; });
  let exitCode;
  const browserExit = new Promise((resolve, reject) => { browser.once("error", reject); browser.once("exit", (code) => { exitCode = code; resolve(code); }); });
  let timeout;
  const timeoutPromise = new Promise((_, reject) => { timeout = setTimeout(() => reject(new Error("Pages smoke test timed out.")), 45_000); });
  try {
    const report = await Promise.race([reportPromise, timeoutPromise, browserExit.then((code) => { throw new Error(`Chrome exited before reporting with status ${code}.`); })]);
    if (report.result !== "pass") throw new Error(report.message);
    console.log(report.message);
  } catch (error) {
    process.stderr.write(stderr);
    throw error;
  } finally {
    clearTimeout(timeout);
    if (exitCode === undefined) {
      browser.kill();
      await Promise.race([browserExit.catch(() => undefined), new Promise((resolve) => setTimeout(resolve, 5000))]);
    }
  }
} finally {
  await new Promise((resolve) => server.close(resolve));
  try {
    await rm(chromeProfile, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  } catch (error) {
    console.warn(`Could not remove the temporary Chrome profile: ${error}`);
  }
}
