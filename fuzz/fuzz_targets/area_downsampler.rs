#![no_main]

use std::hint::black_box;

use libfuzzer_sys::fuzz_target;
use streamthumb_core::{AreaDownsampler, Dimensions};

fuzz_target!(|data: &[u8]| {
    if data.len() < 4 {
        return;
    }
    let Ok(source) = Dimensions::new(u32::from(data[0] % 64) + 1, u32::from(data[1] % 64) + 1)
    else {
        return;
    };
    let Ok(output) = Dimensions::new(u32::from(data[2] % 32) + 1, u32::from(data[3] % 32) + 1)
    else {
        return;
    };
    let Ok(mut downsampler) = AreaDownsampler::new(source, output) else {
        return;
    };

    let row_len = source.width as usize * 4;
    let mut row = vec![0_u8; row_len];
    let samples = &data[4..];
    for y in 0..source.height {
        if !samples.is_empty() {
            for (index, value) in row.iter_mut().enumerate() {
                *value = samples[(index + y as usize) % samples.len()];
            }
        }
        if downsampler.push_row(y, &row).is_err() {
            return;
        }
    }
    let _ = black_box(downsampler.finish());
});
