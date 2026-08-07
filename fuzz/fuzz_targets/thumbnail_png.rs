#![no_main]

use std::hint::black_box;

use libfuzzer_sys::fuzz_target;
use streamthumb_core::{OutputFormat, ThumbnailOptions};
use streamthumb_png::{
    JpegOptions, JpegSubsampling, PngColorMode, PngCompression, PngFilter, PngOptions,
    thumbnail_png, thumbnail_png_with_encoder_options, thumbnail_png_with_jpeg_options,
};

fuzz_target!(|data: &[u8]| {
    let output = match data.last().copied().unwrap_or_default() % 3 {
        0 => OutputFormat::Rgba,
        1 => OutputFormat::Png,
        _ => OutputFormat::Jpeg,
    };
    let mut options = ThumbnailOptions {
        max_width: 64,
        max_height: 64,
        output,
        ..ThumbnailOptions::default()
    };
    options.limits.max_input_bytes = 1024 * 1024;
    options.limits.max_width = 4_096;
    options.limits.max_height = 4_096;
    options.limits.max_pixels = 1_048_576;
    options.limits.max_output_width = 64;
    options.limits.max_output_height = 64;
    options.limits.max_output_pixels = 4_096;
    options.limits.max_working_memory_bytes = 8 * 1024 * 1024;

    let png_options = PngOptions {
        color: match data.first().copied().unwrap_or_default() % 5 {
            0 => PngColorMode::Auto,
            1 => PngColorMode::Rgba8,
            2 => PngColorMode::Rgb8,
            3 => PngColorMode::GrayscaleAlpha8,
            _ => PngColorMode::Grayscale8,
        },
        compression: match data.get(1).copied().unwrap_or_default() % 5 {
            0 => PngCompression::NoCompression,
            1 => PngCompression::Fastest,
            2 => PngCompression::Fast,
            3 => PngCompression::Balanced,
            _ => PngCompression::High,
        },
        filter: match data.get(2).copied().unwrap_or_default() % 8 {
            0 => PngFilter::Default,
            1 => PngFilter::None,
            2 => PngFilter::Sub,
            3 => PngFilter::Up,
            4 => PngFilter::Average,
            5 => PngFilter::Paeth,
            6 => PngFilter::Adaptive,
            _ => PngFilter::MinEntropy,
        },
    };
    let jpeg_options = JpegOptions {
        quality: data.get(3).copied().unwrap_or_default() % 100 + 1,
        background: [
            data.get(4).copied().unwrap_or(255),
            data.get(5).copied().unwrap_or(255),
            data.get(6).copied().unwrap_or(255),
        ],
        subsampling: match data.get(7).copied().unwrap_or_default() % 3 {
            0 => JpegSubsampling::S420,
            1 => JpegSubsampling::S422,
            _ => JpegSubsampling::S444,
        },
    };
    let result = match output {
        OutputFormat::Png => thumbnail_png_with_encoder_options(data, &options, &png_options),
        OutputFormat::Jpeg => thumbnail_png_with_jpeg_options(data, &options, &jpeg_options),
        OutputFormat::Rgba => thumbnail_png(data, &options),
    };
    let _ = black_box(result);
});
