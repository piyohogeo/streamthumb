import { writeFile } from "node:fs/promises";
import { deflateSync } from "node:zlib";

const WIDTH = 2048;
const HEIGHT = 2048;
const TILE_SIZE = 128;
const PNG_SIGNATURE = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
const COLORS = [
  [233, 87, 43, 255],
  [246, 189, 88, 224],
  [31, 90, 114, 255],
  [23, 107, 75, 208],
];

const crcTable = new Uint32Array(256);
for (let index = 0; index < crcTable.length; index += 1) {
  let value = index;
  for (let bit = 0; bit < 8; bit += 1) {
    value = (value & 1) !== 0 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
  }
  crcTable[index] = value >>> 0;
}

function crc32(bytes) {
  let value = 0xffffffff;
  for (const byte of bytes) value = crcTable[(value ^ byte) & 0xff] ^ (value >>> 8);
  return (value ^ 0xffffffff) >>> 0;
}

function pngChunk(type, data) {
  const typeBytes = Buffer.from(type, "ascii");
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const checksum = Buffer.alloc(4);
  checksum.writeUInt32BE(crc32(Buffer.concat([typeBytes, data])));
  return Buffer.concat([length, typeBytes, data, checksum]);
}

export async function generatePagesSample(outputPath) {
  const stride = 1 + WIDTH * 4;
  const pixels = Buffer.allocUnsafe(stride * HEIGHT);
  for (let y = 0; y < HEIGHT; y += 1) {
    const row = y * stride;
    pixels[row] = 0;
    const tileY = Math.floor(y / TILE_SIZE);
    for (let x = 0; x < WIDTH; x += 1) {
      const tileX = Math.floor(x / TILE_SIZE);
      const color = COLORS[(tileX + tileY) % COLORS.length];
      const offset = row + 1 + x * 4;
      pixels[offset] = color[0];
      pixels[offset + 1] = color[1];
      pixels[offset + 2] = color[2];
      pixels[offset + 3] = color[3];
    }
  }

  const header = Buffer.alloc(13);
  header.writeUInt32BE(WIDTH, 0);
  header.writeUInt32BE(HEIGHT, 4);
  header[8] = 8;
  header[9] = 6;
  const png = Buffer.concat([
    PNG_SIGNATURE,
    pngChunk("IHDR", header),
    pngChunk("IDAT", deflateSync(pixels, { level: 9 })),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
  await writeFile(outputPath, png);
  return { width: WIDTH, height: HEIGHT, encodedBytes: png.length };
}
