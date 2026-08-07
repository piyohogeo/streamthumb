import init, { thumbnailPng } from "@streamthumb/wasm";

let initialized;

export default {
  async fetch(request) {
    if (request.method !== "POST") {
      return new Response("POST an encoded PNG request body.\n", { status: 405 });
    }

    initialized ??= init();
    await initialized;

    try {
      const input = new Uint8Array(await request.arrayBuffer());
      const result = thumbnailPng(input, {
        maxWidth: 512,
        maxHeight: 512,
        output: "png",
        png: { color: "auto", compression: "balanced", filter: "default" },
        maxInputBytes: 64 * 1024 * 1024,
        maxInputPixels: 500_000_000,
        maxMemoryBytes: 32 * 1024 * 1024,
      });
      return new Response(result.bytes, {
        headers: {
          "content-type": result.mimeType,
          "x-thumbnail-width": String(result.width),
          "x-thumbnail-height": String(result.height),
        },
      });
    } catch (error) {
      return new Response(String(error), { status: 400 });
    }
  },
};
