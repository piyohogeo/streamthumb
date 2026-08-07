const status = document.querySelector("#status");
const BUILD_REVISION = "__STREAMTHUMB_REVISION__";

function versionedUrl(relativePath) {
  const url = new URL(relativePath, import.meta.url);
  url.searchParams.set("v", BUILD_REVISION);
  return url;
}

const worker = new Worker(versionedUrl("./worker.js"), { type: "module" });
let nextRequestId = 1;

function waitForReady() {
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error("Worker initialization timed out.")), 15_000);
    const onMessage = ({ data }) => {
      if (data.type === "ready") {
        clearTimeout(timeout);
        worker.removeEventListener("message", onMessage);
        resolve(data);
      } else if (data.type === "failure" && data.stage === "initialization") {
        clearTimeout(timeout);
        worker.removeEventListener("message", onMessage);
        reject(new Error(data.error));
      }
    };
    worker.addEventListener("message", onMessage);
  });
}

function run(input, options) {
  return new Promise((resolve, reject) => {
    const requestId = nextRequestId++;
    let plan;
    const timeout = setTimeout(() => reject(new Error(`Run ${requestId} timed out.`)), 15_000);
    const onMessage = ({ data }) => {
      if (data.requestId !== requestId) return;
      if (data.type === "planned") {
        plan = data.plan;
        return;
      }
      if (data.type === "success" || data.type === "failure") {
        clearTimeout(timeout);
        worker.removeEventListener("message", onMessage);
        resolve({ ...data, plan: data.plan ?? plan });
      }
    };
    worker.addEventListener("message", onMessage);
    worker.addEventListener("error", (event) => reject(new Error(event.message)), { once: true });
    worker.postMessage({ type: "run", requestId, input, fileName: "sample.png", options });
  });
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

async function waitFor(condition, message, timeoutMs = 15_000) {
  const started = performance.now();
  while (!condition()) {
    if (performance.now() - started > timeoutMs) throw new Error(message);
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
}

async function verifyPageUi() {
  const frame = document.createElement("iframe");
  frame.src = versionedUrl("./index.html");
  frame.title = "Pages UI under test";
  document.body.append(frame);
  await new Promise((resolve, reject) => {
    frame.addEventListener("load", resolve, { once: true });
    setTimeout(() => reject(new Error("Pages UI frame did not load.")), 10_000);
  });
  const doc = frame.contentDocument;
  const id = (value) => doc.getElementById(value);
  await waitFor(() => id("status").dataset.ready === "true", "Pages UI worker did not become ready.");
  assert(doc.querySelector(".hero h1").textContent === "streamthumb / WebAssembly demo", "The compact demo title was not rendered.");
  assert(id("drop-zone").textContent.includes("The image is never uploaded to a server."), "The drop zone omitted the privacy note.");
  assert(id("max-memory").value === "4096", "The UI memory limit did not default to 4096 KiB.");
  id("sample-button").click();
  await waitFor(() => id("result-label").textContent === "READY", "Bundled sample was not inspected.");
  assert(id("input-dimensions").textContent === "2048 × 2048", "The bundled sample did not expose large dimensions.");

  id("run-button").click();
  await waitFor(() => id("result-label").textContent === "SUCCESS", "Default PNG UI run did not succeed.");
  assert(!id("preview-image").hidden && id("preview-image").src.startsWith("blob:"), "PNG preview was not rendered from a Blob URL.");
  assert(id("result-planned").textContent !== "—" && id("wasm-after").textContent !== "—", "Memory diagnostics were not rendered.");

  doc.querySelector('input[name="output"][value="jpeg"]').click();
  id("run-button").click();
  await waitFor(() => id("result-label").textContent === "SUCCESS" && id("result-output").textContent.includes("JPEG"), "JPEG UI run did not succeed.");

  doc.querySelector('input[name="output"][value="rgba"]').click();
  id("run-button").click();
  await waitFor(() => id("result-label").textContent === "SUCCESS" && !id("preview-canvas").hidden, "RGBA canvas preview was not rendered.");

  id("allow-upscale").checked = true;
  id("max-width").value = "512";
  id("max-height").value = "512";
  doc.querySelector('[data-memory-kib="128"]').click();
  assert(id("max-memory").value === "128", "The 128 KiB preset did not update the memory limit.");
  assert(id("max-memory-bytes").textContent === "131,072 bytes", "The 128 KiB preset used stale MiB JavaScript.");
  id("run-button").click();
  await waitFor(() => id("result-label").textContent === "FAILED", "The UI did not show the planned-memory rejection.");
  assert(id("error-detail").textContent.includes("Required (planned)") && id("error-detail").textContent.includes("Configured limit"), "The UI omitted typed memory-limit values.");

  doc.querySelector('[data-memory-kib="4096"]').click();
  assert(id("max-memory").value === "4096", "The 4 MiB preset did not restore the memory limit.");
  id("run-button").click();
  await waitFor(() => id("result-label").textContent === "SUCCESS", "The UI did not recover after restoring the limit.");
  frame.remove();
}

async function assertImage(bytes, mimeType, width, height) {
  const bitmap = await createImageBitmap(new Blob([bytes], { type: mimeType }));
  try {
    assert(bitmap.width === width && bitmap.height === height, `Decoded ${bitmap.width}x${bitmap.height}; expected ${width}x${height}.`);
  } finally {
    bitmap.close();
  }
}

async function report(result, message) {
  status.textContent = message;
  await fetch("/pages-result", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ result, message }),
  });
}

try {
  const ready = await waitForReady();
  assert(/^\d+\.\d+\.\d+/.test(ready.version), "Worker did not report a semantic version.");
  const input = await fetch(versionedUrl("./samples/large-rgba.png")).then((response) => response.blob());

  const png = await run(input, { maxWidth: 16, maxHeight: 16, output: "png", maxMemoryBytes: 4 * 1024 * 1024 });
  assert(png.type === "success", `PNG run failed: ${png.error}`);
  assert(png.plan.withinMemoryLimit, "PNG plan unexpectedly exceeded the memory limit.");
  assert(Number.isFinite(png.plan.memory.totalBytes) && png.plan.memory.totalBytes > 0, "PNG plan did not contain finite memory.");
  assert(Number.isFinite(png.timings.processingMs) && png.timings.processingMs >= 0, "PNG timing was invalid.");
  assert(png.inputReads.calls > 0 && png.inputReads.bytes > 0, "Seekable input did not issue range reads.");
  assert(png.inputReads.largestReadBytes <= 8 * 1024, "Seekable input exceeded the bounded reader capacity.");
  assert(Number.isFinite(png.wasm.before) && Number.isFinite(png.wasm.after) && Number.isFinite(png.wasm.growth), "WASM memory observations were invalid.");
  await assertImage(new Uint8Array(png.bytes), png.metadata.mimeType, 16, 16);

  const rejected = await run(input, { maxWidth: 16, maxHeight: 16, output: "png", maxMemoryBytes: png.plan.memory.totalBytes - 1 });
  assert(rejected.type === "failure" && rejected.stage === "configured limit rejection", "Low memory did not produce a typed rejection.");
  assert(rejected.required === png.plan.memory.totalBytes, "Typed rejection reported the wrong required memory.");

  const jpeg = await run(input, { maxWidth: 16, maxHeight: 16, output: "jpeg", maxMemoryBytes: 4 * 1024 * 1024 });
  assert(jpeg.type === "success", `JPEG run failed: ${jpeg.error}`);
  await assertImage(new Uint8Array(jpeg.bytes), jpeg.metadata.mimeType, 16, 16);

  const rgba = await run(input, { maxWidth: 16, maxHeight: 16, output: "rgba", maxMemoryBytes: 4 * 1024 * 1024 });
  assert(rgba.type === "success", `RGBA run failed: ${rgba.error}`);
  assert(rgba.bytes.byteLength === 16 * 16 * 4, "RGBA output length was incorrect.");

  const recovered = await run(input, { maxWidth: 8, maxHeight: 8, output: "png", maxMemoryBytes: 4 * 1024 * 1024 });
  assert(recovered.type === "success", "A valid run did not recover after the limit rejection.");
  await verifyPageUi();
  await report("pass", "PASS: Pages UI and seekable Blob worker verified bounded reads, PNG, JPEG, RGBA, planning, limit rejection, previews, and recovery");
} catch (error) {
  await report("fail", String(error));
} finally {
  worker.terminate();
}
