//! Row-streaming PNG decoding for `streamthumb`.

mod decoder;
mod encoder;
mod error;
mod options;
mod output;

pub use decoder::{
    DecodedPngInfo, PngInputColorType, PngInputInfo, PngThumbnailPlan, RgbaRow, decode_png_rows,
    decode_png_rows_from_reader, preflight_thumbnail_png, preflight_thumbnail_png_from_reader,
    preflight_thumbnail_png_from_reader_to_writer_with_buffer,
    preflight_thumbnail_png_to_writer_with_buffer, thumbnail_jpeg_from_reader_to_writer,
    thumbnail_jpeg_from_reader_to_writer_with_options,
    thumbnail_jpeg_from_reader_to_writer_with_options_and_buffer, thumbnail_jpeg_to_writer,
    thumbnail_jpeg_to_writer_with_options, thumbnail_jpeg_to_writer_with_options_and_buffer,
    thumbnail_png, thumbnail_png_from_reader, thumbnail_png_from_reader_to_writer,
    thumbnail_png_from_reader_to_writer_with_encoder_options,
    thumbnail_png_from_reader_to_writer_with_encoder_options_and_buffer,
    thumbnail_png_from_reader_with_encoder_options, thumbnail_png_from_reader_with_jpeg_options,
    thumbnail_png_rgba, thumbnail_png_rgba_from_reader, thumbnail_png_to_writer,
    thumbnail_png_to_writer_with_encoder_options,
    thumbnail_png_to_writer_with_encoder_options_and_buffer, thumbnail_png_with_encoder_options,
    thumbnail_png_with_jpeg_options,
};
pub use error::{Error, Result, UnsupportedFeature};
pub use options::{PngColorMode, PngCompression, PngFilter, PngOptions};
pub use output::ThumbnailOutput;
pub use streamthumb_encode::{JpegOptions, JpegSubsampling};
