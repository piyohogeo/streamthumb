const worker = new Worker(new URL("./worker.js", import.meta.url), {
  type: "module",
});

const fileInput = document.querySelector("#file");
const thumbnail = document.querySelector("#thumbnail");
const status = document.querySelector("#status");

fileInput.addEventListener("change", async () => {
  const [file] = fileInput.files;
  if (!file) return;
  status.textContent = "Generating thumbnail...";
  const bytes = await file.arrayBuffer();
  worker.postMessage(bytes, [bytes]);
});

worker.addEventListener("message", ({ data }) => {
  if (data.error) {
    status.textContent = data.error;
    return;
  }
  const blob = new Blob([data.bytes], { type: data.mimeType });
  thumbnail.src = URL.createObjectURL(blob);
  status.textContent = `${data.width} x ${data.height}`;
});

