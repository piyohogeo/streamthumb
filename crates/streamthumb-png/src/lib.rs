//! Row-streaming PNG decoding for `streamthumb`.

mod decoder;
mod encoder;
mod error;
mod output;

pub use decoder::{DecodedPngInfo, RgbaRow, decode_png_rows, thumbnail_png, thumbnail_png_rgba};
pub use error::{Error, Result, UnsupportedFeature};
pub use output::ThumbnailOutput;
