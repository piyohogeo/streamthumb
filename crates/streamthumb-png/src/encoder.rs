use std::io::Write;

use streamthumb_core::{Dimensions, Error as CoreError, OutputFormat, RgbaRowSink};
use streamthumb_encode::{BoundedWriter, BufferedOutput, ExternalOutput, OutputTarget};

#[cfg(test)]
use streamthumb_core::RgbaImage;

use crate::{Error, PngCompression, PngFilter, PngOptions, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EncoderColor {
    Rgba8,
    Rgb8,
    GrayscaleAlpha8,
    Grayscale8,
}

impl EncoderColor {
    const fn png_color_type(self) -> png::ColorType {
        match self {
            Self::Rgba8 => png::ColorType::Rgba,
            Self::Rgb8 => png::ColorType::Rgb,
            Self::GrayscaleAlpha8 => png::ColorType::GrayscaleAlpha,
            Self::Grayscale8 => png::ColorType::Grayscale,
        }
    }

    const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba8 => 4,
            Self::Rgb8 => 3,
            Self::GrayscaleAlpha8 => 2,
            Self::Grayscale8 => 1,
        }
    }
}

#[cfg(test)]
pub(crate) fn encode_rgba_png(image: &RgbaImage, byte_limit: usize) -> Result<Vec<u8>> {
    let mut sink = PngRowSink::new(
        image.dimensions,
        byte_limit,
        EncoderColor::Rgba8,
        PngOptions::default(),
    )?;
    let row_bytes = rgba_row_bytes(image.dimensions)?;
    for (y, row) in image.pixels.chunks_exact(row_bytes).enumerate() {
        sink.push_row(
            u32::try_from(y).map_err(|_| CoreError::IntegerOverflow {
                operation: "PNG encoder row index",
            })?,
            row,
        )?;
    }
    sink.finish()
}

pub(crate) type PngRowSink = PngEncoderRowSink<BufferedOutput>;
pub(crate) type PngWriterRowSink<W> = PngEncoderRowSink<ExternalOutput<W>>;

pub(crate) struct PngEncoderRowSink<T: OutputTarget + 'static> {
    dimensions: Dimensions,
    next_y: u32,
    stream: png::StreamWriter<'static, BoundedWriter<T>>,
    output: BoundedWriter<T>,
    color: EncoderColor,
    converted_row: Vec<u8>,
}

impl PngEncoderRowSink<BufferedOutput> {
    pub(crate) fn new(
        dimensions: Dimensions,
        byte_limit: usize,
        color: EncoderColor,
        options: PngOptions,
    ) -> Result<Self> {
        let output = BoundedWriter::buffered(byte_limit, OutputFormat::Png)?;
        Self::with_output(dimensions, color, options, output)
    }
}

impl<W: Write + 'static> PngEncoderRowSink<ExternalOutput<W>> {
    pub(crate) fn with_writer(
        dimensions: Dimensions,
        byte_limit: usize,
        color: EncoderColor,
        options: PngOptions,
        writer: W,
    ) -> Result<Self> {
        let output = BoundedWriter::external(writer, byte_limit, OutputFormat::Png)?;
        Self::with_output(dimensions, color, options, output)
    }
}

impl<T: OutputTarget + 'static> PngEncoderRowSink<T> {
    fn with_output(
        dimensions: Dimensions,
        color: EncoderColor,
        options: PngOptions,
        output: BoundedWriter<T>,
    ) -> Result<Self> {
        let mut encoder = png::Encoder::new(output.clone(), dimensions.width, dimensions.height);
        encoder.set_color(color.png_color_type());
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(map_compression(options.compression));
        if let Some(filter) = map_filter(options.filter) {
            encoder.set_filter(filter);
        }
        let writer = encoder
            .write_header()
            .map_err(|error| output.map_external_error(error.to_string()))?;
        let stream = writer
            .into_stream_writer()
            .map_err(|error| output.map_external_error(error.to_string()))?;
        Ok(Self {
            dimensions,
            next_y: 0,
            stream,
            output,
            color,
            converted_row: allocate_converted_row(dimensions, color)?,
        })
    }
}

impl<T: OutputTarget + 'static> RgbaRowSink for PngEncoderRowSink<T> {
    type Output = T::Finished;
    type Error = Error;

    fn push_row(&mut self, y: u32, rgba: &[u8]) -> Result<()> {
        if y != self.next_y {
            return Err(CoreError::UnexpectedRow {
                expected: self.next_y,
                actual: y,
            }
            .into());
        }
        let expected_len = rgba_row_bytes(self.dimensions)?;
        if rgba.len() != expected_len {
            return Err(CoreError::InvalidRowLength {
                expected: expected_len,
                actual: rgba.len(),
            }
            .into());
        }
        let encoded_row = match self.color {
            EncoderColor::Rgba8 => rgba,
            EncoderColor::Rgb8 => {
                convert_rgba_row(rgba, &mut self.converted_row, 3, |pixel, output| {
                    output.copy_from_slice(&pixel[..3]);
                });
                &self.converted_row
            }
            EncoderColor::GrayscaleAlpha8 => {
                convert_rgba_row(rgba, &mut self.converted_row, 2, |pixel, output| {
                    output[0] = grayscale(pixel[0], pixel[1], pixel[2]);
                    output[1] = pixel[3];
                });
                &self.converted_row
            }
            EncoderColor::Grayscale8 => {
                convert_rgba_row(rgba, &mut self.converted_row, 1, |pixel, output| {
                    output[0] = grayscale(pixel[0], pixel[1], pixel[2]);
                });
                &self.converted_row
            }
        };
        self.stream
            .write_all(encoded_row)
            .map_err(|error| self.output.map_external_error(error.to_string()))?;
        self.next_y = self
            .next_y
            .checked_add(1)
            .ok_or(CoreError::IntegerOverflow {
                operation: "PNG encoder row index",
            })?;
        Ok(())
    }

    fn finish(self) -> Result<Self::Output> {
        if self.next_y != self.dimensions.height {
            return Err(CoreError::IncompleteImage {
                expected_rows: self.dimensions.height,
                actual_rows: self.next_y,
            }
            .into());
        }
        self.stream
            .finish()
            .map_err(|error| self.output.map_external_error(error.to_string()))?;
        self.output.into_output().map_err(Into::into)
    }
}

fn allocate_converted_row(dimensions: Dimensions, color: EncoderColor) -> Result<Vec<u8>> {
    if color == EncoderColor::Rgba8 {
        return Ok(Vec::new());
    }
    let bytes = usize::try_from(dimensions.width)
        .ok()
        .and_then(|width| width.checked_mul(color.bytes_per_pixel()))
        .ok_or(CoreError::IntegerOverflow {
            operation: "converted PNG row size",
        })?;
    let mut row = Vec::new();
    row.try_reserve_exact(bytes)
        .map_err(|_| Error::AllocationFailed { bytes })?;
    row.resize(bytes, 0);
    Ok(row)
}

fn convert_rgba_row(
    rgba: &[u8],
    output: &mut [u8],
    output_channels: usize,
    mut convert: impl FnMut(&[u8], &mut [u8]),
) {
    for (pixel, encoded) in rgba
        .chunks_exact(4)
        .zip(output.chunks_exact_mut(output_channels))
    {
        convert(pixel, encoded);
    }
}

const fn grayscale(red: u8, green: u8, blue: u8) -> u8 {
    let weighted = 77 * red as u16 + 150 * green as u16 + 29 * blue as u16 + 128;
    (weighted >> 8) as u8
}

const fn map_compression(compression: PngCompression) -> png::Compression {
    match compression {
        PngCompression::NoCompression => png::Compression::NoCompression,
        PngCompression::Fastest => png::Compression::Fastest,
        PngCompression::Fast => png::Compression::Fast,
        PngCompression::Balanced => png::Compression::Balanced,
        PngCompression::High => png::Compression::High,
    }
}

const fn map_filter(filter: PngFilter) -> Option<png::Filter> {
    match filter {
        PngFilter::Default => None,
        PngFilter::None => Some(png::Filter::NoFilter),
        PngFilter::Sub => Some(png::Filter::Sub),
        PngFilter::Up => Some(png::Filter::Up),
        PngFilter::Average => Some(png::Filter::Avg),
        PngFilter::Paeth => Some(png::Filter::Paeth),
        PngFilter::Adaptive => Some(png::Filter::Adaptive),
        PngFilter::MinEntropy => Some(png::Filter::MinEntropy),
    }
}

fn rgba_row_bytes(dimensions: Dimensions) -> Result<usize> {
    usize::try_from(dimensions.width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or_else(|| {
            CoreError::IntegerOverflow {
                operation: "PNG encoder row size",
            }
            .into()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn row_sink_rejects_out_of_order_rows() {
        let dimensions = Dimensions::new(1, 1).unwrap();
        let mut sink =
            PngRowSink::new(dimensions, 1024, EncoderColor::Rgba8, PngOptions::default()).unwrap();
        assert!(matches!(
            sink.push_row(1, &[0, 0, 0, 255]),
            Err(Error::Core(CoreError::UnexpectedRow {
                expected: 0,
                actual: 1
            }))
        ));
    }

    #[test]
    fn grayscale_conversion_uses_documented_integer_luma() {
        assert_eq!(grayscale(255, 0, 0), 77);
        assert_eq!(grayscale(0, 255, 0), 149);
        assert_eq!(grayscale(0, 0, 255), 29);
        assert_eq!(grayscale(255, 255, 255), 255);
    }
}
