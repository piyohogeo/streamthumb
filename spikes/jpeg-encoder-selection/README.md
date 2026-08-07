# JPEG encoder selection spike

This isolated workspace compares JPEG encoders without adding either crate to
Streamthumb's production dependency graph.

The two programs encode the same deterministic 512 x 512 RGB image at quality
85 with 4:2:0 chroma subsampling. The `mozjpeg-rs` program submits one row at a
time to its streaming API. The `jpeg-encoder` program must retain the complete
RGB image because its encoder drives the input through a whole-image API.

Run native measurements from this directory:

```powershell
cargo run --release -p mozjpeg-rs-spike --bin mozjpeg-rs-bench -- 100
cargo run --release -p jpeg-encoder-spike --bin jpeg-encoder-bench -- 100
```

Build the WebAssembly artifacts with:

```powershell
cargo build --release --target wasm32-unknown-unknown
```

The exported `encode_512` functions keep each encoder reachable so the `.wasm`
file sizes represent the encoder code pulled into a minimal module. These sizes
are comparative spike measurements, not exact deltas for the final Streamthumb
WASM package.

On Windows, run the complete timing, peak-working-set, test, and WASM-size
measurement with:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\measure.ps1
```
