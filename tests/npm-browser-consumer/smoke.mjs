import init, {
  streamthumbVersion,
  thumbnailPng,
} from "@streamthumb/wasm";

const status = document.querySelector("#status");

function finish(result, message) {
  document.documentElement.dataset.result = result;
  status.textContent = message;
}

function hasPngSignature(bytes) {
  const signature = [137, 80, 78, 71, 13, 10, 26, 10];
  return signature.every((value, index) => bytes[index] === value);
}

try {
  await init();
  const response = await fetch("/fixture.png");
  if (!response.ok) {
    throw new Error(`fixture request failed with HTTP ${response.status}`);
  }

  const result = thumbnailPng(new Uint8Array(await response.arrayBuffer()), {
    maxWidth: 32,
    maxHeight: 32,
    output: "png",
    maxMemoryBytes: 32 * 1024 * 1024,
  });
  const bytes = result.bytes;

  if (result.width !== 32 || result.height !== 32) {
    throw new Error(`unexpected dimensions ${result.width}x${result.height}`);
  }
  if (result.mimeType !== "image/png" || !hasPngSignature(bytes)) {
    throw new Error("thumbnail output is not a PNG");
  }

  const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
  const decodedDimensions = `${bitmap.width}x${bitmap.height}`;
  bitmap.close();
  if (decodedDimensions !== "32x32") {
    throw new Error(`decoded PNG is ${decodedDimensions}`);
  }

  finish(
    "pass",
    `PASS: @streamthumb/wasm ${streamthumbVersion()} created and decoded a 32x32 PNG`,
  );
} catch (error) {
  finish("fail", `FAIL: ${error instanceof Error ? error.stack : error}`);
}
