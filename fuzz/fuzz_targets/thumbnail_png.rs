#![no_main]

use std::hint::black_box;

use libfuzzer_sys::fuzz_target;
use streamthumb_core::{OutputFormat, ThumbnailOptions};
use streamthumb_png::{
    PngColorMode, PngCompression, PngFilter, PngOptions, thumbnail_png,
    thumbnail_png_with_encoder_options,
};

fuzz_target!(|data: &[u8]| {
    let output = if data.last().is_some_and(|byte| byte & 1 == 0) {
        OutputFormat::Rgba
    } else {
        OutputFormat::Png
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
    let result = if output == OutputFormat::Png {
        thumbnail_png_with_encoder_options(data, &options, &png_options)
    } else {
        thumbnail_png(data, &options)
    };
    let _ = black_box(result);
});
