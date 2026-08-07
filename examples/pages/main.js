const MIB = 1024 * 1024;
const memoryFields = [
  ["decoderRowsBytes", "Decoder rows"],
  ["decoderStagingBytes", "Decoder staging"],
  ["normalizedRowBytes", "Normalized row"],
  ["horizontalAccumulatorBytes", "Horizontal accumulator"],
  ["verticalAccumulatorBytes", "Vertical accumulator"],
  ["sparseAccumulatorBytes", "Adam7 sparse accumulator"],
  ["outputRowBytes", "Output row"],
  ["outputRgbaBytes", "Full RGBA output"],
  ["encoderStateBytes", "Encoder state"],
  ["encodedOutputBytes", "Encoded output / chunk buffer"],
];

const byId = (id) => document.getElementById(id);
const workspace = byId("workspace");
const status = byId("status");
const statusText = byId("status-text");
const form = byId("settings-form");
const fileInput = byId("file-input");
const sampleButton = byId("sample-button");
const runButton = byId("run-button");
const dropZone = byId("drop-zone");
const resultState = byId("result-state");
const preview = byId("preview");
const previewImage = byId("preview-image");
const previewCanvas = byId("preview-canvas");
const downloadLink = byId("download-link");
const errorDetail = byId("error-detail");

let worker;
let ready = false;
let busy = false;
let currentInput;
let currentName = "input.png";
let currentRunId = 0;
let currentInspectId = 0;
let nextRequestId = 0;
let currentPlan;
let previewUrl;

function formatBytes(bytes) {
  if (!Number.isFinite(bytes)) return "—";
  if (bytes < 1024) return `${bytes.toLocaleString()} B`;
  if (bytes < MIB) return `${(bytes / 1024).toFixed(2)} KiB`;
  return `${(bytes / MIB).toFixed(2)} MiB`;
}

function finiteInteger(id) {
  const input = byId(id);
  const value = input.valueAsNumber;
  if (!Number.isSafeInteger(value) || value <= 0) {
    input.setCustomValidity("Enter a positive safe integer.");
    throw new Error(`${input.labels?.[0]?.textContent ?? id} must be a positive integer.`);
  }
  input.setCustomValidity("");
  return value;
}

function selectedValue(name) {
  return form.elements[name].value;
}

function hexToRgb(value) {
  return [1, 3, 5].map((offset) => Number.parseInt(value.slice(offset, offset + 2), 16));
}

function collectOptions() {
  const memoryMiB = finiteInteger("max-memory");
  const output = selectedValue("output");
  const options = {
    maxWidth: finiteInteger("max-width"),
    maxHeight: finiteInteger("max-height"),
    fit: selectedValue("fit"),
    filter: byId("resize-filter").value,
    allowUpscale: byId("allow-upscale").checked,
    output,
    maxInputBytes: finiteInteger("max-input-bytes"),
    maxInputWidth: finiteInteger("max-input-width"),
    maxInputHeight: finiteInteger("max-input-height"),
    maxInputPixels: finiteInteger("max-input-pixels"),
    maxOutputWidth: finiteInteger("max-output-width"),
    maxOutputHeight: finiteInteger("max-output-height"),
    maxOutputPixels: finiteInteger("max-output-pixels"),
    maxMemoryBytes: memoryMiB * MIB,
  };
  if (output === "png") {
    options.png = {
      color: byId("png-color").value,
      compression: byId("png-compression").value,
      filter: byId("png-filter").value,
    };
  } else if (output === "jpeg") {
    options.jpeg = {
      quality: finiteInteger("jpeg-quality"),
      background: hexToRgb(byId("jpeg-background").value),
      subsampling: byId("jpeg-subsampling").value,
    };
  }
  return options;
}

function setResultState(kind, label, message) {
  resultState.className = `result-state result-state--${kind}`;
  byId("result-mark").textContent = kind === "success" ? "✓" : kind === "failure" ? "×" : kind === "running" ? "◌" : "○";
  byId("result-label").textContent = label;
  byId("result-message").textContent = message;
}

function clearPreview() {
  if (previewUrl) URL.revokeObjectURL(previewUrl);
  previewUrl = undefined;
  preview.hidden = true;
  previewImage.hidden = true;
  previewImage.removeAttribute("src");
  previewCanvas.hidden = true;
  const context = previewCanvas.getContext("2d");
  context?.clearRect(0, 0, previewCanvas.width, previewCanvas.height);
  downloadLink.hidden = true;
  downloadLink.removeAttribute("href");
}

function clearResult() {
  clearPreview();
  currentPlan = undefined;
  byId("result-facts").hidden = true;
  byId("memory-details").hidden = true;
  byId("memory-note").hidden = true;
  errorDetail.hidden = true;
  errorDetail.textContent = "";
  for (const id of ["result-output", "result-bytes", "result-time", "result-planned", "result-limit", "wasm-before", "wasm-after", "wasm-growth"]) {
    byId(id).textContent = "—";
  }
}

function safeDownloadName(name, width, height, format) {
  const base = name.replace(/\.[^.]*$/, "").replace(/[^a-zA-Z0-9._-]+/g, "-").replace(/^-+|-+$/g, "") || "thumbnail";
  const extension = format === "jpeg" ? "jpg" : format;
  return `${base}-streamthumb-${width}x${height}.${extension}`;
}

function renderPlan(plan) {
  currentPlan = plan;
  byId("result-facts").hidden = false;
  byId("memory-details").hidden = false;
  byId("memory-note").hidden = false;
  byId("result-planned").textContent = formatBytes(plan.memory.totalBytes);
  byId("result-limit").textContent = formatBytes(plan.configuredMaxMemoryBytes);
  const list = byId("memory-list");
  list.replaceChildren();
  for (const [field, label] of memoryFields) {
    const row = document.createElement("div");
    const term = document.createElement("dt");
    const detail = document.createElement("dd");
    term.textContent = label;
    detail.textContent = formatBytes(plan.memory[field]);
    row.append(term, detail);
    list.append(row);
  }
  const total = document.createElement("div");
  total.innerHTML = "<dt>Total</dt>";
  const totalValue = document.createElement("dd");
  totalValue.textContent = formatBytes(plan.memory.totalBytes);
  total.append(totalValue);
  list.append(total);
}

function renderInput(name, metadata) {
  byId("input-facts").hidden = false;
  byId("rgba-note").hidden = false;
  byId("input-name").textContent = name;
  byId("input-bytes").textContent = formatBytes(metadata.encodedBytes);
  byId("input-dimensions").textContent = `${metadata.width} × ${metadata.height}`;
  byId("input-format").textContent = `${metadata.colorType}, ${metadata.bitDepth}-bit`;
  byId("input-interlace").textContent = metadata.interlaced ? "Adam7" : "Non-interlaced";
  byId("input-rgba").textContent = formatBytes(metadata.width * metadata.height * 4);
}

function updateBusy(nextBusy) {
  busy = nextBusy;
  runButton.disabled = !ready || !currentInput || busy;
  fileInput.disabled = !ready || busy;
  sampleButton.disabled = !ready || busy;
  workspace.setAttribute("aria-busy", String(!ready || busy));
}

function inspectInput(bytes, name) {
  currentInput = bytes;
  currentName = name;
  clearResult();
  setResultState("running", "INSPECTING", "Reading validated PNG metadata in WebAssembly…");
  const requestId = ++nextRequestId;
  currentInspectId = requestId;
  const transferred = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
  worker.postMessage({ type: "inspect", requestId, input: transferred }, [transferred]);
  updateBusy(true);
}

async function selectFile(file) {
  if (!file) return;
  try {
    inspectInput(new Uint8Array(await file.arrayBuffer()), file.name);
  } catch (error) {
    showFailure("input inspection", error);
  }
}

function showFailure(stage, error, required, limit) {
  updateBusy(false);
  clearPreview();
  const message = stage === "configured limit rejection"
    ? "Planned memory exceeds the configured limit."
    : `The ${stage} stage failed.`;
  setResultState("failure", "FAILED", message);
  errorDetail.hidden = false;
  const lines = [];
  if (Number.isFinite(required)) lines.push(`Required (planned): ${formatBytes(required)}`);
  if (Number.isFinite(limit)) lines.push(`Configured limit: ${formatBytes(limit)}`);
  if (error) lines.push(String(error));
  errorDetail.textContent = lines.join("\n");
}

function renderSuccess(data) {
  const { metadata, bytes, timings, wasm } = data;
  updateBusy(false);
  setResultState("success", "SUCCESS", "Thumbnail completed inside the WebAssembly worker.");
  byId("result-facts").hidden = false;
  byId("result-output").textContent = `${metadata.width} × ${metadata.height} · ${metadata.format.toUpperCase()}`;
  byId("result-bytes").textContent = formatBytes(metadata.bytesWritten);
  byId("result-time").textContent = `${timings.processingMs.toFixed(2)} ms`;
  byId("wasm-before").textContent = formatBytes(wasm.before);
  byId("wasm-after").textContent = formatBytes(wasm.after);
  byId("wasm-growth").textContent = formatBytes(wasm.growth);

  const outputBytes = new Uint8Array(bytes);
  preview.hidden = false;
  if (metadata.format === "rgba") {
    previewCanvas.width = metadata.width;
    previewCanvas.height = metadata.height;
    previewCanvas.getContext("2d").putImageData(new ImageData(new Uint8ClampedArray(outputBytes), metadata.width, metadata.height), 0, 0);
    previewCanvas.hidden = false;
    previewImage.hidden = true;
    downloadLink.hidden = true;
  } else {
    const blob = new Blob([outputBytes], { type: metadata.mimeType });
    previewUrl = URL.createObjectURL(blob);
    previewImage.src = previewUrl;
    previewImage.hidden = false;
    previewCanvas.hidden = true;
    downloadLink.href = previewUrl;
    downloadLink.download = safeDownloadName(currentName, metadata.width, metadata.height, metadata.format);
    downloadLink.hidden = false;
  }
}

function createWorker() {
  worker = new Worker(new URL("./worker.js", import.meta.url), { type: "module" });
  worker.addEventListener("message", ({ data }) => {
    if (data.type === "ready") {
      ready = true;
      status.dataset.ready = "true";
      statusText.textContent = "Choose a PNG or try the bundled sample";
      byId("version").textContent = `v${data.version}`;
      updateBusy(false);
      return;
    }
    if (data.type === "inspected") {
      if (data.requestId !== currentInspectId) return;
      renderInput(currentName, data.input);
      setResultState("idle", "READY", "Review the settings, then run the thumbnail.");
      updateBusy(false);
      return;
    }
    if (data.type === "planned") {
      if (data.requestId !== currentRunId) return;
      renderPlan(data.plan);
      setResultState("running", "RUNNING", "The plan fits the configured limit. Encoding now…");
      return;
    }
    if (data.type === "success") {
      if (data.requestId !== currentRunId) return;
      renderSuccess(data);
      return;
    }
    if (data.type === "failure") {
      if (data.requestId !== currentRunId && data.requestId !== currentInspectId && data.requestId !== 0) return;
      if (data.stage === "initialization") {
        ready = false;
        statusText.textContent = "WebAssembly failed to load — reload this page to retry";
      }
      if (data.plan) renderPlan(data.plan);
      showFailure(data.stage, data.error, data.required, data.limit);
    }
  });
  worker.addEventListener("error", (event) => {
    ready = false;
    statusText.textContent = "WebAssembly worker stopped — reload this page to retry";
    showFailure("worker", event.message);
  });
}

function syncPair(rangeId, numberId) {
  const range = byId(rangeId);
  const number = byId(numberId);
  range.addEventListener("input", () => { number.value = range.value; });
  number.addEventListener("input", () => {
    if (number.valueAsNumber >= Number(range.min) && number.valueAsNumber <= Number(range.max)) range.value = number.value;
  });
}

function updateOutputControls() {
  const output = selectedValue("output");
  byId("png-options").hidden = output !== "png";
  byId("jpeg-options").hidden = output !== "jpeg";
}

function updateMemoryLabel() {
  const mib = byId("max-memory").valueAsNumber;
  byId("max-memory-bytes").textContent = Number.isFinite(mib) ? `${(mib * MIB).toLocaleString()} bytes` : "Invalid";
}

syncPair("max-width-range", "max-width");
syncPair("max-height-range", "max-height");
syncPair("jpeg-quality-range", "jpeg-quality");
syncPair("max-memory-range", "max-memory");

form.addEventListener("change", (event) => {
  if (event.target.name === "output") updateOutputControls();
  if (event.target.id === "jpeg-background") byId("jpeg-background-value").textContent = hexToRgb(event.target.value).join(", ");
  if (event.target.id === "max-memory") updateMemoryLabel();
});
byId("max-memory").addEventListener("input", updateMemoryLabel);
byId("max-memory-range").addEventListener("input", updateMemoryLabel);
document.querySelectorAll("[data-memory]").forEach((button) => button.addEventListener("click", () => {
  byId("max-memory").value = button.dataset.memory;
  byId("max-memory-range").value = button.dataset.memory;
  updateMemoryLabel();
}));

form.addEventListener("submit", (event) => {
  event.preventDefault();
  if (!currentInput || busy || !form.reportValidity()) return;
  try {
    const options = collectOptions();
    clearResult();
    setResultState("running", "PLANNING", "Computing the conservative Rust working-memory plan…");
    updateBusy(true);
    const requestId = ++nextRequestId;
    currentRunId = requestId;
    const transferred = currentInput.buffer.slice(currentInput.byteOffset, currentInput.byteOffset + currentInput.byteLength);
    worker.postMessage({ type: "run", requestId, input: transferred, fileName: currentName, options }, [transferred]);
  } catch (error) {
    showFailure("settings validation", error);
  }
});

byId("reset-button").addEventListener("click", () => {
  form.reset();
  for (const [range, number] of [["max-width-range", "max-width"], ["max-height-range", "max-height"], ["jpeg-quality-range", "jpeg-quality"], ["max-memory-range", "max-memory"]]) byId(range).value = byId(number).value;
  updateOutputControls();
  updateMemoryLabel();
  byId("jpeg-background-value").textContent = "255, 255, 255";
});

fileInput.addEventListener("change", () => selectFile(fileInput.files?.[0]));
for (const type of ["dragenter", "dragover"]) dropZone.addEventListener(type, (event) => { event.preventDefault(); if (ready && !busy) dropZone.dataset.dragging = "true"; });
for (const type of ["dragleave", "drop"]) dropZone.addEventListener(type, (event) => { event.preventDefault(); dropZone.dataset.dragging = "false"; });
dropZone.addEventListener("drop", (event) => { if (ready && !busy) selectFile(event.dataTransfer?.files?.[0]); });

sampleButton.addEventListener("click", async () => {
  try {
    sampleButton.disabled = true;
    const manifest = await fetch("./sample-manifest.json").then((response) => response.json());
    const sample = manifest.samples[0];
    const response = await fetch(sample.path);
    if (!response.ok) throw new Error(`Sample request failed with HTTP ${response.status}.`);
    inspectInput(new Uint8Array(await response.arrayBuffer()), sample.path.split("/").at(-1));
  } catch (error) {
    showFailure("sample loading", error);
  }
});

window.addEventListener("beforeunload", () => { clearPreview(); worker?.terminate(); });

fetch("./build-metadata.json")
  .then((response) => response.json())
  .then(({ revision }) => {
    if (/^[0-9a-f]{40}$/.test(revision)) {
      const link = byId("revision-link");
      link.textContent = revision.slice(0, 8);
      link.href = `https://github.com/piyohogeo/streamthumb/commit/${revision}`;
    }
  })
  .catch(() => undefined);

updateOutputControls();
updateMemoryLabel();
createWorker();
