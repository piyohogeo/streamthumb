use std::io::Cursor;

use png::{BitDepth, ColorType};
use streamthumb_core::{
    AreaDownsampler, Dimensions, InputInfo, OutputFormat, ProcessingPlan, RgbaImage,
    ThumbnailOptions, plan_thumbnail,
};

use crate::{Error, Result, ThumbnailOutput, UnsupportedFeature, encoder::encode_rgba_png};

/// A normalized 8-bit RGBA source row.
#[derive(Clone, Copy, Debug)]
pub struct RgbaRow<'a> {
    pub y: u32,
    pub pixels: &'a [u8],
    pub plan: ProcessingPlan,
}

/// Metadata returned after all source rows have been decoded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodedPngInfo {
    pub dimensions: Dimensions,
    pub rows_decoded: u32,
    pub plan: ProcessingPlan,
}

/// Decodes a supported PNG one row at a time and normalizes each row to RGBA8.
///
/// The callback's row slice is valid only for the duration of the call. This
/// function never allocates a full-resolution source image.
pub fn decode_png_rows<F>(
    input: &[u8],
    options: &ThumbnailOptions,
    mut consume_row: F,
) -> Result<DecodedPngInfo>
where
    F: FnMut(RgbaRow<'_>) -> Result<()>,
{
    let encoded_bytes =
        u64::try_from(input.len()).map_err(|_| streamthumb_core::Error::IntegerOverflow {
            operation: "encoded input length conversion",
        })?;
    if encoded_bytes > options.limits.max_input_bytes {
        return Err(streamthumb_core::Error::LimitExceeded {
            kind: streamthumb_core::LimitKind::InputBytes,
            actual: encoded_bytes,
            limit: options.limits.max_input_bytes,
        }
        .into());
    }

    let decoder_limit = options.limits.max_working_memory_bytes;
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(input),
        png::Limits {
            bytes: decoder_limit,
        },
    );
    decoder.set_transformations(png::Transformations::IDENTITY);
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);

    let header = decoder
        .read_header_info()
        .map_err(|error| map_decode_error(error, decoder_limit))?;
    let dimensions = Dimensions::new(header.width, header.height)?;
    validate_header(header.color_type, header.bit_depth, header.interlaced)?;
    let source_bytes_per_pixel = bytes_per_pixel(header.color_type)?;
    let plan = plan_thumbnail(
        InputInfo {
            dimensions,
            encoded_bytes,
            source_bytes_per_pixel,
        },
        options,
    )?;
    decoder.set_limits(png::Limits {
        bytes: plan
            .memory
            .decoder_rows_bytes
            .checked_add(plan.memory.decoder_staging_bytes)
            .ok_or(streamthumb_core::Error::IntegerOverflow {
                operation: "PNG decoder allowance",
            })?,
    });

    let mut reader = decoder
        .read_info()
        .map_err(|error| map_decode_error(error, decoder_limit))?;
    if reader.info().is_animated() {
        return Err(Error::Unsupported {
            feature: UnsupportedFeature::Animation,
            detail: "APNG is not supported",
        });
    }
    let (output_color, output_depth) = reader.output_color_type();
    validate_header(output_color, output_depth, reader.info().interlaced)?;

    let rgba_row_bytes = usize::try_from(dimensions.width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or(streamthumb_core::Error::IntegerOverflow {
            operation: "normalized RGBA row size",
        })?;
    let mut normalized_row = Vec::new();
    normalized_row
        .try_reserve_exact(rgba_row_bytes)
        .map_err(|_| Error::AllocationFailed {
            bytes: rgba_row_bytes,
        })?;
    normalized_row.resize(rgba_row_bytes, 0);
    let mut rows_decoded = 0_u32;

    while let Some(row) = reader
        .next_row()
        .map_err(|error| map_decode_error(error, decoder_limit))?
    {
        normalize_row(row.data(), output_color, &mut normalized_row)?;
        consume_row(RgbaRow {
            y: rows_decoded,
            pixels: &normalized_row,
            plan,
        })?;
        rows_decoded =
            rows_decoded
                .checked_add(1)
                .ok_or(streamthumb_core::Error::IntegerOverflow {
                    operation: "decoded row count",
                })?;
    }

    if rows_decoded != dimensions.height {
        return Err(Error::TruncatedInput);
    }
    reader
        .finish()
        .map_err(|error| map_decode_error(error, decoder_limit))?;

    Ok(DecodedPngInfo {
        dimensions,
        rows_decoded,
        plan,
    })
}

/// Decodes and area-downsamples a supported PNG into a bounded RGBA8 image.
pub fn thumbnail_png_rgba(input: &[u8], options: &ThumbnailOptions) -> Result<RgbaImage> {
    let mut rgba_options = *options;
    rgba_options.output = OutputFormat::Rgba;
    thumbnail_png_rgba_planned(input, &rgba_options).map(|(image, _)| image)
}

/// Creates a thumbnail using the output representation selected in `options`.
pub fn thumbnail_png(input: &[u8], options: &ThumbnailOptions) -> Result<ThumbnailOutput> {
    let (image, plan) = thumbnail_png_rgba_planned(input, options)?;
    match options.output {
        OutputFormat::Rgba => Ok(image.into()),
        OutputFormat::Png => {
            let width = image.dimensions.width;
            let height = image.dimensions.height;
            let bytes = encode_rgba_png(&image, plan.memory.encoded_output_bytes)?;
            Ok(ThumbnailOutput::Encoded {
                bytes,
                width,
                height,
                mime_type: "image/png",
            })
        }
    }
}

fn thumbnail_png_rgba_planned(
    input: &[u8],
    options: &ThumbnailOptions,
) -> Result<(RgbaImage, ProcessingPlan)> {
    let mut downsampler = None;
    let decoded = decode_png_rows(input, options, |row| {
        if downsampler.is_none() {
            downsampler = Some(AreaDownsampler::new(row.plan.source, row.plan.output)?);
        }
        let active = downsampler.as_mut().ok_or_else(|| {
            Error::DecodeFailure("failed to initialize the area downsampler".to_owned())
        })?;
        active.push_row(row.y, row.pixels)?;
        Ok(())
    })?;

    let image = downsampler
        .ok_or(Error::TruncatedInput)?
        .finish()
        .map_err(Error::from)?;
    Ok((image, decoded.plan))
}

fn validate_header(color: ColorType, depth: BitDepth, interlaced: bool) -> Result<()> {
    if interlaced {
        return Err(Error::Unsupported {
            feature: UnsupportedFeature::Interlacing,
            detail: "Adam7 support is deferred",
        });
    }
    if depth != BitDepth::Eight {
        return Err(Error::Unsupported {
            feature: UnsupportedFeature::BitDepth,
            detail: "only 8-bit samples are supported",
        });
    }
    if !matches!(color, ColorType::Rgb | ColorType::Rgba) {
        return Err(Error::Unsupported {
            feature: UnsupportedFeature::ColorType,
            detail: "only RGB and RGBA are supported",
        });
    }
    Ok(())
}

fn bytes_per_pixel(color: ColorType) -> Result<u8> {
    match color {
        ColorType::Rgb => Ok(3),
        ColorType::Rgba => Ok(4),
        _ => Err(Error::Unsupported {
            feature: UnsupportedFeature::ColorType,
            detail: "only RGB and RGBA are supported",
        }),
    }
}

fn normalize_row(source: &[u8], color: ColorType, destination: &mut [u8]) -> Result<()> {
    let samples = match color {
        ColorType::Rgb => 3,
        ColorType::Rgba => 4,
        _ => {
            return Err(Error::Unsupported {
                feature: UnsupportedFeature::ColorType,
                detail: "only RGB and RGBA are supported",
            });
        }
    };
    let expected_source_len = destination
        .len()
        .checked_div(4)
        .and_then(|width| width.checked_mul(samples))
        .ok_or(streamthumb_core::Error::IntegerOverflow {
            operation: "decoded PNG row length",
        })?;
    if source.len() != expected_source_len {
        return Err(Error::DecodeFailure(format!(
            "decoded row has {} bytes; expected {expected_source_len}",
            source.len()
        )));
    }

    match color {
        ColorType::Rgb => {
            for (rgb, rgba) in source.chunks_exact(3).zip(destination.chunks_exact_mut(4)) {
                rgba.copy_from_slice(&[rgb[0], rgb[1], rgb[2], u8::MAX]);
            }
        }
        ColorType::Rgba => destination.copy_from_slice(source),
        _ => {
            return Err(Error::Unsupported {
                feature: UnsupportedFeature::ColorType,
                detail: "only RGB and RGBA are supported",
            });
        }
    }
    Ok(())
}

fn map_decode_error(error: png::DecodingError, decoder_limit: usize) -> Error {
    match error {
        png::DecodingError::IoError(io_error)
            if io_error.kind() == std::io::ErrorKind::UnexpectedEof =>
        {
            Error::TruncatedInput
        }
        png::DecodingError::LimitsExceeded => Error::DecoderMemoryLimitExceeded {
            limit: decoder_limit,
        },
        other => Error::DecodeFailure(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use png::Filter;
    use streamthumb_core::{Error as CoreError, LimitKind};

    fn encode_png(
        width: u32,
        height: u32,
        color: ColorType,
        depth: BitDepth,
        filter: Filter,
        pixels: &[u8],
    ) -> Vec<u8> {
        let mut encoded = Vec::new();
        let mut encoder = png::Encoder::new(&mut encoded, width, height);
        encoder.set_color(color);
        encoder.set_depth(depth);
        encoder.set_filter(filter);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(pixels).unwrap();
        writer.finish().unwrap();
        encoded
    }

    fn default_options() -> ThumbnailOptions {
        ThumbnailOptions::default()
    }

    #[test]
    fn streams_rgb_rows_in_order_and_normalizes_alpha() {
        let pixels = [
            1, 2, 3, 4, 5, 6, // row 0
            7, 8, 9, 10, 11, 12, // row 1
            13, 14, 15, 16, 17, 18, // row 2
        ];
        let encoded = encode_png(
            2,
            3,
            ColorType::Rgb,
            BitDepth::Eight,
            Filter::Paeth,
            &pixels,
        );
        let mut rows = Vec::new();

        let info = decode_png_rows(&encoded, &default_options(), |row| {
            rows.push((row.y, row.pixels.to_vec()));
            Ok(())
        })
        .unwrap();

        assert_eq!(info.dimensions, Dimensions::new(2, 3).unwrap());
        assert_eq!(info.rows_decoded, 3);
        assert_eq!(rows[0], (0, vec![1, 2, 3, 255, 4, 5, 6, 255]));
        assert_eq!(rows[1], (1, vec![7, 8, 9, 255, 10, 11, 12, 255]));
        assert_eq!(rows[2], (2, vec![13, 14, 15, 255, 16, 17, 18, 255]));
    }

    #[test]
    fn preserves_rgba_rows_for_every_png_filter() {
        let pixels = [
            1, 2, 3, 4, 5, 6, 7, 8, // row 0
            9, 10, 11, 12, 13, 14, 15, 16, // row 1
        ];
        for filter in [
            Filter::NoFilter,
            Filter::Sub,
            Filter::Up,
            Filter::Avg,
            Filter::Paeth,
        ] {
            let encoded = encode_png(2, 2, ColorType::Rgba, BitDepth::Eight, filter, &pixels);
            let mut decoded = Vec::new();
            decode_png_rows(&encoded, &default_options(), |row| {
                decoded.extend_from_slice(row.pixels);
                Ok(())
            })
            .unwrap();
            assert_eq!(decoded, pixels, "filter {filter:?}");
        }
    }

    #[test]
    fn fuses_rgb_decode_with_arbitrary_ratio_area_downsampling() {
        let encoded = encode_png(
            3,
            1,
            ColorType::Rgb,
            BitDepth::Eight,
            Filter::Sub,
            &[0, 0, 0, 60, 60, 60, 120, 120, 120],
        );
        let options = ThumbnailOptions {
            max_width: 2,
            max_height: 1,
            ..default_options()
        };

        let thumbnail = thumbnail_png_rgba(&encoded, &options).unwrap();

        assert_eq!(thumbnail.dimensions, Dimensions::new(2, 1).unwrap());
        assert_eq!(thumbnail.pixels, [20, 20, 20, 255, 100, 100, 100, 255]);
    }

    #[test]
    fn fused_path_uses_premultiplied_alpha() {
        let encoded = encode_png(
            2,
            1,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[255, 0, 0, 0, 0, 0, 255, 255],
        );
        let options = ThumbnailOptions {
            max_width: 1,
            max_height: 1,
            ..default_options()
        };

        let thumbnail = thumbnail_png_rgba(&encoded, &options).unwrap();

        assert_eq!(thumbnail.pixels, [0, 0, 255, 128]);
    }

    #[test]
    fn public_output_api_returns_raw_rgba_when_requested() {
        let encoded = encode_png(
            1,
            1,
            ColorType::Rgb,
            BitDepth::Eight,
            Filter::NoFilter,
            &[10, 20, 30],
        );
        let options = ThumbnailOptions {
            output: OutputFormat::Rgba,
            ..default_options()
        };

        let output = thumbnail_png(&encoded, &options).unwrap();

        assert_eq!(
            output,
            ThumbnailOutput::Rgba {
                pixels: vec![10, 20, 30, 255],
                width: 1,
                height: 1,
            }
        );
        assert_eq!(output.info().format, OutputFormat::Rgba);
    }

    #[test]
    fn encoded_output_round_trips_as_rgba_png() {
        let encoded = encode_png(
            2,
            1,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[255, 0, 0, 0, 0, 0, 255, 255],
        );
        let options = ThumbnailOptions {
            max_width: 1,
            max_height: 1,
            output: OutputFormat::Png,
            ..default_options()
        };

        let output = thumbnail_png(&encoded, &options).unwrap();
        let ThumbnailOutput::Encoded {
            bytes,
            width,
            height,
            mime_type,
        } = output
        else {
            panic!("expected encoded PNG output");
        };
        assert_eq!((width, height, mime_type), (1, 1, "image/png"));

        let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        assert_eq!(&pixels[..info.buffer_size()], &[0, 0, 255, 128]);
    }

    #[test]
    fn rejects_input_byte_limit_before_png_parsing() {
        let encoded = encode_png(
            1,
            1,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[0, 0, 0, 0],
        );
        let mut options = default_options();
        options.limits.max_input_bytes = u64::try_from(encoded.len() - 1).unwrap();

        let error = decode_png_rows(&encoded, &options, |_| Ok(())).unwrap_err();
        assert!(matches!(
            error,
            Error::Core(CoreError::LimitExceeded {
                kind: LimitKind::InputBytes,
                ..
            })
        ));
    }

    #[test]
    fn rejects_dimensions_before_decoding_rows() {
        let encoded = encode_png(
            2,
            1,
            ColorType::Rgb,
            BitDepth::Eight,
            Filter::NoFilter,
            &[0; 6],
        );
        let mut options = default_options();
        options.limits.max_width = 1;
        let mut callbacks = 0;

        let error = decode_png_rows(&encoded, &options, |_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();

        assert_eq!(callbacks, 0);
        assert!(matches!(
            error,
            Error::Core(CoreError::LimitExceeded {
                kind: LimitKind::InputWidth,
                ..
            })
        ));
    }

    #[test]
    fn rejects_pixel_bomb_from_ihdr_before_decoding_rows() {
        let mut encoded = encode_png(
            1,
            1,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[0, 0, 0, 0],
        );
        replace_ihdr_dimensions(&mut encoded, 50_000, 50_000);
        let mut options = default_options();
        options.limits.max_width = 60_000;
        options.limits.max_height = 60_000;
        options.limits.max_pixels = 500_000_000;
        let mut callbacks = 0;

        let error = decode_png_rows(&encoded, &options, |_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();

        assert_eq!(callbacks, 0);
        assert!(matches!(
            error,
            Error::Core(CoreError::LimitExceeded {
                kind: LimitKind::InputPixels,
                ..
            })
        ));
    }

    #[test]
    fn rejects_oversized_truncated_ancillary_chunk_without_decoding_rows() {
        let mut encoded = encode_png(
            1,
            1,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[0, 0, 0, 0],
        );
        let mut chunk_header = Vec::new();
        chunk_header.extend_from_slice(&0x7fff_ffff_u32.to_be_bytes());
        chunk_header.extend_from_slice(b"eXIf");
        encoded.splice(33..33, chunk_header);
        let mut callbacks = 0;

        let error = decode_png_rows(&encoded, &default_options(), |_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();

        assert_eq!(callbacks, 0);
        assert!(matches!(
            error,
            Error::TruncatedInput
                | Error::DecodeFailure(_)
                | Error::DecoderMemoryLimitExceeded { .. }
        ));
    }

    #[test]
    fn highly_compressible_source_plan_stays_below_full_frame_size() {
        let width = 512;
        let height = 512;
        let pixels = vec![0_u8; width * height * 4];
        let encoded = encode_png(
            width as u32,
            height as u32,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &pixels,
        );
        let options = ThumbnailOptions {
            max_width: 16,
            max_height: 16,
            output: OutputFormat::Rgba,
            ..default_options()
        };
        let mut rows = 0;

        let info = decode_png_rows(&encoded, &options, |_| {
            rows += 1;
            Ok(())
        })
        .unwrap();

        assert_eq!(rows, height);
        assert!(encoded.len() < pixels.len() / 10);
        assert!(info.plan.memory.total_bytes < pixels.len());
    }

    #[test]
    fn rejects_memory_budget_before_decoding_rows() {
        let encoded = encode_png(
            2,
            2,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[0; 16],
        );
        let mut options = default_options();
        options.limits.max_working_memory_bytes = 1;
        let mut callbacks = 0;

        let error = decode_png_rows(&encoded, &options, |_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();

        assert_eq!(callbacks, 0);
        assert!(matches!(
            error,
            Error::Core(CoreError::LimitExceeded {
                kind: LimitKind::WorkingMemory,
                ..
            })
        ));
    }

    #[test]
    fn rejects_grayscale_and_sixteen_bit_pngs() {
        let grayscale = encode_png(
            1,
            1,
            ColorType::Grayscale,
            BitDepth::Eight,
            Filter::NoFilter,
            &[10],
        );
        assert!(matches!(
            decode_png_rows(&grayscale, &default_options(), |_| Ok(())).unwrap_err(),
            Error::Unsupported {
                feature: UnsupportedFeature::ColorType,
                ..
            }
        ));

        let sixteen_bit = encode_png(
            1,
            1,
            ColorType::Rgb,
            BitDepth::Sixteen,
            Filter::NoFilter,
            &[0; 6],
        );
        assert!(matches!(
            decode_png_rows(&sixteen_bit, &default_options(), |_| Ok(())).unwrap_err(),
            Error::Unsupported {
                feature: UnsupportedFeature::BitDepth,
                ..
            }
        ));
    }

    #[test]
    fn rejects_interlacing_from_the_header() {
        let mut encoded = encode_png(
            1,
            1,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[0, 0, 0, 0],
        );
        encoded[28] = 1;
        replace_ihdr_crc(&mut encoded);

        assert!(matches!(
            decode_png_rows(&encoded, &default_options(), |_| Ok(())).unwrap_err(),
            Error::Unsupported {
                feature: UnsupportedFeature::Interlacing,
                ..
            }
        ));
    }

    #[test]
    fn rejects_apng_before_decoding_rows() {
        let mut encoded = encode_png(
            1,
            1,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[0, 0, 0, 0],
        );
        insert_chunk_after_ihdr(
            &mut encoded,
            *b"fcTL",
            &[
                0, 0, 0, 0, // sequence number
                0, 0, 0, 1, // width
                0, 0, 0, 1, // height
                0, 0, 0, 0, // x offset
                0, 0, 0, 0, // y offset
                0, 1, // delay numerator
                0, 100, // delay denominator
                0,   // dispose operation
                0,   // blend operation
            ],
        );
        insert_chunk_after_ihdr(&mut encoded, *b"acTL", &[0, 0, 0, 1, 0, 0, 0, 0]);
        let mut callbacks = 0;

        let error = decode_png_rows(&encoded, &default_options(), |_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();

        assert_eq!(callbacks, 0);
        assert!(matches!(
            error,
            Error::Unsupported {
                feature: UnsupportedFeature::Animation,
                ..
            }
        ));
    }

    #[test]
    fn rejects_malformed_input_without_calling_the_consumer() {
        let mut callbacks = 0;
        let error = decode_png_rows(b"not a PNG", &default_options(), |_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();

        assert_eq!(callbacks, 0);
        assert!(matches!(error, Error::DecodeFailure(_)));
    }

    #[test]
    fn rejects_truncated_input_without_panicking() {
        let mut encoded = encode_png(
            8,
            8,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[42; 8 * 8 * 4],
        );
        encoded.truncate(encoded.len() / 2);

        let error = decode_png_rows(&encoded, &default_options(), |_| Ok(())).unwrap_err();
        assert!(matches!(error, Error::TruncatedInput), "{error:?}");
    }

    #[test]
    fn propagates_consumer_failures_and_stops_decoding() {
        let encoded = encode_png(
            1,
            3,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[0; 12],
        );
        let mut callbacks = 0;

        let error = decode_png_rows(&encoded, &default_options(), |_| {
            callbacks += 1;
            Err(Error::RowConsumer("intentional test failure".to_owned()))
        })
        .unwrap_err();

        assert_eq!(callbacks, 1);
        assert!(matches!(error, Error::RowConsumer(_)));
    }

    fn replace_ihdr_crc(png: &mut [u8]) {
        let crc = crc32(&png[12..29]).to_be_bytes();
        png[29..33].copy_from_slice(&crc);
    }

    fn replace_ihdr_dimensions(png: &mut [u8], width: u32, height: u32) {
        png[16..20].copy_from_slice(&width.to_be_bytes());
        png[20..24].copy_from_slice(&height.to_be_bytes());
        replace_ihdr_crc(png);
    }

    fn insert_chunk_after_ihdr(png: &mut Vec<u8>, chunk_type: [u8; 4], data: &[u8]) {
        let mut chunk = Vec::with_capacity(12 + data.len());
        chunk.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
        chunk.extend_from_slice(&chunk_type);
        chunk.extend_from_slice(data);
        chunk.extend_from_slice(&crc32(&chunk[4..]).to_be_bytes());
        png.splice(33..33, chunk);
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = 0_u32.wrapping_sub(crc & 1);
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        !crc
    }
}
