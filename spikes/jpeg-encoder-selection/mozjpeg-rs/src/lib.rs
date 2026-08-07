use mozjpeg_rs::{Encoder, Subsampling};

const WIDTH: u32 = 512;
const HEIGHT: u32 = 512;

pub fn encode() -> Result<Vec<u8>, mozjpeg_rs::Error> {
    let mut output = Vec::new();
    let mut stream = Encoder::streaming()
        .quality(85)
        .subsampling(Subsampling::S420)
        .force_baseline(true)
        .start_rgb(WIDTH, HEIGHT, &mut output)?;
    let mut row = vec![0_u8; WIDTH as usize * 3];

    for y in 0..HEIGHT {
        fill_row(&mut row, y);
        stream.write_scanlines(&row)?;
    }

    stream.finish()?;
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
    use std::io::{self, Write};

    use super::{encode, fill_row, HEIGHT, WIDTH};
    use mozjpeg_rs::{Encoder, Subsampling};

    struct LimitWriter {
        bytes: Vec<u8>,
        limit: usize,
    }

    impl Write for LimitWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if buffer.len() > self.limit.saturating_sub(self.bytes.len()) {
                return Err(io::Error::other("encoded output limit exceeded"));
            }
            self.bytes.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn streaming_output_is_baseline_sequential() {
        let output = encode().expect("the spike image must encode");
        assert!(output.starts_with(&[0xff, 0xd8]));
        assert!(output.ends_with(&[0xff, 0xd9]));
        assert!(output.windows(2).any(|marker| marker == [0xff, 0xc0]));
        assert!(!output.windows(2).any(|marker| marker == [0xff, 0xc2]));
    }

    #[test]
    fn writer_error_stops_streaming_output() {
        let writer = LimitWriter {
            bytes: Vec::new(),
            limit: 1_024,
        };
        let mut stream = Encoder::streaming()
            .quality(85)
            .subsampling(Subsampling::S420)
            .force_baseline(true)
            .start_rgb(WIDTH, HEIGHT, writer)
            .expect("the JPEG headers fit below the test limit");
        let mut row = vec![0_u8; WIDTH as usize * 3];
        let mut result = Ok(());

        for y in 0..HEIGHT {
            fill_row(&mut row, y);
            result = stream.write_scanlines(&row);
            if result.is_err() {
                break;
            }
        }

        assert!(result.is_err());
    }
}
