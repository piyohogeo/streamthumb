const status = document.querySelector("#status");

function finish(result, message) {
  document.documentElement.dataset.result = result;
  status.textContent = message;
}

function isPng(bytes) {
  const signature = [137, 80, 78, 71, 13, 10, 26, 10];
  return signature.every((value, index) => bytes[index] === value);
}

async function run() {
  const response = await fetch(
    "../../fuzz/corpus/thumbnail_png/pngsuite_basn6a08.png",
  );
  if (!response.ok) {
    throw new Error(`fixture request failed with HTTP ${response.status}`);
  }

  const input = await response.arrayBuffer();
  const worker = new Worker("./worker.js", { type: "module" });
  const timeout = setTimeout(() => {
    worker.terminate();
    finish("fail", "FAIL: module worker timed out");
  }, 15000);

  worker.addEventListener(
    "message",
    async ({ data }) => {
      try {
        if (data.initializing) {
          status.textContent = "Initializing WebAssembly...";
          return;
        }
        if (data.ready) {
          worker.postMessage(input, [input]);
          return;
        }

        clearTimeout(timeout);
        worker.terminate();
        if (data.error) {
          throw new Error(data.error);
        }

        const bytes = new Uint8Array(data.bytes);
        if (data.width !== 32 || data.height !== 32) {
          throw new Error(`unexpected dimensions ${data.width}x${data.height}`);
        }
        if (data.mimeType !== "image/png" || !isPng(bytes)) {
          throw new Error("worker output is not a PNG");
        }

        const bitmap = await createImageBitmap(
          new Blob([bytes], { type: data.mimeType }),
        );
        const decodedDimensions = `${bitmap.width}x${bitmap.height}`;
        bitmap.close();
        if (decodedDimensions !== "32x32") {
          throw new Error(`decoded PNG is ${decodedDimensions}`);
        }

        finish("pass", "PASS: module worker created and decoded a 32x32 PNG");
      } catch (error) {
        finish("fail", `FAIL: ${error}`);
      }
    },
  );

  worker.addEventListener(
    "error",
    (event) => {
      clearTimeout(timeout);
      worker.terminate();
      finish("fail", `FAIL: ${event.message}`);
    },
    { once: true },
  );
}

run().catch((error) => finish("fail", `FAIL: ${error}`));
