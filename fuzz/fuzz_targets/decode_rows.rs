#![no_main]

use std::hint::black_box;

use libfuzzer_sys::fuzz_target;
use streamthumb_core::{OutputFormat, ThumbnailOptions};
use streamthumb_png::decode_png_rows;

fuzz_target!(|data: &[u8]| {
    let mut options = ThumbnailOptions {
        max_width: 64,
        max_height: 64,
        output: OutputFormat::Rgba,
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

    let _ = decode_png_rows(data, &options, |row| {
        black_box(row.y);
        black_box(row.pixels);
        Ok(())
    });
});
