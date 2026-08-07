#![no_main]

use std::hint::black_box;

use libfuzzer_sys::fuzz_target;
use streamthumb_core::{AreaDownsampler, Dimensions, Fit, SparseAreaDownsampler};

fuzz_target!(|data: &[u8]| {
    if data.len() < 5 {
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
    let fit = if data[4] & 1 == 0 {
        Fit::Contain
    } else {
        Fit::Cover
    };
    let Ok(mut downsampler) = AreaDownsampler::new_with_fit(source, output, fit) else {
        return;
    };
    let Ok(mut sparse) = SparseAreaDownsampler::new_with_fit(source, output, fit) else {
        return;
    };

    let row_len = source.width as usize * 4;
    let mut row = vec![0_u8; row_len];
    let samples = &data[5..];
    for y in 0..source.height {
        if !samples.is_empty() {
            for (index, value) in row.iter_mut().enumerate() {
                *value = samples[(index + y as usize) % samples.len()];
            }
        }
        if downsampler.push_row(y, &row).is_err() {
            return;
        }
        for x in (0..source.width).rev() {
            let offset = x as usize * 4;
            let Ok(pixel) = row[offset..offset + 4].try_into() else {
                return;
            };
            if sparse.push_pixel(x, y, pixel).is_err() {
                return;
            }
        }
    }
    let ordered = downsampler.finish();
    let arbitrary = sparse.finish();
    assert_eq!(ordered, arbitrary);
    let _ = black_box(ordered);
});
