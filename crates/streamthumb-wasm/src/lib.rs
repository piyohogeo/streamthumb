//! Runtime-neutral WebAssembly bindings for `streamthumb`.

use std::{
    cell::RefCell,
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
    rc::Rc,
};

use js_sys::{Function, Object, Reflect, Uint8Array};
use streamthumb_core::{Filter, Fit, OutputFormat, ThumbnailOptions};
use streamthumb_png::{
    JpegOptions, JpegSubsampling, PngColorMode, PngCompression, PngFilter, PngInputColorType,
    PngOptions, PngThumbnailPlan, ThumbnailOutput, preflight_thumbnail_png,
    preflight_thumbnail_png_to_writer_with_buffer,
    thumbnail_jpeg_from_reader_to_writer_with_options_and_buffer, thumbnail_png_from_reader,
    thumbnail_png_from_reader_to_writer_with_encoder_options_and_buffer,
    thumbnail_png_from_reader_with_encoder_options, thumbnail_png_from_reader_with_jpeg_options,
};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(typescript_custom_section)]
const THUMBNAIL_TYPES: &str = r#"
/** Fit modes supported by the bounded thumbnail pipeline. */
export type ThumbnailFit = "contain" | "cover";

/** Resize filters supported by the bounded thumbnail pipeline. */
export type ThumbnailFilter = "area";

/** Output representations supported by the WebAssembly API. */
export type ThumbnailOutputFormat = "png" | "jpeg" | "rgba";

/** PNG color representations supported by the streaming encoder. */
export type PngColorMode = "auto" | "rgba8" | "rgb8" | "grayscale-alpha8" | "grayscale8";

/** PNG compression speed and size tradeoffs. */
export type PngCompression = "none" | "fastest" | "fast" | "balanced" | "high";

/** PNG scanline filter strategies. */
export type PngFilter = "default" | "none" | "sub" | "up" | "average" | "paeth" | "adaptive" | "min-entropy";

/** Settings used only when output is "png". */
export interface PngOptions {
    color?: PngColorMode;
    compression?: PngCompression;
    filter?: PngFilter;
}

/** JPEG chroma-subsampling modes. */
export type JpegSubsampling = "420" | "422" | "444";

/** Settings used only when output is "jpeg". */
export interface JpegOptions {
    quality?: number;
    background?: [number, number, number];
    subsampling?: JpegSubsampling;
}

/** Options and resource limits for thumbnail generation. */
export interface ThumbnailOptions {
    maxWidth?: number;
    maxHeight?: number;
    fit?: ThumbnailFit;
    filter?: ThumbnailFilter;
    allowUpscale?: boolean;
    output?: ThumbnailOutputFormat;
    png?: PngOptions;
    jpeg?: JpegOptions;
    maxInputBytes?: number;
    maxInputWidth?: number;
    maxInputHeight?: number;
    maxInputPixels?: number;
    maxOutputWidth?: number;
    maxOutputHeight?: number;
    maxOutputPixels?: number;
    maxMemoryBytes?: number;
}

/** Selects the memory model used to plan output delivery. */
export type OutputDelivery = "buffered" | "chunks";

/** PNG metadata validated without decoding image pixels. */
export interface ThumbnailPlanInput {
    width: number;
    height: number;
    encodedBytes: number;
    colorType: "grayscale" | "rgb" | "indexed" | "grayscale-alpha" | "rgba";
    bitDepth: number;
    interlaced: boolean;
}

/** Planned output geometry and format. */
export interface ThumbnailPlanOutput {
    width: number;
    height: number;
    format: ThumbnailOutputFormat;
}

/** Conservative Rust-owned working-memory estimate. */
export interface ThumbnailMemoryPlan {
    decoderRowsBytes: number;
    decoderStagingBytes: number;
    normalizedRowBytes: number;
    horizontalAccumulatorBytes: number;
    verticalAccumulatorBytes: number;
    sparseAccumulatorBytes: number;
    outputRowBytes: number;
    outputRgbaBytes: number;
    encoderStateBytes: number;
    encodedOutputBytes: number;
    totalBytes: number;
}

/** Plain-object result returned by planThumbnailPng. */
export interface ThumbnailPlan {
    input: ThumbnailPlanInput;
    output: ThumbnailPlanOutput;
    memory: ThumbnailMemoryPlan;
    configuredMaxMemoryBytes: number;
    withinMemoryLimit: boolean;
}

/** Inspects and plans a thumbnail without decoding image pixels. */
export function planThumbnailPng(
    input: Uint8Array,
    options?: ThumbnailOptions | null,
    delivery?: OutputDelivery | null,
): ThumbnailPlan;

/** Creates a bounded PNG, JPEG, or RGBA thumbnail from encoded PNG bytes. */
export function thumbnailPng(
    input: Uint8Array,
    options?: ThumbnailOptions | null,
): ThumbnailResult;

/** Synchronously reads an exact encoded-input range. */
export type SeekableReadAt = (offset: number, length: number) => Uint8Array;

/** Creates a thumbnail from bounded synchronous range reads. */
export function thumbnailPngFromSeekable(
    inputLength: number,
    readAt: SeekableReadAt,
    options?: ThumbnailOptions | null,
): ThumbnailResult;

/** Receives one owned encoded-output chunk. */
export type ThumbnailChunkCallback = (chunk: Uint8Array) => void;

/** Creates encoded PNG or JPEG output without retaining the complete result. */
export function thumbnailPngToChunks(
    input: Uint8Array,
    onChunk: ThumbnailChunkCallback,
    options?: ThumbnailOptions | null,
): ChunkedThumbnailResult;

/** Creates chunked encoded output from bounded synchronous range reads. */
export function thumbnailPngFromSeekableToChunks(
    inputLength: number,
    readAt: SeekableReadAt,
    onChunk: ThumbnailChunkCallback,
    options?: ThumbnailOptions | null,
): ChunkedThumbnailResult;
"#;

const OUTPUT_CHUNK_BYTES: usize = 64 * 1024;

/// Returns the package version for bootstrap and packaging checks.
#[wasm_bindgen(js_name = streamthumbVersion)]
pub fn streamthumb_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

/// Returns the current WebAssembly linear-memory size in bytes.
///
/// WebAssembly memory only grows, so sampling this after a benchmark operation
/// reports the process-local linear-memory high-water mark.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(js_name = wasmMemoryBytes)]
pub fn wasm_memory_bytes() -> u32 {
    let memory = wasm_bindgen::memory().unchecked_into::<js_sys::WebAssembly::Memory>();
    let buffer = memory.buffer().unchecked_into::<js_sys::ArrayBuffer>();
    buffer.byte_length()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputDelivery {
    Buffered,
    Chunks,
}

struct JsSeekableReader {
    length: u64,
    position: u64,
    read_at: Function,
    callback_error: Rc<RefCell<Option<JsValue>>>,
}

enum WasmInput<'a> {
    Slice(Cursor<&'a [u8]>),
    Seekable(JsSeekableReader),
}

impl Read for WasmInput<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Slice(reader) => reader.read(output),
            Self::Seekable(reader) => reader.read(output),
        }
    }
}

impl Seek for WasmInput<'_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        match self {
            Self::Slice(reader) => reader.seek(position),
            Self::Seekable(reader) => reader.seek(position),
        }
    }
}

impl JsSeekableReader {
    fn new(length: u64, read_at: Function, callback_error: Rc<RefCell<Option<JsValue>>>) -> Self {
        Self {
            length,
            position: 0,
            read_at,
            callback_error,
        }
    }

    fn callback_failure(&self, error: JsValue) -> io::Error {
        *self.callback_error.borrow_mut() = Some(error);
        io::Error::other("JavaScript input callback failed")
    }
}

impl Read for JsSeekableReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.callback_error.borrow().is_some() {
            return Err(io::Error::other(
                "JavaScript input callback previously failed",
            ));
        }
        if output.is_empty() || self.position == self.length {
            return Ok(0);
        }
        let remaining = self.length.checked_sub(self.position).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "reader position exceeds input length",
            )
        })?;
        let requested = usize::try_from(remaining.min(output.len() as u64)).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "requested read cannot be represented",
            )
        })?;
        let requested_u32 = u32::try_from(requested).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "requested read exceeds the adapter range",
            )
        })?;
        let value = self
            .read_at
            .call2(
                &JsValue::UNDEFINED,
                &JsValue::from_f64(self.position as f64),
                &JsValue::from_f64(requested as f64),
            )
            .map_err(|error| self.callback_failure(error))?;
        if !value.is_instance_of::<Uint8Array>() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "readAt must return a Uint8Array",
            ));
        }
        let bytes = value.unchecked_into::<Uint8Array>();
        if bytes.length() != requested_u32 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "readAt returned a different byte length than requested",
            ));
        }
        bytes.copy_to(&mut output[..requested]);
        self.position = self.position.checked_add(requested as u64).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "reader position overflow")
        })?;
        Ok(requested)
    }
}

impl Seek for JsSeekableReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let next = match position {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.length) + i128::from(delta),
        };
        if !(0..=i128::from(self.length)).contains(&next) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek position is outside the encoded input",
            ));
        }
        self.position = u64::try_from(next).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek position cannot be represented",
            )
        })?;
        Ok(self.position)
    }
}

#[inline(never)]
fn create_thumbnail_from_input(
    reader: WasmInput<'_>,
    options: &ThumbnailOptions,
    png_options: &PngOptions,
    jpeg_options: &JpegOptions,
) -> streamthumb_png::Result<ThumbnailOutput> {
    match options.output {
        OutputFormat::Png => {
            thumbnail_png_from_reader_with_encoder_options(reader, options, png_options)
        }
        OutputFormat::Jpeg => {
            thumbnail_png_from_reader_with_jpeg_options(reader, options, jpeg_options)
        }
        OutputFormat::Rgba => thumbnail_png_from_reader(reader, options),
    }
}

/// Inspects and plans a thumbnail without decoding image pixels.
#[wasm_bindgen(js_name = planThumbnailPng, skip_typescript)]
pub fn plan_thumbnail_png(
    input: &[u8],
    options: &JsValue,
    delivery: &JsValue,
) -> Result<JsValue, JsError> {
    let (options, _, _) = parse_options(options)?;
    let plan = match parse_output_delivery(delivery)? {
        OutputDelivery::Buffered => preflight_thumbnail_png(input, &options),
        OutputDelivery::Chunks => {
            preflight_thumbnail_png_to_writer_with_buffer(input, &options, OUTPUT_CHUNK_BYTES)
        }
    }
    .map_err(|error| JsError::new(&error.to_string()))?;
    thumbnail_plan_object(plan)
}

/// Creates a bounded PNG, JPEG, or RGBA thumbnail from encoded PNG bytes.
#[wasm_bindgen(js_name = thumbnailPng, skip_typescript)]
pub fn thumbnail_png(input: &[u8], options: &JsValue) -> Result<ThumbnailResult, JsError> {
    let (options, png_options, jpeg_options) = parse_options(options)?;
    let reader = WasmInput::Slice(Cursor::new(input));
    let output = create_thumbnail_from_input(reader, &options, &png_options, &jpeg_options)
        .map_err(|error| JsError::new(&error.to_string()))?;
    ThumbnailResult::from_output(output)
}

/// Creates a bounded thumbnail through synchronous encoded-input range reads.
#[wasm_bindgen(js_name = thumbnailPngFromSeekable, skip_typescript)]
pub fn thumbnail_png_from_seekable(
    input_length: f64,
    read_at: &Function,
    options: &JsValue,
) -> Result<ThumbnailResult, JsValue> {
    let (options, png_options, jpeg_options) = parse_options(options).map_err(JsValue::from)?;
    let input_length = required_safe_u64(input_length, "inputLength").map_err(JsValue::from)?;
    let callback_error = Rc::new(RefCell::new(None));
    let reader = WasmInput::Seekable(JsSeekableReader::new(
        input_length,
        read_at.clone(),
        Rc::clone(&callback_error),
    ));
    let output = create_thumbnail_from_input(reader, &options, &png_options, &jpeg_options);
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            if let Some(callback_error) = callback_error.borrow_mut().take() {
                return Err(callback_error);
            }
            return Err(JsError::new(&error.to_string()).into());
        }
    };
    ThumbnailResult::from_output(output).map_err(JsValue::from)
}

/// Creates encoded output and forwards owned chunks to a JavaScript callback.
#[wasm_bindgen(js_name = thumbnailPngToChunks, skip_typescript)]
pub fn thumbnail_png_to_chunks(
    input: &[u8],
    on_chunk: &Function,
    options: &JsValue,
) -> Result<ChunkedThumbnailResult, JsValue> {
    let reader = WasmInput::Slice(Cursor::new(input));
    thumbnail_to_chunks_from_input(reader, on_chunk, options, None)
}

/// Creates chunked encoded output through synchronous input range reads.
#[wasm_bindgen(js_name = thumbnailPngFromSeekableToChunks, skip_typescript)]
pub fn thumbnail_png_from_seekable_to_chunks(
    input_length: f64,
    read_at: &Function,
    on_chunk: &Function,
    options: &JsValue,
) -> Result<ChunkedThumbnailResult, JsValue> {
    let input_length = required_safe_u64(input_length, "inputLength").map_err(JsValue::from)?;
    let input_callback_error = Rc::new(RefCell::new(None));
    let reader = WasmInput::Seekable(JsSeekableReader::new(
        input_length,
        read_at.clone(),
        Rc::clone(&input_callback_error),
    ));
    thumbnail_to_chunks_from_input(reader, on_chunk, options, Some(input_callback_error))
}

#[inline(never)]
fn thumbnail_to_chunks_from_input(
    reader: WasmInput<'_>,
    on_chunk: &Function,
    options: &JsValue,
    input_callback_error: Option<Rc<RefCell<Option<JsValue>>>>,
) -> Result<ChunkedThumbnailResult, JsValue> {
    let (options, png_options, jpeg_options) = parse_options(options).map_err(JsValue::from)?;
    if options.output == OutputFormat::Rgba {
        return Err(JsError::new("chunk output requires PNG or JPEG output").into());
    }
    let output_callback_error = Rc::new(RefCell::new(None));
    let stats = Rc::new(RefCell::new(ChunkStats::default()));
    let writer = ChunkCallbackWriter::new({
        let on_chunk = on_chunk.clone();
        let output_callback_error = Rc::clone(&output_callback_error);
        let stats = Rc::clone(&stats);
        move |chunk: &[u8]| {
            let bytes = Uint8Array::from(chunk);
            match on_chunk.call1(&JsValue::UNDEFINED, bytes.as_ref()) {
                Ok(_) => {
                    let mut stats = stats.borrow_mut();
                    stats.bytes_written = stats
                        .bytes_written
                        .checked_add(chunk.len())
                        .ok_or_else(|| io::Error::other("chunk byte count overflow"))?;
                    stats.chunk_count = stats
                        .chunk_count
                        .checked_add(1)
                        .ok_or_else(|| io::Error::other("chunk count overflow"))?;
                    Ok(())
                }
                Err(error) => {
                    *output_callback_error.borrow_mut() = Some(error);
                    Err(io::Error::other("JavaScript chunk callback failed"))
                }
            }
        }
    })
    .map_err(|error| JsError::new(&error.to_string()))?;
    let finalizer = writer.clone();

    let result = match options.output {
        OutputFormat::Png => thumbnail_png_from_reader_to_writer_with_encoder_options_and_buffer(
            reader,
            &options,
            &png_options,
            OUTPUT_CHUNK_BYTES,
            writer,
        ),
        OutputFormat::Jpeg => thumbnail_jpeg_from_reader_to_writer_with_options_and_buffer(
            reader,
            &options,
            &jpeg_options,
            OUTPUT_CHUNK_BYTES,
            writer,
        ),
        OutputFormat::Rgba => unreachable!("raw RGBA output was rejected above"),
    };
    if result.is_ok() {
        if let Err(error) = finalizer.finish() {
            if let Some(callback_error) = input_callback_error
                .as_ref()
                .and_then(|error| error.borrow_mut().take())
            {
                return Err(callback_error);
            }
            if let Some(callback_error) = output_callback_error.borrow_mut().take() {
                return Err(callback_error);
            }
            return Err(JsError::new(&error.to_string()).into());
        }
    }
    let info = match result {
        Ok(info) => info,
        Err(error) => {
            if let Some(callback_error) = input_callback_error
                .as_ref()
                .and_then(|error| error.borrow_mut().take())
            {
                return Err(callback_error);
            }
            if let Some(callback_error) = output_callback_error.borrow_mut().take() {
                return Err(callback_error);
            }
            return Err(JsError::new(&error.to_string()).into());
        }
    };
    let stats = *stats.borrow();
    Ok(ChunkedThumbnailResult {
        width: info.width,
        height: info.height,
        mime_type: match info.format {
            OutputFormat::Png => "image/png",
            OutputFormat::Jpeg => "image/jpeg",
            OutputFormat::Rgba => unreachable!("chunk output cannot return raw RGBA"),
        }
        .to_owned(),
        format: output_format_name(info.format).to_owned(),
        bytes_written: stats.bytes_written as f64,
        chunk_count: stats.chunk_count,
    })
}

#[derive(Clone, Copy, Default)]
struct ChunkStats {
    bytes_written: usize,
    chunk_count: u32,
}

struct ChunkCallbackWriter<F> {
    state: Rc<RefCell<ChunkCallbackState<F>>>,
}

struct ChunkCallbackState<F> {
    buffer: Vec<u8>,
    callback: F,
}

impl<F> Clone for ChunkCallbackWriter<F> {
    fn clone(&self) -> Self {
        Self {
            state: Rc::clone(&self.state),
        }
    }
}

impl<F> ChunkCallbackWriter<F>
where
    F: FnMut(&[u8]) -> io::Result<()>,
{
    fn new(callback: F) -> io::Result<Self> {
        let mut buffer = Vec::new();
        buffer
            .try_reserve_exact(OUTPUT_CHUNK_BYTES)
            .map_err(|_| io::Error::other("could not allocate the encoded chunk buffer"))?;
        Ok(Self {
            state: Rc::new(RefCell::new(ChunkCallbackState { buffer, callback })),
        })
    }

    fn finish(&self) -> io::Result<()> {
        self.state.borrow_mut().emit()
    }
}

impl<F> ChunkCallbackState<F>
where
    F: FnMut(&[u8]) -> io::Result<()>,
{
    fn emit(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        (self.callback)(&self.buffer)?;
        self.buffer.clear();
        Ok(())
    }
}

impl<F> Write for ChunkCallbackWriter<F>
where
    F: FnMut(&[u8]) -> io::Result<()>,
{
    fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
        let original_len = bytes.len();
        let mut state = self.state.borrow_mut();
        while !bytes.is_empty() {
            let available = OUTPUT_CHUNK_BYTES - state.buffer.len();
            let take = available.min(bytes.len());
            state.buffer.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if state.buffer.len() == OUTPUT_CHUNK_BYTES {
                state.emit()?;
            }
        }
        Ok(original_len)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.finish()
    }
}

/// Metadata returned after chunked encoding completes.
#[wasm_bindgen]
pub struct ChunkedThumbnailResult {
    width: u32,
    height: u32,
    mime_type: String,
    format: String,
    bytes_written: f64,
    chunk_count: u32,
}

#[wasm_bindgen]
impl ChunkedThumbnailResult {
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.height
    }

    #[wasm_bindgen(getter, js_name = mimeType)]
    pub fn mime_type(&self) -> String {
        self.mime_type.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn format(&self) -> String {
        self.format.clone()
    }

    #[wasm_bindgen(getter, js_name = bytesWritten)]
    pub fn bytes_written(&self) -> f64 {
        self.bytes_written
    }

    #[wasm_bindgen(getter, js_name = chunkCount)]
    pub fn chunk_count(&self) -> u32 {
        self.chunk_count
    }
}

/// A JavaScript-facing thumbnail result.
#[wasm_bindgen]
pub struct ThumbnailResult {
    bytes: Vec<u8>,
    width: u32,
    height: u32,
    mime_type: String,
    format: String,
}

#[wasm_bindgen]
impl ThumbnailResult {
    /// Returns a copy of the encoded image or raw RGBA bytes.
    #[wasm_bindgen(getter)]
    pub fn bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.height
    }

    #[wasm_bindgen(getter, js_name = mimeType)]
    pub fn mime_type(&self) -> String {
        self.mime_type.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn format(&self) -> String {
        self.format.clone()
    }
}

impl ThumbnailResult {
    fn from_output(output: ThumbnailOutput) -> Result<Self, JsError> {
        let result = match output {
            ThumbnailOutput::Encoded {
                bytes,
                width,
                height,
                mime_type,
                format,
            } => Self {
                bytes,
                width,
                height,
                mime_type: mime_type.to_owned(),
                format: output_format_name(format).to_owned(),
            },
            ThumbnailOutput::Rgba {
                pixels,
                width,
                height,
            } => Self {
                bytes: pixels,
                width,
                height,
                mime_type: "application/octet-stream".to_owned(),
                format: "rgba".to_owned(),
            },
            _ => return Err(JsError::new("unsupported thumbnail output representation")),
        };
        Ok(result)
    }
}

fn parse_options(value: &JsValue) -> Result<(ThumbnailOptions, PngOptions, JpegOptions), JsError> {
    let mut options = ThumbnailOptions::default();
    if value.is_null() || value.is_undefined() {
        return Ok((options, PngOptions::default(), JpegOptions::default()));
    }
    if !value.is_object() {
        return Err(JsError::new("thumbnail options must be an object"));
    }

    if let Some(number) = optional_u32(value, "maxWidth")? {
        options.max_width = number;
    }
    if let Some(number) = optional_u32(value, "maxHeight")? {
        options.max_height = number;
    }
    if let Some(boolean) = optional_bool(value, "allowUpscale")? {
        options.allow_upscale = boolean;
    }
    if let Some(fit) = optional_string(value, "fit")? {
        options.fit = match fit.as_str() {
            "contain" => Fit::Contain,
            "cover" => Fit::Cover,
            _ => return Err(JsError::new("fit must be \"contain\" or \"cover\"")),
        };
    }
    if let Some(filter) = optional_string(value, "filter")? {
        options.filter = match filter.as_str() {
            "area" => Filter::Area,
            _ => return Err(JsError::new("filter must be \"area\"")),
        };
    }
    if let Some(output) = optional_string(value, "output")? {
        options.output = match output.as_str() {
            "png" => OutputFormat::Png,
            "jpeg" => OutputFormat::Jpeg,
            "rgba" => OutputFormat::Rgba,
            _ => {
                return Err(JsError::new(
                    "output must be \"png\", \"jpeg\", or \"rgba\"",
                ));
            }
        };
    }
    let png_value = optional_property(value, "png")?;
    if png_value.is_some() && options.output != OutputFormat::Png {
        return Err(JsError::new("png options require output \"png\""));
    }
    let png_options = png_value
        .as_ref()
        .map(parse_png_options)
        .transpose()?
        .unwrap_or_default();
    let jpeg_value = optional_property(value, "jpeg")?;
    if jpeg_value.is_some() && options.output != OutputFormat::Jpeg {
        return Err(JsError::new("jpeg options require output \"jpeg\""));
    }
    let jpeg_options = jpeg_value
        .as_ref()
        .map(parse_jpeg_options)
        .transpose()?
        .unwrap_or_default();

    if let Some(number) = optional_u64(value, "maxInputBytes")? {
        options.limits.max_input_bytes = number;
    }
    if let Some(number) = optional_u32(value, "maxInputWidth")? {
        options.limits.max_width = number;
    }
    if let Some(number) = optional_u32(value, "maxInputHeight")? {
        options.limits.max_height = number;
    }
    if let Some(number) = optional_u64(value, "maxInputPixels")? {
        options.limits.max_pixels = number;
    }
    if let Some(number) = optional_u32(value, "maxOutputWidth")? {
        options.limits.max_output_width = number;
    }
    if let Some(number) = optional_u32(value, "maxOutputHeight")? {
        options.limits.max_output_height = number;
    }
    if let Some(number) = optional_u64(value, "maxOutputPixels")? {
        options.limits.max_output_pixels = number;
    }
    if let Some(number) = optional_u64(value, "maxMemoryBytes")? {
        options.limits.max_working_memory_bytes = usize::try_from(number)
            .map_err(|_| JsError::new("maxMemoryBytes is too large for this WebAssembly build"))?;
    }

    Ok((options, png_options, jpeg_options))
}

fn parse_output_delivery(value: &JsValue) -> Result<OutputDelivery, JsError> {
    if value.is_null() || value.is_undefined() {
        return Ok(OutputDelivery::Buffered);
    }
    match value.as_string().as_deref() {
        Some("buffered") => Ok(OutputDelivery::Buffered),
        Some("chunks") => Ok(OutputDelivery::Chunks),
        _ => Err(JsError::new("delivery must be \"buffered\" or \"chunks\"")),
    }
}

fn thumbnail_plan_object(plan: PngThumbnailPlan) -> Result<JsValue, JsError> {
    let input = Object::new();
    set_number(&input, "width", plan.input.dimensions.width)?;
    set_number(&input, "height", plan.input.dimensions.height)?;
    set_number(&input, "encodedBytes", plan.input.encoded_bytes)?;
    set_string(
        &input,
        "colorType",
        png_input_color_type_name(plan.input.color_type),
    )?;
    set_number(&input, "bitDepth", plan.input.bit_depth)?;
    set_bool(&input, "interlaced", plan.input.interlaced)?;

    let output = Object::new();
    set_number(&output, "width", plan.processing.output.width)?;
    set_number(&output, "height", plan.processing.output.height)?;
    set_string(&output, "format", output_format_name(plan.output_format))?;

    let memory = Object::new();
    let estimate = plan.processing.memory;
    for (name, value) in [
        ("decoderRowsBytes", estimate.decoder_rows_bytes),
        ("decoderStagingBytes", estimate.decoder_staging_bytes),
        ("normalizedRowBytes", estimate.normalized_row_bytes),
        (
            "horizontalAccumulatorBytes",
            estimate.horizontal_accumulator_bytes,
        ),
        (
            "verticalAccumulatorBytes",
            estimate.vertical_accumulator_bytes,
        ),
        ("sparseAccumulatorBytes", estimate.sparse_accumulator_bytes),
        ("outputRowBytes", estimate.output_row_bytes),
        ("outputRgbaBytes", estimate.output_rgba_bytes),
        ("encoderStateBytes", estimate.encoder_state_bytes),
        ("encodedOutputBytes", estimate.encoded_output_bytes),
        ("totalBytes", estimate.total_bytes),
    ] {
        set_number(&memory, name, value)?;
    }

    let result = Object::new();
    set_property(&result, "input", input.as_ref())?;
    set_property(&result, "output", output.as_ref())?;
    set_property(&result, "memory", memory.as_ref())?;
    set_number(
        &result,
        "configuredMaxMemoryBytes",
        plan.configured_max_working_memory_bytes,
    )?;
    set_bool(&result, "withinMemoryLimit", plan.within_memory_limit)?;
    Ok(result.into())
}

trait JsNumber {
    fn to_f64(self) -> f64;
}

impl JsNumber for u8 {
    fn to_f64(self) -> f64 {
        f64::from(self)
    }
}

impl JsNumber for u32 {
    fn to_f64(self) -> f64 {
        f64::from(self)
    }
}

impl JsNumber for u64 {
    fn to_f64(self) -> f64 {
        self as f64
    }
}

impl JsNumber for usize {
    fn to_f64(self) -> f64 {
        self as f64
    }
}

fn set_number<T: JsNumber>(object: &Object, name: &str, value: T) -> Result<(), JsError> {
    set_property(object, name, &JsValue::from_f64(value.to_f64()))
}

fn set_string(object: &Object, name: &str, value: &str) -> Result<(), JsError> {
    set_property(object, name, &JsValue::from_str(value))
}

fn set_bool(object: &Object, name: &str, value: bool) -> Result<(), JsError> {
    set_property(object, name, &JsValue::from_bool(value))
}

fn set_property(object: &Object, name: &str, value: &JsValue) -> Result<(), JsError> {
    let written = Reflect::set(object.as_ref(), &JsValue::from_str(name), value)
        .map_err(|_| JsError::new(&format!("could not write plan property {name}")))?;
    if written {
        Ok(())
    } else {
        Err(JsError::new(&format!(
            "could not write plan property {name}"
        )))
    }
}

const fn png_input_color_type_name(color: PngInputColorType) -> &'static str {
    match color {
        PngInputColorType::Grayscale => "grayscale",
        PngInputColorType::Rgb => "rgb",
        PngInputColorType::Indexed => "indexed",
        PngInputColorType::GrayscaleAlpha => "grayscale-alpha",
        PngInputColorType::Rgba => "rgba",
    }
}

fn parse_png_options(value: &JsValue) -> Result<PngOptions, JsError> {
    if !value.is_object() || js_sys::Array::is_array(value) {
        return Err(JsError::new("png must be an object"));
    }
    let mut options = PngOptions::default();
    if let Some(color) = optional_string(value, "color")? {
        options.color = match color.as_str() {
            "auto" => PngColorMode::Auto,
            "rgba8" => PngColorMode::Rgba8,
            "rgb8" => PngColorMode::Rgb8,
            "grayscale-alpha8" => PngColorMode::GrayscaleAlpha8,
            "grayscale8" => PngColorMode::Grayscale8,
            _ => {
                return Err(JsError::new(
                    "png.color must be \"auto\", \"rgba8\", \"rgb8\", \"grayscale-alpha8\", or \"grayscale8\"",
                ));
            }
        };
    }
    if let Some(compression) = optional_string(value, "compression")? {
        options.compression = match compression.as_str() {
            "none" => PngCompression::NoCompression,
            "fastest" => PngCompression::Fastest,
            "fast" => PngCompression::Fast,
            "balanced" => PngCompression::Balanced,
            "high" => PngCompression::High,
            _ => {
                return Err(JsError::new(
                    "png.compression must be \"none\", \"fastest\", \"fast\", \"balanced\", or \"high\"",
                ));
            }
        };
    }
    if let Some(filter) = optional_string(value, "filter")? {
        options.filter = match filter.as_str() {
            "default" => PngFilter::Default,
            "none" => PngFilter::None,
            "sub" => PngFilter::Sub,
            "up" => PngFilter::Up,
            "average" => PngFilter::Average,
            "paeth" => PngFilter::Paeth,
            "adaptive" => PngFilter::Adaptive,
            "min-entropy" => PngFilter::MinEntropy,
            _ => {
                return Err(JsError::new(
                    "png.filter must be \"default\", \"none\", \"sub\", \"up\", \"average\", \"paeth\", \"adaptive\", or \"min-entropy\"",
                ));
            }
        };
    }
    Ok(options)
}

fn parse_jpeg_options(value: &JsValue) -> Result<JpegOptions, JsError> {
    if !value.is_object() || js_sys::Array::is_array(value) {
        return Err(JsError::new("jpeg must be an object"));
    }
    let mut options = JpegOptions::default();
    if let Some(quality) = optional_u64(value, "quality")? {
        options.quality = u8::try_from(quality)
            .ok()
            .filter(|quality| (1..=100).contains(quality))
            .ok_or_else(|| JsError::new("jpeg.quality must be an integer from 1 through 100"))?;
    }
    if let Some(background) = optional_property(value, "background")? {
        options.background = parse_rgb(&background)?;
    }
    if let Some(subsampling) = optional_string(value, "subsampling")? {
        options.subsampling = match subsampling.as_str() {
            "420" => JpegSubsampling::S420,
            "422" => JpegSubsampling::S422,
            "444" => JpegSubsampling::S444,
            _ => {
                return Err(JsError::new(
                    "jpeg.subsampling must be \"420\", \"422\", or \"444\"",
                ));
            }
        };
    }
    Ok(options)
}

fn parse_rgb(value: &JsValue) -> Result<[u8; 3], JsError> {
    if !js_sys::Array::is_array(value) {
        return Err(JsError::new(
            "jpeg.background must be a three-element RGB array",
        ));
    }
    let values = js_sys::Array::from(value);
    if values.length() != 3 {
        return Err(JsError::new(
            "jpeg.background must be a three-element RGB array",
        ));
    }
    let mut rgb = [0_u8; 3];
    for (index, channel) in rgb.iter_mut().enumerate() {
        let number = values
            .get(index as u32)
            .as_f64()
            .filter(|number| number.is_finite() && number.fract() == 0.0)
            .ok_or_else(|| JsError::new("jpeg.background channels must be integers"))?;
        if !(0.0..=255.0).contains(&number) {
            return Err(JsError::new(
                "jpeg.background channels must be between 0 and 255",
            ));
        }
        *channel = number as u8;
    }
    Ok(rgb)
}

const fn output_format_name(format: OutputFormat) -> &'static str {
    match format {
        OutputFormat::Png => "png",
        OutputFormat::Jpeg => "jpeg",
        OutputFormat::Rgba => "rgba",
    }
}

fn optional_property(object: &JsValue, name: &str) -> Result<Option<JsValue>, JsError> {
    let value = Reflect::get(object, &JsValue::from_str(name))
        .map_err(|_| JsError::new(&format!("could not read option {name}")))?;
    if value.is_null() || value.is_undefined() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

fn optional_bool(object: &JsValue, name: &str) -> Result<Option<bool>, JsError> {
    optional_property(object, name)?
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| JsError::new(&format!("{name} must be a boolean")))
        })
        .transpose()
}

fn optional_string(object: &JsValue, name: &str) -> Result<Option<String>, JsError> {
    optional_property(object, name)?
        .map(|value| {
            value
                .as_string()
                .ok_or_else(|| JsError::new(&format!("{name} must be a string")))
        })
        .transpose()
}

fn optional_u32(object: &JsValue, name: &str) -> Result<Option<u32>, JsError> {
    optional_u64(object, name)?
        .map(|value| {
            u32::try_from(value).map_err(|_| JsError::new(&format!("{name} exceeds u32 range")))
        })
        .transpose()
}

fn optional_u64(object: &JsValue, name: &str) -> Result<Option<u64>, JsError> {
    optional_property(object, name)?
        .map(|value| {
            let number = value.as_f64().ok_or_else(|| {
                JsError::new(&format!("{name} must be a non-negative safe integer"))
            })?;
            required_safe_u64(number, name)
        })
        .transpose()
}

fn required_safe_u64(number: f64, name: &str) -> Result<u64, JsError> {
    if !number.is_finite()
        || number < 0.0
        || number.fract() != 0.0
        || number > 9_007_199_254_740_991.0
    {
        return Err(JsError::new(&format!(
            "{name} must be a non-negative safe integer"
        )));
    }
    Ok(number as u64)
}

#[cfg(test)]
mod chunk_writer_tests {
    use super::*;

    #[test]
    fn batches_output_into_bounded_chunks_and_flushes_the_tail() {
        let chunks = Rc::new(RefCell::new(Vec::new()));
        let target = Rc::clone(&chunks);
        let mut writer = ChunkCallbackWriter::new(move |chunk: &[u8]| {
            target.borrow_mut().push(chunk.to_vec());
            Ok(())
        })
        .unwrap();
        let bytes = vec![7_u8; OUTPUT_CHUNK_BYTES * 2 + 17];

        writer.write_all(&bytes).unwrap();
        writer.flush().unwrap();

        let chunks = chunks.borrow();
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), OUTPUT_CHUNK_BYTES);
        assert_eq!(chunks[1].len(), OUTPUT_CHUNK_BYTES);
        assert_eq!(chunks[2].len(), 17);
        assert_eq!(chunks.concat(), bytes);
    }

    #[test]
    fn stops_after_a_chunk_callback_error() {
        let mut writer = ChunkCallbackWriter::new(|_: &[u8]| {
            Err(io::Error::other("injected chunk callback failure"))
        })
        .unwrap();

        assert!(writer.write_all(&vec![0; OUTPUT_CHUNK_BYTES]).is_err());
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod browser_tests {
    use super::*;
    use js_sys::Object;
    use wasm_bindgen::{JsCast, closure::Closure};
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_dedicated_worker);

    const PNG_INPUT: &[u8] =
        include_bytes!("../../../fuzz/corpus/thumbnail_png/pngsuite_basn6a08.png");

    fn options_with(name: &str, value: &JsValue) -> Object {
        let options = Object::new();
        Reflect::set(options.as_ref(), &JsValue::from_str(name), value)
            .expect("the test options object must be writable");
        options
    }

    fn blob_read_at(input: &[u8]) -> (Function, Object) {
        let factory = Function::new_with_args(
            "input",
            r#"
                const blob = new Blob([input]);
                const reader = new FileReaderSync();
                const stats = { calls: 0, largest: 0 };
                const readAt = (offset, length) => {
                    stats.calls += 1;
                    stats.largest = Math.max(stats.largest, length);
                    return new Uint8Array(
                        reader.readAsArrayBuffer(blob.slice(offset, offset + length)),
                    );
                };
                return { readAt, stats };
            "#,
        );
        let input = Uint8Array::from(input);
        let harness = factory
            .call1(&JsValue::UNDEFINED, input.as_ref())
            .expect("the Blob callback harness must initialize")
            .unchecked_into::<Object>();
        let read_at = Reflect::get(&harness, &JsValue::from_str("readAt"))
            .expect("the Blob harness must expose readAt")
            .unchecked_into::<Function>();
        let stats = Reflect::get(&harness, &JsValue::from_str("stats"))
            .expect("the Blob harness must expose stats")
            .unchecked_into::<Object>();
        (read_at, stats)
    }

    fn property(object: &JsValue, name: &str) -> JsValue {
        Reflect::get(object, &JsValue::from_str(name))
            .unwrap_or_else(|_| panic!("plan property {name} must be readable"))
    }

    fn number_property(object: &JsValue, name: &str) -> f64 {
        property(object, name)
            .as_f64()
            .unwrap_or_else(|| panic!("plan property {name} must be numeric"))
    }

    fn bool_property(object: &JsValue, name: &str) -> bool {
        property(object, name)
            .as_bool()
            .unwrap_or_else(|| panic!("plan property {name} must be boolean"))
    }

    fn adam7_header_input() -> Vec<u8> {
        let mut input = PNG_INPUT.to_vec();
        input[28] = 1;
        let crc = crc32(&input[12..29]).to_be_bytes();
        input[29..33].copy_from_slice(&crc);
        input
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = 0_u32.wrapping_sub(crc & 1);
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        !crc
    }

    fn high_entropy_png() -> Vec<u8> {
        const DIMENSION: u32 = 256;
        let mut state = 0x6d2b_79f5_u32;
        let mut pixels = Vec::with_capacity((DIMENSION * DIMENSION * 3) as usize);
        for _ in 0..DIMENSION * DIMENSION * 3 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            pixels.push(state as u8);
        }

        let mut input = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut input, DIMENSION, DIMENSION);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::Fast);
            let mut writer = encoder.write_header().expect("PNG header must encode");
            writer
                .write_image_data(&pixels)
                .expect("PNG pixels must encode");
        }
        input
    }

    fn large_output_options(format: OutputFormat) -> Object {
        let options = options_with("output", &JsValue::from_str(output_format_name(format)));
        if format == OutputFormat::Png {
            let png = Object::new();
            Reflect::set(
                png.as_ref(),
                &JsValue::from_str("compression"),
                &JsValue::from_str("none"),
            )
            .expect("the PNG options object must be writable");
            Reflect::set(options.as_ref(), &JsValue::from_str("png"), png.as_ref())
                .expect("the thumbnail options object must be writable");
        } else {
            let jpeg = Object::new();
            Reflect::set(
                jpeg.as_ref(),
                &JsValue::from_str("quality"),
                &JsValue::from_f64(100.0),
            )
            .expect("the JPEG options object must be writable");
            Reflect::set(
                jpeg.as_ref(),
                &JsValue::from_str("subsampling"),
                &JsValue::from_str("444"),
            )
            .expect("the JPEG options object must be writable");
            Reflect::set(options.as_ref(), &JsValue::from_str("jpeg"), jpeg.as_ref())
                .expect("the thumbnail options object must be writable");
        }
        options
    }

    #[wasm_bindgen_test]
    fn returns_a_complete_plain_object_thumbnail_plan() {
        let plan = plan_thumbnail_png(PNG_INPUT, &JsValue::NULL, &JsValue::UNDEFINED)
            .expect("default planning must succeed");
        let input = property(&plan, "input");
        let output = property(&plan, "output");
        let memory = property(&plan, "memory");

        assert_eq!(number_property(&input, "width"), 32.0);
        assert_eq!(number_property(&input, "height"), 32.0);
        assert_eq!(
            number_property(&input, "encodedBytes"),
            PNG_INPUT.len() as f64
        );
        assert_eq!(
            property(&input, "colorType").as_string().as_deref(),
            Some("rgba")
        );
        assert_eq!(number_property(&input, "bitDepth"), 8.0);
        assert!(!bool_property(&input, "interlaced"));
        assert_eq!(number_property(&output, "width"), 32.0);
        assert_eq!(number_property(&output, "height"), 32.0);
        assert_eq!(
            property(&output, "format").as_string().as_deref(),
            Some("png")
        );
        assert_eq!(
            number_property(&plan, "configuredMaxMemoryBytes"),
            32.0 * 1024.0 * 1024.0
        );
        assert!(bool_property(&plan, "withinMemoryLimit"));
        assert!(property(&plan, "free").is_undefined());

        let components = [
            "decoderRowsBytes",
            "decoderStagingBytes",
            "normalizedRowBytes",
            "horizontalAccumulatorBytes",
            "verticalAccumulatorBytes",
            "sparseAccumulatorBytes",
            "outputRowBytes",
            "outputRgbaBytes",
            "encoderStateBytes",
            "encodedOutputBytes",
        ];
        let sum = components
            .iter()
            .map(|name| number_property(&memory, name))
            .sum::<f64>();
        assert_eq!(number_property(&memory, "totalBytes"), sum);
    }

    #[wasm_bindgen_test]
    fn distinguishes_buffered_chunked_rgba_and_adam7_plans() {
        let buffered =
            plan_thumbnail_png(PNG_INPUT, &JsValue::NULL, &JsValue::from_str("buffered"))
                .expect("buffered planning must succeed");
        let chunks = plan_thumbnail_png(PNG_INPUT, &JsValue::NULL, &JsValue::from_str("chunks"))
            .expect("chunk planning must succeed");
        assert!(number_property(&property(&buffered, "memory"), "encodedOutputBytes") > 0.0);
        assert_eq!(
            number_property(&property(&chunks, "memory"), "encodedOutputBytes"),
            OUTPUT_CHUNK_BYTES as f64
        );

        let rgba_options = options_with("output", &JsValue::from_str("rgba"));
        let rgba = plan_thumbnail_png(
            PNG_INPUT,
            rgba_options.as_ref(),
            &JsValue::from_str("buffered"),
        )
        .expect("RGBA buffered planning must succeed");
        assert!(number_property(&property(&rgba, "memory"), "outputRgbaBytes") > 0.0);
        assert!(
            plan_thumbnail_png(
                PNG_INPUT,
                rgba_options.as_ref(),
                &JsValue::from_str("chunks"),
            )
            .is_err()
        );

        let adam7 = plan_thumbnail_png(&adam7_header_input(), &JsValue::NULL, &JsValue::UNDEFINED)
            .expect("Adam7 header planning must succeed");
        assert!(bool_property(&property(&adam7, "input"), "interlaced"));
        let memory = property(&adam7, "memory");
        assert!(number_property(&memory, "sparseAccumulatorBytes") > 0.0);
        assert_eq!(number_property(&memory, "horizontalAccumulatorBytes"), 0.0);
        assert_eq!(number_property(&memory, "verticalAccumulatorBytes"), 0.0);
    }

    #[wasm_bindgen_test]
    fn reports_memory_rejection_before_the_enforcing_call() {
        let options = options_with("maxMemoryBytes", &JsValue::from_f64(1.0));
        let plan = plan_thumbnail_png(PNG_INPUT, options.as_ref(), &JsValue::UNDEFINED)
            .expect("planning must return a below-limit result");

        assert!(!bool_property(&plan, "withinMemoryLimit"));
        assert_eq!(number_property(&plan, "configuredMaxMemoryBytes"), 1.0);
        assert!(number_property(&property(&plan, "memory"), "totalBytes") > 1.0);
        assert!(thumbnail_png(PNG_INPUT, options.as_ref()).is_err());
        assert!(
            plan_thumbnail_png(PNG_INPUT, &JsValue::NULL, &JsValue::from_str("stream")).is_err()
        );
    }

    #[wasm_bindgen_test]
    fn planned_options_remain_executable() {
        let options = options_with("maxWidth", &JsValue::from_f64(8.0));
        let plan = plan_thumbnail_png(PNG_INPUT, options.as_ref(), &JsValue::UNDEFINED)
            .expect("planning must succeed");
        let output = property(&plan, "output");
        let result = thumbnail_png(PNG_INPUT, options.as_ref()).expect("execution must succeed");

        assert_eq!(number_property(&output, "width"), f64::from(result.width()));
        assert_eq!(
            number_property(&output, "height"),
            f64::from(result.height())
        );
    }

    #[wasm_bindgen_test]
    fn creates_a_png_thumbnail_in_a_dedicated_worker() {
        let options = Object::new();
        Reflect::set(
            options.as_ref(),
            &JsValue::from_str("maxWidth"),
            &JsValue::from_f64(8.0),
        )
        .expect("the test options object must be writable");
        Reflect::set(
            options.as_ref(),
            &JsValue::from_str("maxHeight"),
            &JsValue::from_f64(8.0),
        )
        .expect("the test options object must be writable");
        Reflect::set(
            options.as_ref(),
            &JsValue::from_str("output"),
            &JsValue::from_str("png"),
        )
        .expect("the test options object must be writable");

        let result = thumbnail_png(PNG_INPUT, options.as_ref()).expect("thumbnail must succeed");
        let bytes = result.bytes();

        assert_eq!(result.width(), 8);
        assert_eq!(result.height(), 8);
        assert_eq!(result.mime_type(), "image/png");
        assert_eq!(result.format(), "png");
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[wasm_bindgen_test]
    fn seekable_file_input_matches_buffered_png_jpeg_and_rgba() {
        for format in [OutputFormat::Png, OutputFormat::Jpeg, OutputFormat::Rgba] {
            let options = options_with("output", &JsValue::from_str(output_format_name(format)));
            let expected =
                thumbnail_png(PNG_INPUT, options.as_ref()).expect("slice thumbnail must succeed");
            let (read_at, stats) = blob_read_at(PNG_INPUT);
            let actual =
                thumbnail_png_from_seekable(PNG_INPUT.len() as f64, &read_at, options.as_ref())
                    .expect("seekable thumbnail must succeed");

            assert_eq!(actual.width(), expected.width());
            assert_eq!(actual.height(), expected.height());
            assert_eq!(actual.format(), expected.format());
            assert_eq!(actual.mime_type(), expected.mime_type());
            assert_eq!(actual.bytes(), expected.bytes());
            assert!(number_property(stats.as_ref(), "calls") > 0.0);
            assert!(number_property(stats.as_ref(), "largest") <= 8.0 * 1024.0);
        }
    }

    #[wasm_bindgen_test]
    fn seekable_chunk_output_matches_buffered_png_and_jpeg() {
        for format in [OutputFormat::Png, OutputFormat::Jpeg] {
            let options = options_with("output", &JsValue::from_str(output_format_name(format)));
            let expected = thumbnail_png(PNG_INPUT, options.as_ref())
                .expect("buffered thumbnail must succeed")
                .bytes();
            let (read_at, _) = blob_read_at(PNG_INPUT);
            let chunks = Rc::new(RefCell::new(Vec::<Vec<u8>>::new()));
            let target = Rc::clone(&chunks);
            let callback: Closure<dyn FnMut(Uint8Array)> =
                Closure::new(move |chunk: Uint8Array| target.borrow_mut().push(chunk.to_vec()));

            let result = thumbnail_png_from_seekable_to_chunks(
                PNG_INPUT.len() as f64,
                &read_at,
                callback.as_ref().unchecked_ref(),
                options.as_ref(),
            )
            .expect("seekable chunk output must succeed");
            let chunks = chunks.borrow();
            assert_eq!(chunks.concat(), expected);
            assert!(chunks.iter().all(|chunk| chunk.len() <= OUTPUT_CHUNK_BYTES));
            assert_eq!(result.format(), output_format_name(format));
            assert_eq!(result.bytes_written(), expected.len() as f64);
            assert_eq!(result.chunk_count(), chunks.len() as u32);
        }
    }

    #[wasm_bindgen_test]
    fn seekable_input_limit_is_checked_before_the_first_read() {
        let (read_at, stats) = blob_read_at(PNG_INPUT);
        let options = options_with(
            "maxInputBytes",
            &JsValue::from_f64((PNG_INPUT.len() - 1) as f64),
        );
        assert!(
            thumbnail_png_from_seekable(PNG_INPUT.len() as f64, &read_at, options.as_ref())
                .is_err()
        );
        assert_eq!(number_property(stats.as_ref(), "calls"), 0.0);
    }

    #[wasm_bindgen_test]
    fn seekable_input_and_output_callback_exceptions_preserve_identity() {
        let input_factory = Function::new_no_args(
            "const marker = { kind: 'input' }; return { marker, readAt() { throw marker; } };",
        );
        let input_harness = input_factory
            .call0(&JsValue::UNDEFINED)
            .unwrap()
            .unchecked_into::<Object>();
        let input_marker = property(input_harness.as_ref(), "marker");
        let read_at = property(input_harness.as_ref(), "readAt").unchecked_into::<Function>();
        let error = match thumbnail_png_from_seekable(16.0, &read_at, &JsValue::NULL) {
            Ok(_) => panic!("input callback exception must abort processing"),
            Err(error) => error,
        };
        assert!(Object::is(&error, &input_marker));

        let (read_at, _) = blob_read_at(PNG_INPUT);
        let output_marker = Object::new();
        let output_factory = Function::new_with_args("marker", "return () => { throw marker; };");
        let on_chunk = output_factory
            .call1(&JsValue::UNDEFINED, output_marker.as_ref())
            .unwrap()
            .unchecked_into::<Function>();
        let error = match thumbnail_png_from_seekable_to_chunks(
            PNG_INPUT.len() as f64,
            &read_at,
            &on_chunk,
            &JsValue::NULL,
        ) {
            Ok(_) => panic!("output callback exception must abort processing"),
            Err(error) => error,
        };
        assert!(Object::is(&error, output_marker.as_ref()));
    }

    #[wasm_bindgen_test]
    fn seekable_callback_contract_rejects_invalid_values() {
        let noop = Function::new_no_args("");
        for input_length in [f64::NAN, -1.0, 1.5, 9_007_199_254_740_992.0] {
            assert!(
                thumbnail_png_from_seekable(input_length, &noop, &JsValue::NULL).is_err(),
                "invalid inputLength {input_length} must fail",
            );
        }

        for body in [
            "return Promise.resolve(new Uint8Array(length));",
            "return new Uint8Array(Math.max(0, length - 1));",
            "return new Uint8Array(length + 1);",
            "return [offset, length];",
        ] {
            let read_at = Function::new_with_args("offset, length", body);
            assert!(
                thumbnail_png_from_seekable(16.0, &read_at, &JsValue::NULL).is_err(),
                "invalid callback body must fail: {body}",
            );
        }
    }

    #[wasm_bindgen_test]
    fn applies_nested_png_encoder_options() {
        let options = Object::new();
        let png = Object::new();
        for (name, value) in [
            ("color", "grayscale8"),
            ("compression", "fast"),
            ("filter", "paeth"),
        ] {
            Reflect::set(
                png.as_ref(),
                &JsValue::from_str(name),
                &JsValue::from_str(value),
            )
            .expect("the PNG options object must be writable");
        }
        Reflect::set(options.as_ref(), &JsValue::from_str("png"), png.as_ref())
            .expect("the thumbnail options object must be writable");

        let result = thumbnail_png(PNG_INPUT, options.as_ref()).expect("thumbnail must succeed");
        assert_eq!(result.bytes()[25], 0, "IHDR must declare grayscale output");
    }

    #[wasm_bindgen_test]
    fn creates_jpeg_output_with_nested_options() {
        let options = Object::new();
        Reflect::set(
            options.as_ref(),
            &JsValue::from_str("output"),
            &JsValue::from_str("jpeg"),
        )
        .expect("the thumbnail options object must be writable");
        let jpeg = Object::new();
        Reflect::set(
            jpeg.as_ref(),
            &JsValue::from_str("quality"),
            &JsValue::from_f64(92.0),
        )
        .expect("the JPEG options object must be writable");
        Reflect::set(
            jpeg.as_ref(),
            &JsValue::from_str("subsampling"),
            &JsValue::from_str("444"),
        )
        .expect("the JPEG options object must be writable");
        let background = js_sys::Array::new();
        for channel in [10, 100, 220] {
            background.push(&JsValue::from_f64(f64::from(channel)));
        }
        Reflect::set(
            jpeg.as_ref(),
            &JsValue::from_str("background"),
            background.as_ref(),
        )
        .expect("the JPEG options object must be writable");
        Reflect::set(options.as_ref(), &JsValue::from_str("jpeg"), jpeg.as_ref())
            .expect("the thumbnail options object must be writable");

        let result = thumbnail_png(PNG_INPUT, options.as_ref()).expect("thumbnail must succeed");
        let bytes = result.bytes();
        assert_eq!(result.mime_type(), "image/jpeg");
        assert_eq!(result.format(), "jpeg");
        assert_eq!(&bytes[..2], &[0xff, 0xd8]);
        assert_eq!(&bytes[bytes.len() - 2..], &[0xff, 0xd9]);
    }

    #[wasm_bindgen_test]
    fn chunk_callback_matches_buffered_png_and_jpeg_output() {
        for format in [OutputFormat::Png, OutputFormat::Jpeg] {
            let options = options_with("output", &JsValue::from_str(output_format_name(format)));
            let expected_result = thumbnail_png(PNG_INPUT, options.as_ref())
                .expect("buffered thumbnail must succeed");
            let expected_width = expected_result.width();
            let expected_height = expected_result.height();
            let expected = expected_result.bytes();
            let chunks = Rc::new(RefCell::new(Vec::<Vec<u8>>::new()));
            let target = Rc::clone(&chunks);
            let callback: Closure<dyn FnMut(Uint8Array)> =
                Closure::new(move |chunk: Uint8Array| {
                    target.borrow_mut().push(chunk.to_vec());
                });

            let result = thumbnail_png_to_chunks(
                PNG_INPUT,
                callback.as_ref().unchecked_ref(),
                options.as_ref(),
            )
            .expect("chunked thumbnail must succeed");
            let chunks = chunks.borrow();
            assert_eq!(chunks.concat(), expected);
            assert!(chunks.iter().all(|chunk| chunk.len() <= OUTPUT_CHUNK_BYTES));
            assert_eq!(result.width(), expected_width);
            assert_eq!(result.height(), expected_height);
            assert_eq!(result.format(), output_format_name(format));
            assert_eq!(result.bytes_written(), expected.len() as f64);
            assert_eq!(result.chunk_count(), chunks.len() as u32);
        }
    }

    #[wasm_bindgen_test]
    fn chunk_callback_delivers_multiple_bounded_png_and_jpeg_chunks() {
        let input = high_entropy_png();
        for format in [OutputFormat::Png, OutputFormat::Jpeg] {
            let options = large_output_options(format);
            let expected = thumbnail_png(&input, options.as_ref())
                .expect("buffered thumbnail must succeed")
                .bytes();
            let chunks = Rc::new(RefCell::new(Vec::<Vec<u8>>::new()));
            let target = Rc::clone(&chunks);
            let callback: Closure<dyn FnMut(Uint8Array)> =
                Closure::new(move |chunk: Uint8Array| {
                    target.borrow_mut().push(chunk.to_vec());
                });

            let result = thumbnail_png_to_chunks(
                &input,
                callback.as_ref().unchecked_ref(),
                options.as_ref(),
            )
            .expect("chunked thumbnail must succeed");
            let chunks = chunks.borrow();
            assert!(chunks.len() > 1, "output must cross the chunk boundary");
            assert!(chunks.iter().all(|chunk| chunk.len() <= OUTPUT_CHUNK_BYTES));
            assert_eq!(chunks.concat(), expected);
            assert_eq!(result.bytes_written(), expected.len() as f64);
            assert_eq!(result.chunk_count(), chunks.len() as u32);
        }
    }

    #[wasm_bindgen_test]
    fn chunk_callback_errors_and_raw_output_are_rejected() {
        let sentinel = Object::new();
        let factory = Function::new_with_args("sentinel", "return () => { throw sentinel; };");
        let throwing = factory
            .call1(&JsValue::UNDEFINED, sentinel.as_ref())
            .expect("the callback factory must succeed")
            .dyn_into::<Function>()
            .expect("the callback factory must return a function");
        let error = match thumbnail_png_to_chunks(PNG_INPUT, &throwing, &JsValue::NULL) {
            Ok(_) => panic!("the callback exception must abort encoding"),
            Err(error) => error,
        };
        assert!(Object::is(&error, sentinel.as_ref()));

        let rgba = options_with("output", &JsValue::from_str("rgba"));
        let noop = Function::new_no_args("");
        assert!(thumbnail_png_to_chunks(PNG_INPUT, &noop, rgba.as_ref()).is_err());
    }

    #[wasm_bindgen_test]
    fn reports_invalid_browser_options() {
        if thumbnail_png(PNG_INPUT, &JsValue::from_str("invalid")).is_ok() {
            panic!("non-object options must fail");
        }
    }

    #[wasm_bindgen_test]
    fn creates_raw_rgba_output() {
        let options = Object::new();
        for (name, value) in [
            ("maxWidth", JsValue::from_f64(8.0)),
            ("maxHeight", JsValue::from_f64(8.0)),
            ("output", JsValue::from_str("rgba")),
        ] {
            Reflect::set(options.as_ref(), &JsValue::from_str(name), &value)
                .expect("the test options object must be writable");
        }

        let result = thumbnail_png(PNG_INPUT, options.as_ref()).expect("thumbnail must succeed");
        assert_eq!(result.width(), 8);
        assert_eq!(result.height(), 8);
        assert_eq!(result.mime_type(), "application/octet-stream");
        assert_eq!(result.format(), "rgba");
        assert_eq!(result.bytes().len(), 8 * 8 * 4);
    }

    #[wasm_bindgen_test]
    fn creates_centered_cover_output() {
        let options = Object::new();
        for (name, value) in [
            ("maxWidth", JsValue::from_f64(8.0)),
            ("maxHeight", JsValue::from_f64(4.0)),
            ("fit", JsValue::from_str("cover")),
            ("output", JsValue::from_str("rgba")),
        ] {
            Reflect::set(options.as_ref(), &JsValue::from_str(name), &value)
                .expect("the test options object must be writable");
        }

        let result = thumbnail_png(PNG_INPUT, options.as_ref()).expect("thumbnail must succeed");
        assert_eq!(result.width(), 8);
        assert_eq!(result.height(), 4);
        assert_eq!(result.bytes().len(), 8 * 4 * 4);
    }

    #[wasm_bindgen_test]
    fn reports_resource_limit_errors() {
        let input_limit = options_with(
            "maxInputBytes",
            &JsValue::from_f64((PNG_INPUT.len() - 1) as f64),
        );
        assert!(thumbnail_png(PNG_INPUT, input_limit.as_ref()).is_err());

        let memory_limit = options_with("maxMemoryBytes", &JsValue::from_f64(1.0));
        assert!(thumbnail_png(PNG_INPUT, memory_limit.as_ref()).is_err());
    }

    #[wasm_bindgen_test]
    fn reports_invalid_option_values() {
        let cases = [
            ("maxWidth", JsValue::from_f64(0.0)),
            ("maxHeight", JsValue::from_f64(1.5)),
            ("maxInputPixels", JsValue::from_f64(-1.0)),
            ("maxOutputPixels", JsValue::from_str("many")),
            ("allowUpscale", JsValue::from_str("true")),
            ("fit", JsValue::from_str("crop")),
            ("filter", JsValue::from_str("nearest")),
            ("output", JsValue::from_str("webp")),
        ];

        for (name, value) in cases {
            let options = options_with(name, &value);
            assert!(
                thumbnail_png(PNG_INPUT, options.as_ref()).is_err(),
                "{name} must reject its invalid test value"
            );
        }

        for (name, value) in [
            ("color", "indexed8"),
            ("compression", "maximum"),
            ("filter", "predictive"),
        ] {
            let options = Object::new();
            let png = options_with(name, &JsValue::from_str(value));
            Reflect::set(options.as_ref(), &JsValue::from_str("png"), png.as_ref())
                .expect("the thumbnail options object must be writable");
            assert!(
                thumbnail_png(PNG_INPUT, options.as_ref()).is_err(),
                "png.{name} must reject its invalid test value"
            );
        }

        let rgba = options_with("output", &JsValue::from_str("rgba"));
        Reflect::set(
            rgba.as_ref(),
            &JsValue::from_str("png"),
            Object::new().as_ref(),
        )
        .expect("the thumbnail options object must be writable");
        assert!(thumbnail_png(PNG_INPUT, rgba.as_ref()).is_err());

        let png = Object::new();
        Reflect::set(
            png.as_ref(),
            &JsValue::from_str("jpeg"),
            Object::new().as_ref(),
        )
        .expect("the thumbnail options object must be writable");
        assert!(thumbnail_png(PNG_INPUT, png.as_ref()).is_err());

        let jpeg_output = options_with("output", &JsValue::from_str("jpeg"));
        let jpeg = options_with("quality", &JsValue::from_f64(0.0));
        Reflect::set(
            jpeg_output.as_ref(),
            &JsValue::from_str("jpeg"),
            jpeg.as_ref(),
        )
        .expect("the thumbnail options object must be writable");
        assert!(thumbnail_png(PNG_INPUT, jpeg_output.as_ref()).is_err());
    }
}
