use std::io::Write;

use jpeg_encoder::{ColorType, Encoder, SamplingFactor};
use streamthumb_core::{Dimensions, Error as CoreError, OutputFormat, RgbaRowSink};

use crate::{BoundedWriter, BufferedOutput, Error, ExternalOutput, OutputTarget, Result};

/// Chroma resolution used by the JPEG encoder.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum JpegSubsampling {
    /// Chroma at half horizontal and vertical resolution.
    #[default]
    S420,
    /// Chroma at half horizontal resolution and full vertical resolution.
    S422,
    /// Full-resolution chroma.
    S444,
}

/// Settings used only when the requested output representation is JPEG.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JpegOptions {
    /// Lossy quality from 1 through 100.
    pub quality: u8,
    /// RGB background used to composite straight-alpha input.
    pub background: [u8; 3],
    /// Chroma resolution written to the JPEG frame.
    pub subsampling: JpegSubsampling,
}

impl Default for JpegOptions {
    fn default() -> Self {
        Self {
            quality: 85,
            background: [255, 255, 255],
            subsampling: JpegSubsampling::S420,
        }
    }
}

impl JpegOptions {
    fn validate(self) -> Result<Self> {
        if self.quality == 0 || self.quality > 100 {
            return Err(Error::InvalidJpegOptions(
                "quality must be between 1 and 100",
            ));
        }
        Ok(self)
    }
}

/// Composites straight-alpha RGBA8 rows and encodes baseline sequential JPEG.
///
/// Each buffered MCU row is encoded independently and joined with JPEG restart
/// markers. This bounds raw encoder input to 8 or 16 rows while retaining a
/// standards-compliant single-image bitstream.
pub struct JpegRowSink {
    encoder: JpegEncoder<BufferedOutput>,
}

/// A JPEG row sink that forwards encoded bytes to a caller-owned writer.
pub struct JpegWriterRowSink<W: Write> {
    encoder: JpegEncoder<ExternalOutput<W>>,
}

struct JpegEncoder<T: OutputTarget> {
    dimensions: Dimensions,
    next_y: u32,
    output: BoundedWriter<T>,
    byte_limit: usize,
    rgb_rows: Vec<u8>,
    lines_in_buffer: usize,
    options: JpegOptions,
    segment_count: u32,
}

impl JpegRowSink {
    pub fn new(dimensions: Dimensions, byte_limit: usize, options: JpegOptions) -> Result<Self> {
        let output = BoundedWriter::buffered(byte_limit, OutputFormat::Jpeg)?;
        Ok(Self {
            encoder: JpegEncoder::new(dimensions, byte_limit, options, output)?,
        })
    }
}

impl<W: Write> JpegWriterRowSink<W> {
    pub fn new(
        dimensions: Dimensions,
        byte_limit: usize,
        options: JpegOptions,
        writer: W,
    ) -> Result<Self> {
        let output = BoundedWriter::external(writer, byte_limit, OutputFormat::Jpeg)?;
        Ok(Self {
            encoder: JpegEncoder::new(dimensions, byte_limit, options, output)?,
        })
    }
}

impl<T: OutputTarget> JpegEncoder<T> {
    fn new(
        dimensions: Dimensions,
        byte_limit: usize,
        options: JpegOptions,
        output: BoundedWriter<T>,
    ) -> Result<Self> {
        let options = options.validate()?;
        if dimensions.width > u32::from(u16::MAX) || dimensions.height > u32::from(u16::MAX) {
            return Err(Error::InvalidJpegOptions(
                "output dimensions must not exceed 65535 pixels",
            ));
        }
        let buffer_bytes = usize::try_from(dimensions.width)
            .ok()
            .and_then(|width| width.checked_mul(3))
            .and_then(|row| row.checked_mul(mcu_height(options.subsampling)))
            .ok_or(CoreError::IntegerOverflow {
                operation: "JPEG MCU row buffer size",
            })?;
        let mut rgb_rows = Vec::new();
        rgb_rows
            .try_reserve_exact(buffer_bytes)
            .map_err(|_| Error::AllocationFailed {
                bytes: buffer_bytes,
            })?;
        rgb_rows.resize(buffer_bytes, 0);
        Ok(Self {
            dimensions,
            next_y: 0,
            output,
            byte_limit,
            rgb_rows,
            lines_in_buffer: 0,
            options,
            segment_count: 0,
        })
    }

    fn encode_buffered_mcu_row(&mut self) -> Result<()> {
        let width = u16::try_from(self.dimensions.width).map_err(|_| {
            Error::InvalidJpegOptions("output width exceeds the JPEG baseline limit")
        })?;
        let height = u16::try_from(self.lines_in_buffer).map_err(|_| {
            Error::InvalidJpegOptions("MCU row height exceeds the JPEG baseline limit")
        })?;
        let row_bytes = usize::from(width) * 3;
        let pixels = &self.rgb_rows[..row_bytes * self.lines_in_buffer];
        let temporary = BoundedWriter::buffered(self.byte_limit, OutputFormat::Jpeg)?;
        {
            let mut encoder = Encoder::new(temporary.clone(), self.options.quality);
            encoder.set_sampling_factor(map_subsampling(self.options.subsampling));
            encoder
                .encode(pixels, width, height, ColorType::Rgb)
                .map_err(|error| temporary.map_external_error(error.to_string()))?;
        }
        let segment = temporary.into_output()?;
        let parts = parse_segment(&segment)?;

        if self.segment_count == 0 {
            let mut header = parts.header.to_vec();
            set_sof_height(&mut header, self.dimensions.height)?;
            let restart_interval = mcus_per_row(self.dimensions.width, self.options.subsampling)?;
            self.output
                .write_all(&header[..parts.sos_offset])
                .map_err(|error| self.output.map_external_error(error.to_string()))?;
            self.output
                .write_all(&[
                    0xff,
                    0xdd,
                    0x00,
                    0x04,
                    (restart_interval >> 8) as u8,
                    restart_interval as u8,
                ])
                .map_err(|error| self.output.map_external_error(error.to_string()))?;
            self.output
                .write_all(&header[parts.sos_offset..])
                .map_err(|error| self.output.map_external_error(error.to_string()))?;
        } else {
            let restart = 0xd0 + ((self.segment_count - 1) & 7) as u8;
            self.output
                .write_all(&[0xff, restart])
                .map_err(|error| self.output.map_external_error(error.to_string()))?;
        }
        self.output
            .write_all(parts.entropy)
            .map_err(|error| self.output.map_external_error(error.to_string()))?;
        self.segment_count += 1;
        self.lines_in_buffer = 0;
        Ok(())
    }
}

impl<T: OutputTarget> JpegEncoder<T> {
    fn push_row(&mut self, y: u32, rgba: &[u8]) -> Result<()> {
        if y != self.next_y {
            return Err(CoreError::UnexpectedRow {
                expected: self.next_y,
                actual: y,
            }
            .into());
        }
        let width =
            usize::try_from(self.dimensions.width).map_err(|_| CoreError::IntegerOverflow {
                operation: "JPEG output width",
            })?;
        let expected_len = width.checked_mul(4).ok_or(CoreError::IntegerOverflow {
            operation: "JPEG RGBA input row size",
        })?;
        if rgba.len() != expected_len {
            return Err(CoreError::InvalidRowLength {
                expected: expected_len,
                actual: rgba.len(),
            }
            .into());
        }

        let rgb_start = self.lines_in_buffer * width * 3;
        let rgb_end = rgb_start + width * 3;
        for (pixel, rgb) in rgba
            .chunks_exact(4)
            .zip(self.rgb_rows[rgb_start..rgb_end].chunks_exact_mut(3))
        {
            composite_pixel(pixel, self.options.background, rgb);
        }
        self.lines_in_buffer += 1;
        self.next_y = self
            .next_y
            .checked_add(1)
            .ok_or(CoreError::IntegerOverflow {
                operation: "JPEG encoder row index",
            })?;
        if self.lines_in_buffer == mcu_height(self.options.subsampling) {
            self.encode_buffered_mcu_row()?;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<T::Finished> {
        if self.next_y != self.dimensions.height {
            return Err(CoreError::IncompleteImage {
                expected_rows: self.dimensions.height,
                actual_rows: self.next_y,
            }
            .into());
        }
        if self.lines_in_buffer > 0 {
            self.encode_buffered_mcu_row()?;
        }
        self.output
            .write_all(&[0xff, 0xd9])
            .map_err(|error| self.output.map_external_error(error.to_string()))?;
        self.output
            .flush()
            .map_err(|error| self.output.map_external_error(error.to_string()))?;
        self.output.into_output()
    }
}

impl RgbaRowSink for JpegRowSink {
    type Output = Vec<u8>;
    type Error = Error;

    fn push_row(&mut self, y: u32, rgba: &[u8]) -> Result<()> {
        self.encoder.push_row(y, rgba)
    }

    fn finish(self) -> Result<Self::Output> {
        self.encoder.finish()
    }
}

impl<W: Write> RgbaRowSink for JpegWriterRowSink<W> {
    type Output = ();
    type Error = Error;

    fn push_row(&mut self, y: u32, rgba: &[u8]) -> Result<()> {
        self.encoder.push_row(y, rgba)
    }

    fn finish(self) -> Result<Self::Output> {
        self.encoder.finish()
    }
}

struct SegmentParts<'a> {
    header: &'a [u8],
    sos_offset: usize,
    entropy: &'a [u8],
}

fn parse_segment(bytes: &[u8]) -> Result<SegmentParts<'_>> {
    if !bytes.starts_with(&[0xff, 0xd8]) || !bytes.ends_with(&[0xff, 0xd9]) {
        return Err(jpeg_structure_error(
            "temporary encoder produced invalid boundaries",
        ));
    }
    let mut offset = 2;
    while offset + 4 <= bytes.len() - 2 {
        if bytes[offset] != 0xff {
            return Err(jpeg_structure_error(
                "temporary encoder produced an invalid marker",
            ));
        }
        let marker = bytes[offset + 1];
        let length = usize::from(u16::from_be_bytes([bytes[offset + 2], bytes[offset + 3]]));
        if length < 2 || offset + 2 + length > bytes.len() - 2 {
            return Err(jpeg_structure_error(
                "temporary encoder produced an invalid marker length",
            ));
        }
        if marker == 0xda {
            let entropy_start = offset + 2 + length;
            return Ok(SegmentParts {
                header: &bytes[..entropy_start],
                sos_offset: offset,
                entropy: &bytes[entropy_start..bytes.len() - 2],
            });
        }
        offset += 2 + length;
    }
    Err(jpeg_structure_error(
        "temporary encoder omitted the SOS marker",
    ))
}

fn set_sof_height(header: &mut [u8], height: u32) -> Result<()> {
    let height = u16::try_from(height)
        .map_err(|_| Error::InvalidJpegOptions("output height exceeds the JPEG baseline limit"))?;
    let sof = header
        .windows(2)
        .position(|marker| marker == [0xff, 0xc0])
        .ok_or_else(|| jpeg_structure_error("temporary encoder omitted the SOF0 marker"))?;
    let target = header.get_mut(sof + 5..sof + 7).ok_or_else(|| {
        jpeg_structure_error("temporary encoder produced a truncated SOF0 marker")
    })?;
    target.copy_from_slice(&height.to_be_bytes());
    Ok(())
}

fn mcus_per_row(width: u32, subsampling: JpegSubsampling) -> Result<u16> {
    let mcu_width = match subsampling {
        JpegSubsampling::S420 | JpegSubsampling::S422 => 16,
        JpegSubsampling::S444 => 8,
    };
    u16::try_from(width.div_ceil(mcu_width))
        .map_err(|_| Error::InvalidJpegOptions("restart interval exceeds the JPEG limit"))
}

const fn mcu_height(subsampling: JpegSubsampling) -> usize {
    match subsampling {
        JpegSubsampling::S420 => 16,
        JpegSubsampling::S422 | JpegSubsampling::S444 => 8,
    }
}

const fn map_subsampling(subsampling: JpegSubsampling) -> SamplingFactor {
    match subsampling {
        JpegSubsampling::S420 => SamplingFactor::F_2_2,
        JpegSubsampling::S422 => SamplingFactor::F_2_1,
        JpegSubsampling::S444 => SamplingFactor::F_1_1,
    }
}

fn composite_pixel(rgba: &[u8], background: [u8; 3], output: &mut [u8]) {
    let alpha = u32::from(rgba[3]);
    let inverse = 255 - alpha;
    for channel in 0..3 {
        let blended =
            u32::from(rgba[channel]) * alpha + u32::from(background[channel]) * inverse + 127;
        output[channel] = (blended / 255) as u8;
    }
}

fn jpeg_structure_error(message: &'static str) -> Error {
    Error::EncodeFailure {
        format: OutputFormat::Jpeg,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use jpeg_decoder::{Decoder, PixelFormat};

    use super::*;

    fn encode_solid(pixel: [u8; 4], options: JpegOptions) -> Vec<u8> {
        let dimensions = Dimensions::new(17, 33).unwrap();
        let mut sink = JpegRowSink::new(dimensions, 128 * 1024, options).unwrap();
        let row: Vec<u8> = pixel.into_iter().cycle().take(17 * 4).collect();
        for y in 0..33 {
            sink.push_row(y, &row).unwrap();
        }
        sink.finish().unwrap()
    }

    fn decode_rgb(jpeg: &[u8]) -> Vec<u8> {
        let mut decoder = Decoder::new(Cursor::new(jpeg));
        let pixels = decoder.decode().unwrap();
        assert_eq!(decoder.info().unwrap().pixel_format, PixelFormat::RGB24);
        pixels
    }

    #[test]
    fn defaults_match_the_public_contract() {
        assert_eq!(
            JpegOptions::default(),
            JpegOptions {
                quality: 85,
                background: [255, 255, 255],
                subsampling: JpegSubsampling::S420,
            }
        );
    }

    #[test]
    fn rejects_zero_quality() {
        let options = JpegOptions {
            quality: 0,
            ..JpegOptions::default()
        };
        assert!(matches!(
            JpegRowSink::new(Dimensions::new(1, 1).unwrap(), 1024, options),
            Err(Error::InvalidJpegOptions(_))
        ));
    }

    #[test]
    fn rejects_dimensions_outside_the_baseline_header_range() {
        assert!(matches!(
            JpegRowSink::new(
                Dimensions::new(65_536, 1).unwrap(),
                1024,
                JpegOptions::default()
            ),
            Err(Error::InvalidJpegOptions(_))
        ));
    }

    #[test]
    fn emits_decodable_baseline_jpeg_across_restart_intervals() {
        let jpeg = encode_solid([20, 80, 140, 255], JpegOptions::default());
        assert!(jpeg.starts_with(&[0xff, 0xd8]));
        assert!(jpeg.ends_with(&[0xff, 0xd9]));
        assert!(jpeg.windows(2).any(|marker| marker == [0xff, 0xc0]));
        assert!(jpeg.windows(2).any(|marker| marker == [0xff, 0xdd]));
        assert!(jpeg.windows(2).any(|marker| marker == [0xff, 0xd0]));
        assert!(!jpeg.windows(2).any(|marker| marker == [0xff, 0xc2]));
        assert_eq!(decode_rgb(&jpeg).len(), 17 * 33 * 3);
    }

    #[test]
    fn composites_transparency_over_the_configured_background() {
        let options = JpegOptions {
            quality: 100,
            background: [60, 120, 220],
            subsampling: JpegSubsampling::S444,
        };
        let transparent = decode_rgb(&encode_solid([255, 0, 0, 0], options));
        let partial = decode_rgb(&encode_solid([200, 40, 80, 128], options));
        for (actual, expected) in transparent[..3].iter().zip(options.background) {
            assert!(actual.abs_diff(expected) <= 2);
        }
        for (actual, expected) in partial[..3].iter().zip([130_u8, 80, 150]) {
            assert!(actual.abs_diff(expected) <= 2);
        }
    }

    #[test]
    fn maps_subsampling_to_the_sof_sampling_factors() {
        for (subsampling, expected) in [
            (JpegSubsampling::S420, 0x22),
            (JpegSubsampling::S422, 0x21),
            (JpegSubsampling::S444, 0x11),
        ] {
            let jpeg = encode_solid(
                [10, 20, 30, 255],
                JpegOptions {
                    subsampling,
                    ..JpegOptions::default()
                },
            );
            let sof = jpeg
                .windows(2)
                .position(|marker| marker == [0xff, 0xc0])
                .unwrap();
            assert_eq!(jpeg[sof + 11], expected);
            assert_eq!(decode_rgb(&jpeg).len(), 17 * 33 * 3);
        }
    }

    #[test]
    fn rotates_restart_markers_after_each_mcu_row() {
        let dimensions = Dimensions::new(17, 145).unwrap();
        let mut sink = JpegRowSink::new(
            dimensions,
            256 * 1024,
            JpegOptions {
                subsampling: JpegSubsampling::S420,
                ..JpegOptions::default()
            },
        )
        .unwrap();
        let row = [90_u8; 17 * 4];
        for y in 0..145 {
            sink.push_row(y, &row).unwrap();
        }
        let jpeg = sink.finish().unwrap();
        let markers = jpeg
            .windows(2)
            .filter_map(|marker| {
                (marker[0] == 0xff && (0xd0..=0xd7).contains(&marker[1])).then_some(marker[1])
            })
            .collect::<Vec<_>>();
        assert_eq!(
            markers,
            [0xd0, 0xd1, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd0]
        );
        assert_eq!(decode_rgb(&jpeg).len(), 17 * 145 * 3);
    }

    #[test]
    fn preserves_row_and_output_bounds() {
        let dimensions = Dimensions::new(16, 16).unwrap();
        let mut sink = JpegRowSink::new(dimensions, 128 * 1024, JpegOptions::default()).unwrap();
        assert!(matches!(
            sink.push_row(1, &[0; 16 * 4]),
            Err(Error::Core(CoreError::UnexpectedRow { .. }))
        ));

        let mut sink = JpegRowSink::new(dimensions, 32, JpegOptions::default()).unwrap();
        let row = [0; 16 * 4];
        let mut error = None;
        for y in 0..16 {
            if let Err(current) = sink.push_row(y, &row) {
                error = Some(current);
                break;
            }
        }
        assert!(matches!(
            error,
            Some(Error::EncodedOutputLimitExceeded {
                format: OutputFormat::Jpeg,
                limit: 32
            })
        ));
    }
}
