use jpeg_encoder::{ColorType, Encoder, SamplingFactor};

const WIDTH: u16 = 512;
const HEIGHT: u16 = 512;

pub fn encode() -> Result<Vec<u8>, jpeg_encoder::EncodingError> {
    let mut pixels = vec![0_u8; WIDTH as usize * HEIGHT as usize * 3];

    for (y, row) in pixels.chunks_exact_mut(WIDTH as usize * 3).enumerate() {
        fill_row(row, y as u32);
    }

    let mut output = Vec::new();
    let mut encoder = Encoder::new(&mut output, 85);
    encoder.set_sampling_factor(SamplingFactor::F_2_2);
    encoder.encode(&pixels, WIDTH, HEIGHT, ColorType::Rgb)?;
    Ok(output)
}

fn fill_row(row: &mut [u8], y: u32) {
    for (x, pixel) in row.chunks_exact_mut(3).enumerate() {
        let x = x as u32;
        pixel[0] = ((x * 13 + y * 3) & 0xff) as u8;
        pixel[1] = ((x * 5 + y * 11) & 0xff) as u8;
        pixel[2] = ((x * 7 + y * 17) & 0xff) as u8;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn encode_512() -> usize {
    encode().map_or(0, |output| output.len())
}

#[cfg(test)]
mod tests {
    use super::encode;

    #[test]
    fn whole_image_output_is_baseline_sequential() {
        let output = encode().expect("the spike image must encode");
        assert!(output.starts_with(&[0xff, 0xd8]));
        assert!(output.ends_with(&[0xff, 0xd9]));
        assert!(output.windows(2).any(|marker| marker == [0xff, 0xc0]));
        assert!(!output.windows(2).any(|marker| marker == [0xff, 0xc2]));
    }
}
