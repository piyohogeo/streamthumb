import init, {
  thumbnailPngFromSeekable,
  thumbnailPngFromSeekableToChunks,
} from "@streamthumb/wasm";

const initialized = init();

self.onmessage = async ({ data: file }) => {
  try {
    await initialized;
    if (!(file instanceof Blob)) {
      throw new TypeError("seekable worker input must be a Blob");
    }
    const reader = new FileReaderSync();
    const readAt = (offset, length) => new Uint8Array(
      reader.readAsArrayBuffer(file.slice(offset, offset + length)),
    );
    const options = {
      maxWidth: 32,
      maxHeight: 32,
      output: "png",
      png: { color: "auto", compression: "fast", filter: "adaptive" },
    };

    const buffered = thumbnailPngFromSeekable(file.size, readAt, options);
    const bytes = buffered.bytes;
    const chunks = [];
    const chunked = thumbnailPngFromSeekableToChunks(
      file.size,
      readAt,
      (chunk) => chunks.push(chunk),
      options,
    );
    const chunkBytes = new Uint8Array(chunked.bytesWritten);
    let offset = 0;
    for (const chunk of chunks) {
      chunkBytes.set(chunk, offset);
      offset += chunk.length;
    }
    const metadata = {
      width: buffered.width,
      height: buffered.height,
      format: buffered.format,
      chunkCount: chunked.chunkCount,
    };
    buffered.free();
    chunked.free();
    self.postMessage({ ok: true, bytes, chunkBytes, metadata }, [
      bytes.buffer,
      chunkBytes.buffer,
    ]);
  } catch (error) {
    self.postMessage({
      ok: false,
      error: error instanceof Error ? error.stack : String(error),
    });
  }
};
