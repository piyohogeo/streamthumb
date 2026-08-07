//! Row-streaming PNG decoding for `streamthumb`.

mod decoder;
mod encoder;
mod error;
mod options;
mod output;

pub use decoder::{
    DecodedPngInfo, RgbaRow, decode_png_rows, thumbnail_png, thumbnail_png_rgba,
    thumbnail_png_with_encoder_options, thumbnail_png_with_jpeg_options,
};
pub use error::{Error, Result, UnsupportedFeature};
pub use options::{PngColorMode, PngCompression, PngFilter, PngOptions};
pub use output::ThumbnailOutput;
pub use streamthumb_encode::{JpegOptions, JpegSubsampling};
