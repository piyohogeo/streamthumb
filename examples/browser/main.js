const worker = new Worker(new URL("./worker.js", import.meta.url), {
  type: "module",
});

const fileInput = document.querySelector("#file");
const thumbnail = document.querySelector("#thumbnail");
const status = document.querySelector("#status");
fileInput.disabled = true;

let resolveWorkerReady;
let rejectWorkerReady;
const workerReady = new Promise((resolve, reject) => {
  resolveWorkerReady = resolve;
  rejectWorkerReady = reject;
});

fileInput.addEventListener("change", async () => {
  const [file] = fileInput.files;
  if (!file) return;
  status.textContent = "Generating thumbnail...";
  try {
    await workerReady;
    const bytes = await file.arrayBuffer();
    worker.postMessage(bytes, [bytes]);
  } catch (error) {
    status.textContent = String(error);
  }
});

worker.addEventListener("message", ({ data }) => {
  if (data.initializing) {
    status.textContent = "Loading WebAssembly...";
    return;
  }
  if (data.ready) {
    fileInput.disabled = false;
    resolveWorkerReady();
    return;
  }
  if (data.error) {
    rejectWorkerReady(new Error(data.error));
    status.textContent = data.error;
    return;
  }
  const blob = new Blob([data.bytes], { type: data.mimeType });
  thumbnail.src = URL.createObjectURL(blob);
  status.textContent = `${data.width} x ${data.height}`;
});

worker.addEventListener(
  "error",
  (event) => {
    rejectWorkerReady(new Error(event.message));
    status.textContent = event.message;
  },
  { once: true },
);
