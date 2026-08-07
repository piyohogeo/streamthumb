import init, {
  streamthumbVersion,
  thumbnailPng,
} from "@streamthumb/wasm";

function assertThumbnail(result: ReturnType<typeof thumbnailPng>) {
  const signature = [137, 80, 78, 71, 13, 10, 26, 10];
  if (result.width !== 32 || result.height !== 32) {
    throw new Error(`unexpected dimensions ${result.width}x${result.height}`);
  }
  if (
    result.mimeType !== "image/png"
    || !signature.every((value, index) => result.bytes[index] === value)
  ) {
    throw new Error("thumbnail output is not a PNG");
  }
}

const packageModule = import.meta.resolve("@streamthumb/wasm");
const wasm = await Deno.readFile(
  new URL("streamthumb_wasm_bg.wasm", packageModule),
);
await init({ module_or_path: wasm });

const input = await Deno.readFile(new URL("fixture.png", import.meta.url));
const result = thumbnailPng(input, {
  maxWidth: 32,
  maxHeight: 32,
  output: "png",
  maxMemoryBytes: 32 * 1024 * 1024,
});
assertThumbnail(result);
console.log(
  `PASS: Deno loaded @streamthumb/wasm ${streamthumbVersion()} and created a 32x32 PNG`,
);
