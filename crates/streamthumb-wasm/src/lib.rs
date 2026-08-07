//! Runtime-neutral WebAssembly bindings for `streamthumb`.

use std::{
    cell::RefCell,
    io::{self, Write},
    rc::Rc,
};

use js_sys::{Function, Reflect, Uint8Array};
use streamthumb_core::{Filter, Fit, OutputFormat, ThumbnailOptions};
use streamthumb_png::{
    JpegOptions, JpegSubsampling, PngColorMode, PngCompression, PngFilter, PngOptions,
    ThumbnailOutput, thumbnail_jpeg_to_writer_with_options_and_buffer,
    thumbnail_png as create_thumbnail, thumbnail_png_to_writer_with_encoder_options_and_buffer,
    thumbnail_png_with_encoder_options as create_thumbnail_with_png_options,
    thumbnail_png_with_jpeg_options as create_thumbnail_with_jpeg_options,
};
#[cfg(target_arch = "wasm32")]
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

/** Creates a bounded PNG, JPEG, or RGBA thumbnail from encoded PNG bytes. */
export function thumbnailPng(
    input: Uint8Array,
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

/// Creates a bounded PNG, JPEG, or RGBA thumbnail from encoded PNG bytes.
#[wasm_bindgen(js_name = thumbnailPng, skip_typescript)]
pub fn thumbnail_png(input: &[u8], options: &JsValue) -> Result<ThumbnailResult, JsError> {
    let (options, png_options, jpeg_options) = parse_options(options)?;
    let output = match options.output {
        OutputFormat::Png => create_thumbnail_with_png_options(input, &options, &png_options),
        OutputFormat::Jpeg => create_thumbnail_with_jpeg_options(input, &options, &jpeg_options),
        OutputFormat::Rgba => create_thumbnail(input, &options),
    }
    .map_err(|error| JsError::new(&error.to_string()))?;
    ThumbnailResult::from_output(output)
}

/// Creates encoded output and forwards owned chunks to a JavaScript callback.
#[wasm_bindgen(js_name = thumbnailPngToChunks, skip_typescript)]
pub fn thumbnail_png_to_chunks(
    input: &[u8],
    on_chunk: &Function,
    options: &JsValue,
) -> Result<ChunkedThumbnailResult, JsValue> {
    let (options, png_options, jpeg_options) = parse_options(options).map_err(JsValue::from)?;
    if options.output == OutputFormat::Rgba {
        return Err(JsError::new("chunk output requires PNG or JPEG output").into());
    }

    let callback_error = Rc::new(RefCell::new(None));
    let stats = Rc::new(RefCell::new(ChunkStats::default()));
    let writer = ChunkCallbackWriter::new({
        let on_chunk = on_chunk.clone();
        let callback_error = Rc::clone(&callback_error);
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
                    *callback_error.borrow_mut() = Some(error);
                    Err(io::Error::other("JavaScript chunk callback failed"))
                }
            }
        }
    })
    .map_err(|error| JsError::new(&error.to_string()))?;
    let finalizer = writer.clone();

    let result = match options.output {
        OutputFormat::Png => thumbnail_png_to_writer_with_encoder_options_and_buffer(
            input,
            &options,
            &png_options,
            OUTPUT_CHUNK_BYTES,
            writer,
        ),
        OutputFormat::Jpeg => thumbnail_jpeg_to_writer_with_options_and_buffer(
            input,
            &options,
            &jpeg_options,
            OUTPUT_CHUNK_BYTES,
            writer,
        ),
        OutputFormat::Rgba => unreachable!("raw RGBA output was rejected above"),
    };
    if result.is_ok() {
        if let Err(error) = finalizer.finish() {
            if let Some(callback_error) = callback_error.borrow_mut().take() {
                return Err(callback_error);
            }
            return Err(JsError::new(&error.to_string()).into());
        }
    }
    let info = match result {
        Ok(info) => info,
        Err(error) => {
            if let Some(callback_error) = callback_error.borrow_mut().take() {
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
        })
        .transpose()
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
    fn chunk_callback_errors_and_raw_output_are_rejected() {
        let throwing = Function::new_no_args("throw new Error('injected chunk callback failure')");
        assert!(thumbnail_png_to_chunks(PNG_INPUT, &throwing, &JsValue::NULL).is_err());

        let rgba = options_with("output", &JsValue::from_str("rgba"));
        let noop = Function::new_no_args("");
        assert!(thumbnail_png_to_chunks(PNG_INPUT, &noop, rgba.as_ref()).is_err());
    }

    #[wasm_bindgen_test]
    fn reports_invalid_browser_options() {
        match thumbnail_png(PNG_INPUT, &JsValue::from_str("invalid")) {
            Ok(_) => panic!("non-object options must fail"),
            Err(_) => {}
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
