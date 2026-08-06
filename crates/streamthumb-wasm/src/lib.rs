//! Runtime-neutral WebAssembly bindings for `streamthumb`.

use js_sys::Reflect;
use streamthumb_core::{Filter, Fit, OutputFormat, ThumbnailOptions};
use streamthumb_png::{ThumbnailOutput, thumbnail_png as create_thumbnail};
use wasm_bindgen::prelude::*;

/// Returns the package version for bootstrap and packaging checks.
#[wasm_bindgen(js_name = streamthumbVersion)]
pub fn streamthumb_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

/// Creates a bounded PNG or RGBA thumbnail from encoded PNG bytes.
#[wasm_bindgen(js_name = thumbnailPng)]
pub fn thumbnail_png(input: &[u8], options: &JsValue) -> Result<ThumbnailResult, JsError> {
    let options = parse_options(options)?;
    let output =
        create_thumbnail(input, &options).map_err(|error| JsError::new(&error.to_string()))?;
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

fn parse_options(value: &JsValue) -> Result<ThumbnailOptions, JsError> {
    let mut options = ThumbnailOptions::default();
    if value.is_null() || value.is_undefined() {
        return Ok(options);
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
