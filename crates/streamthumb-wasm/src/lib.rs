//! Runtime-neutral WebAssembly bindings for `streamthumb`.

use js_sys::Reflect;
use streamthumb_core::{Filter, Fit, OutputFormat, ThumbnailOptions};
use streamthumb_png::{
    PngColorMode, PngCompression, PngFilter, PngOptions, ThumbnailOutput,
    thumbnail_png as create_thumbnail,
    thumbnail_png_with_encoder_options as create_thumbnail_with_png_options,
};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(typescript_custom_section)]
const THUMBNAIL_TYPES: &str = r#"
/** Fit modes supported by the bounded thumbnail pipeline. */
export type ThumbnailFit = "contain";

/** Resize filters supported by the bounded thumbnail pipeline. */
export type ThumbnailFilter = "area";

/** Output representations supported by the WebAssembly API. */
export type ThumbnailOutputFormat = "png" | "rgba";

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

/** Options and resource limits for thumbnail generation. */
export interface ThumbnailOptions {
    maxWidth?: number;
    maxHeight?: number;
    fit?: ThumbnailFit;
    filter?: ThumbnailFilter;
    allowUpscale?: boolean;
    output?: ThumbnailOutputFormat;
    png?: PngOptions;
    maxInputBytes?: number;
    maxInputWidth?: number;
    maxInputHeight?: number;
    maxInputPixels?: number;
    maxOutputWidth?: number;
    maxOutputHeight?: number;
    maxOutputPixels?: number;
    maxMemoryBytes?: number;
}

/** Creates a bounded PNG or RGBA thumbnail from encoded PNG bytes. */
export function thumbnailPng(
    input: Uint8Array,
    options?: ThumbnailOptions | null,
): ThumbnailResult;
"#;

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

/// Creates a bounded PNG or RGBA thumbnail from encoded PNG bytes.
#[wasm_bindgen(js_name = thumbnailPng, skip_typescript)]
pub fn thumbnail_png(input: &[u8], options: &JsValue) -> Result<ThumbnailResult, JsError> {
    let (options, png_options) = parse_options(options)?;
    let output = match options.output {
        OutputFormat::Png => create_thumbnail_with_png_options(input, &options, &png_options),
        OutputFormat::Rgba => create_thumbnail(input, &options),
    }
    .map_err(|error| JsError::new(&error.to_string()))?;
    ThumbnailResult::from_output(output)
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
    /// Returns a copy of the encoded PNG or raw RGBA bytes.
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
            } => Self {
                bytes,
                width,
                height,
                mime_type: mime_type.to_owned(),
                format: "png".to_owned(),
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

fn parse_options(value: &JsValue) -> Result<(ThumbnailOptions, PngOptions), JsError> {
    let mut options = ThumbnailOptions::default();
    if value.is_null() || value.is_undefined() {
        return Ok((options, PngOptions::default()));
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
            _ => return Err(JsError::new("fit must be \"contain\"")),
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
            "rgba" => OutputFormat::Rgba,
            _ => return Err(JsError::new("output must be \"png\" or \"rgba\"")),
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

    Ok((options, png_options))
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

#[cfg(all(test, target_arch = "wasm32"))]
mod browser_tests {
    use super::*;
    use js_sys::Object;
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
            ("fit", JsValue::from_str("cover")),
            ("filter", JsValue::from_str("nearest")),
            ("output", JsValue::from_str("jpeg")),
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
    }
}
