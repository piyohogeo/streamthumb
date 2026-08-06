use std::io::{self, Write};

use streamthumb_core::RgbaImage;

use crate::{Error, Result};

pub(crate) fn encode_rgba_png(image: &RgbaImage, byte_limit: usize) -> Result<Vec<u8>> {
    let mut output = BoundedWriter::new(byte_limit)?;
    let encode_result = (|| {
        let mut encoder =
            png::Encoder::new(&mut output, image.dimensions.width, image.dimensions.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&image.pixels)?;
        writer.finish()
    })();

    if let Err(error) = encode_result {
        if output.limit_exceeded {
            return Err(Error::EncodedOutputLimitExceeded { limit: byte_limit });
        }
        return Err(Error::EncodeFailure(error.to_string()));
    }
    Ok(output.bytes)
}

struct BoundedWriter {
    bytes: Vec<u8>,
    limit: usize,
    limit_exceeded: bool,
}

impl BoundedWriter {
    fn new(limit: usize) -> Result<Self> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(limit)
            .map_err(|_| Error::AllocationFailed { bytes: limit })?;
        Ok(Self {
            bytes,
            limit,
            limit_exceeded: false,
        })
    }
}

impl Write for BoundedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let remaining = self.limit.saturating_sub(self.bytes.len());
        if buffer.len() > remaining {
            self.limit_exceeded = true;
            return Err(io::Error::other("encoded PNG output limit exceeded"));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use streamthumb_core::Dimensions;

    #[test]
    fn encoded_output_cannot_grow_past_its_limit() {
        let image = RgbaImage {
            dimensions: Dimensions::new(1, 1).unwrap(),
            pixels: vec![1, 2, 3, 4],
        };
        assert!(matches!(
            encode_rgba_png(&image, 1),
            Err(Error::EncodedOutputLimitExceeded { limit: 1 })
        ));
    }
}
