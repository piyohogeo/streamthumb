use std::io::Cursor;

use png::{BitDepth, ColorType};
use streamthumb_core::{
    AreaDownsampler, Dimensions, InputInfo, OutputFormat, ProcessingPlan, RgbaImage,
    SparseAreaDownsampler, ThumbnailOptions, plan_thumbnail, plan_thumbnail_sparse,
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
    let source_color = header.color_type;
    validate_source_color_depth(source_color, header.bit_depth)?;
    reject_interlacing(header.interlaced)?;
    let source_bytes_per_pixel = planning_bytes_per_pixel(source_color, header.bit_depth)?;
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
    reject_separate_transparency(source_color, reader.info().trns.is_some())?;
    let (output_color, output_depth) = reader.output_color_type();
    validate_source_color_depth(output_color, output_depth)?;
    let source_format = SourceFormat::from_info(reader.info())?;
    reject_interlacing(reader.info().interlaced)?;

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
        normalize_row(row.data(), &source_format, &mut normalized_row)?;
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
    if png_is_interlaced(input, options)? {
        return thumbnail_png_adam7_rgba_planned(input, options);
    }

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

fn png_is_interlaced(input: &[u8], options: &ThumbnailOptions) -> Result<bool> {
    checked_encoded_length(input, options)?;
    let decoder_limit = options.limits.max_working_memory_bytes;
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(input),
        png::Limits {
            bytes: decoder_limit,
        },
    );
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);
    let header = decoder
        .read_header_info()
        .map_err(|error| map_decode_error(error, decoder_limit))?;
    validate_source_color_depth(header.color_type, header.bit_depth)?;
    Ok(header.interlaced)
}

fn thumbnail_png_adam7_rgba_planned(
    input: &[u8],
    options: &ThumbnailOptions,
) -> Result<(RgbaImage, ProcessingPlan)> {
    let encoded_bytes = checked_encoded_length(input, options)?;
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
    let source_color = header.color_type;
    validate_source_color_depth(source_color, header.bit_depth)?;
    if !header.interlaced {
        return Err(Error::DecodeFailure(
            "Adam7 path received a non-interlaced PNG".to_owned(),
        ));
    }
    let source_bytes_per_pixel = planning_bytes_per_pixel(source_color, header.bit_depth)?;
    let plan = plan_thumbnail_sparse(
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
                operation: "Adam7 PNG decoder allowance",
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
    reject_separate_transparency(source_color, reader.info().trns.is_some())?;
    let (output_color, output_depth) = reader.output_color_type();
    validate_source_color_depth(output_color, output_depth)?;
    let source_format = SourceFormat::from_info(reader.info())?;
    if !reader.info().interlaced {
        return Err(Error::DecodeFailure(
            "PNG interlace mode changed after header validation".to_owned(),
        ));
    }

    let mut downsampler = SparseAreaDownsampler::new(plan.source, plan.output)?;
    for pass in ADAM7_PASSES {
        let samples = pass_sample_count(dimensions.width, pass.x_offset, pass.x_stride);
        let lines = pass_sample_count(dimensions.height, pass.y_offset, pass.y_stride);
        if samples == 0 || lines == 0 {
            continue;
        }
        for line in 0..lines {
            let row = reader
                .next_interlaced_row()
                .map_err(|error| map_decode_error(error, decoder_limit))?
                .ok_or(Error::TruncatedInput)?;
            if !matches!(row.interlace(), png::InterlaceInfo::Adam7(_)) {
                return Err(Error::DecodeFailure(
                    "PNG decoder returned a non-Adam7 row for interlaced input".to_owned(),
                ));
            }
            let sample_count =
                usize::try_from(samples).map_err(|_| streamthumb_core::Error::IntegerOverflow {
                    operation: "Adam7 pass sample count conversion",
                })?;
            let expected_bytes = source_format.row_bytes(sample_count)?;
            if row.data().len() != expected_bytes {
                return Err(Error::DecodeFailure(format!(
                    "Adam7 pass row has {} bytes; expected {expected_bytes}",
                    row.data().len()
                )));
            }
            let y = pass
                .y_offset
                .checked_add(line.checked_mul(pass.y_stride).ok_or(
                    streamthumb_core::Error::IntegerOverflow {
                        operation: "Adam7 source y coordinate",
                    },
                )?)
                .ok_or(streamthumb_core::Error::IntegerOverflow {
                    operation: "Adam7 source y coordinate",
                })?;
            for sample in 0..samples {
                let x = pass
                    .x_offset
                    .checked_add(sample.checked_mul(pass.x_stride).ok_or(
                        streamthumb_core::Error::IntegerOverflow {
                            operation: "Adam7 source x coordinate",
                        },
                    )?)
                    .ok_or(streamthumb_core::Error::IntegerOverflow {
                        operation: "Adam7 source x coordinate",
                    })?;
                downsampler.push_pixel(
                    x,
                    y,
                    source_format.pixel(
                        row.data(),
                        usize::try_from(sample).map_err(|_| {
                            streamthumb_core::Error::IntegerOverflow {
                                operation: "Adam7 sample index conversion",
                            }
                        })?,
                    )?,
                )?;
            }
        }
    }

    if reader
        .next_interlaced_row()
        .map_err(|error| map_decode_error(error, decoder_limit))?
        .is_some()
    {
        return Err(Error::DecodeFailure(
            "PNG decoder returned more Adam7 rows than expected".to_owned(),
        ));
    }
    reader
        .finish()
        .map_err(|error| map_decode_error(error, decoder_limit))?;
    Ok((downsampler.finish()?, plan))
}

#[derive(Clone, Copy)]
struct Adam7Pass {
    x_stride: u32,
    x_offset: u32,
    y_stride: u32,
    y_offset: u32,
}

const ADAM7_PASSES: [Adam7Pass; 7] = [
    Adam7Pass {
        x_stride: 8,
        x_offset: 0,
        y_stride: 8,
        y_offset: 0,
    },
    Adam7Pass {
        x_stride: 8,
        x_offset: 4,
        y_stride: 8,
        y_offset: 0,
    },
    Adam7Pass {
        x_stride: 4,
        x_offset: 0,
        y_stride: 8,
        y_offset: 4,
    },
    Adam7Pass {
        x_stride: 4,
        x_offset: 2,
        y_stride: 4,
        y_offset: 0,
    },
    Adam7Pass {
        x_stride: 2,
        x_offset: 0,
        y_stride: 4,
        y_offset: 2,
    },
    Adam7Pass {
        x_stride: 2,
        x_offset: 1,
        y_stride: 2,
        y_offset: 0,
    },
    Adam7Pass {
        x_stride: 1,
        x_offset: 0,
        y_stride: 2,
        y_offset: 1,
    },
];

fn pass_sample_count(length: u32, offset: u32, stride: u32) -> u32 {
    length.saturating_sub(offset).div_ceil(stride)
}

fn checked_encoded_length(input: &[u8], options: &ThumbnailOptions) -> Result<u64> {
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
    Ok(encoded_bytes)
}

fn validate_source_color_depth(color: ColorType, depth: BitDepth) -> Result<()> {
    let valid_depth = match color {
        ColorType::Indexed => matches!(
            depth,
            BitDepth::One | BitDepth::Two | BitDepth::Four | BitDepth::Eight
        ),
        ColorType::Grayscale | ColorType::GrayscaleAlpha | ColorType::Rgb | ColorType::Rgba => {
            matches!(depth, BitDepth::Eight | BitDepth::Sixteen)
        }
    };
    if !valid_depth {
        return Err(Error::Unsupported {
            feature: UnsupportedFeature::BitDepth,
            detail: "only 8- or 16-bit direct samples and 1-, 2-, 4-, or 8-bit palette indices are supported",
        });
    }
    Ok(())
}

fn reject_interlacing(interlaced: bool) -> Result<()> {
    if interlaced {
        return Err(Error::Unsupported {
            feature: UnsupportedFeature::Interlacing,
            detail: "row callbacks require non-interlaced input; use thumbnail_png for Adam7",
        });
    }
    Ok(())
}

fn reject_separate_transparency(color: ColorType, has_transparency: bool) -> Result<()> {
    if has_transparency && matches!(color, ColorType::Grayscale | ColorType::Rgb) {
        return Err(Error::Unsupported {
            feature: UnsupportedFeature::ColorType,
            detail: "tRNS transparency is not supported; use an alpha color type",
        });
    }
    Ok(())
}

fn planning_bytes_per_pixel(color: ColorType, depth: BitDepth) -> Result<u8> {
    if color == ColorType::Indexed {
        Ok(1)
    } else {
        direct_bytes_per_pixel(color, depth)
    }
}

fn direct_channel_count(color: ColorType) -> Result<u8> {
    match color {
        ColorType::Grayscale => Ok(1),
        ColorType::GrayscaleAlpha => Ok(2),
        ColorType::Rgb => Ok(3),
        ColorType::Rgba => Ok(4),
        ColorType::Indexed => Err(Error::Unsupported {
            feature: UnsupportedFeature::ColorType,
            detail: "palette pixels do not have a direct channel count",
        }),
    }
}

fn direct_bytes_per_pixel(color: ColorType, depth: BitDepth) -> Result<u8> {
    let sample_bytes = match depth {
        BitDepth::Eight => 1,
        BitDepth::Sixteen => 2,
        BitDepth::One | BitDepth::Two | BitDepth::Four => {
            return Err(Error::Unsupported {
                feature: UnsupportedFeature::BitDepth,
                detail: "sub-byte direct samples are not supported",
            });
        }
    };
    direct_channel_count(color)?
        .checked_mul(sample_bytes)
        .ok_or_else(|| {
            streamthumb_core::Error::IntegerOverflow {
                operation: "direct source bytes per pixel",
            }
            .into()
        })
}

enum SourceFormat {
    Direct {
        color: ColorType,
        depth: BitDepth,
        bytes: usize,
    },
    Indexed {
        colors: Vec<[u8; 4]>,
        bits: usize,
    },
}

impl SourceFormat {
    fn from_info(info: &png::Info<'_>) -> Result<Self> {
        if info.color_type != ColorType::Indexed {
            return Ok(Self::Direct {
                color: info.color_type,
                depth: info.bit_depth,
                bytes: usize::from(direct_bytes_per_pixel(info.color_type, info.bit_depth)?),
            });
        }

        let palette = info.palette.as_deref().ok_or_else(|| {
            Error::DecodeFailure("indexed PNG is missing its PLTE chunk".to_owned())
        })?;
        if palette.is_empty() || palette.len() % 3 != 0 {
            return Err(Error::DecodeFailure(
                "indexed PNG has an invalid PLTE length".to_owned(),
            ));
        }
        let entries = palette.len() / 3;
        let bits = bit_depth_bits(info.bit_depth)?;
        let capacity = 1_usize
            .checked_shl(u32::try_from(bits).map_err(|_| {
                streamthumb_core::Error::IntegerOverflow {
                    operation: "palette bit depth conversion",
                }
            })?)
            .ok_or(streamthumb_core::Error::IntegerOverflow {
                operation: "palette capacity",
            })?;
        if entries > capacity {
            return Err(Error::DecodeFailure(format!(
                "indexed PNG has {entries} palette entries but its bit depth permits {capacity}"
            )));
        }
        let transparency = match info.trns.as_deref() {
            Some(transparency) => transparency,
            None => &[],
        };
        if transparency.len() > entries {
            return Err(Error::DecodeFailure(format!(
                "indexed PNG has {} transparency entries for {entries} palette entries",
                transparency.len()
            )));
        }
        let allocation_bytes =
            entries
                .checked_mul(4)
                .ok_or(streamthumb_core::Error::IntegerOverflow {
                    operation: "palette lookup allocation",
                })?;
        let mut colors = Vec::new();
        colors
            .try_reserve_exact(entries)
            .map_err(|_| Error::AllocationFailed {
                bytes: allocation_bytes,
            })?;
        for (index, rgb) in palette.chunks_exact(3).enumerate() {
            let alpha = match transparency.get(index) {
                Some(alpha) => *alpha,
                None => u8::MAX,
            };
            colors.push([rgb[0], rgb[1], rgb[2], alpha]);
        }
        Ok(Self::Indexed { colors, bits })
    }

    fn row_bytes(&self, samples: usize) -> Result<usize> {
        match self {
            Self::Direct { bytes, .. } => samples.checked_mul(*bytes).ok_or_else(|| {
                streamthumb_core::Error::IntegerOverflow {
                    operation: "decoded PNG row length",
                }
                .into()
            }),
            Self::Indexed { bits, .. } => samples
                .checked_mul(*bits)
                .and_then(|value| value.checked_add(7))
                .map(|value| value / 8)
                .ok_or_else(|| {
                    streamthumb_core::Error::IntegerOverflow {
                        operation: "packed palette row length",
                    }
                    .into()
                }),
        }
    }

    fn pixel(&self, source: &[u8], index: usize) -> Result<[u8; 4]> {
        match self {
            Self::Direct {
                color,
                depth,
                bytes,
            } => {
                let start =
                    index
                        .checked_mul(*bytes)
                        .ok_or(streamthumb_core::Error::IntegerOverflow {
                            operation: "direct sample offset",
                        })?;
                let end =
                    start
                        .checked_add(*bytes)
                        .ok_or(streamthumb_core::Error::IntegerOverflow {
                            operation: "direct sample end",
                        })?;
                let sample = source.get(start..end).ok_or_else(|| {
                    Error::DecodeFailure("decoded PNG row ended within a sample".to_owned())
                })?;
                normalize_direct_pixel(sample, *color, *depth)
            }
            Self::Indexed { colors, bits } => {
                let bit_offset =
                    index
                        .checked_mul(*bits)
                        .ok_or(streamthumb_core::Error::IntegerOverflow {
                            operation: "palette sample bit offset",
                        })?;
                let byte = *source.get(bit_offset / 8).ok_or_else(|| {
                    Error::DecodeFailure("packed palette row ended within an index".to_owned())
                })?;
                let shift = 8 - *bits - bit_offset % 8;
                let mask = (1_u16 << *bits) - 1;
                let palette_index = usize::from((u16::from(byte >> shift)) & mask);
                colors.get(palette_index).copied().ok_or_else(|| {
                    Error::DecodeFailure(format!(
                        "palette index {palette_index} exceeds {} entries",
                        colors.len()
                    ))
                })
            }
        }
    }
}

fn bit_depth_bits(depth: BitDepth) -> Result<usize> {
    match depth {
        BitDepth::One => Ok(1),
        BitDepth::Two => Ok(2),
        BitDepth::Four => Ok(4),
        BitDepth::Eight => Ok(8),
        BitDepth::Sixteen => Err(Error::Unsupported {
            feature: UnsupportedFeature::BitDepth,
            detail: "16-bit palette indices are not supported",
        }),
    }
}

fn normalize_row(source: &[u8], format: &SourceFormat, destination: &mut [u8]) -> Result<()> {
    let width =
        destination
            .len()
            .checked_div(4)
            .ok_or(streamthumb_core::Error::IntegerOverflow {
                operation: "normalized PNG row width",
            })?;
    let expected_source_len = format.row_bytes(width)?;
    if source.len() != expected_source_len {
        return Err(Error::DecodeFailure(format!(
            "decoded row has {} bytes; expected {expected_source_len}",
            source.len()
        )));
    }
    for (index, rgba) in destination.chunks_exact_mut(4).enumerate() {
        rgba.copy_from_slice(&format.pixel(source, index)?);
    }
    Ok(())
}

fn normalize_direct_pixel(source: &[u8], color: ColorType, depth: BitDepth) -> Result<[u8; 4]> {
    let channels = usize::from(direct_channel_count(color)?);
    let sample_bytes = match depth {
        BitDepth::Eight => 1,
        BitDepth::Sixteen => 2,
        BitDepth::One | BitDepth::Two | BitDepth::Four => {
            return Err(Error::Unsupported {
                feature: UnsupportedFeature::BitDepth,
                detail: "sub-byte direct samples are not supported",
            });
        }
    };
    let expected =
        channels
            .checked_mul(sample_bytes)
            .ok_or(streamthumb_core::Error::IntegerOverflow {
                operation: "direct sample length",
            })?;
    if source.len() != expected {
        return Err(Error::DecodeFailure(format!(
            "decoded PNG sample has {} bytes; expected {expected}",
            source.len()
        )));
    }

    let channel = |index: usize| -> Result<u8> {
        let start =
            index
                .checked_mul(sample_bytes)
                .ok_or(streamthumb_core::Error::IntegerOverflow {
                    operation: "direct channel offset",
                })?;
        match depth {
            BitDepth::Eight => source.get(start).copied().ok_or_else(|| {
                Error::DecodeFailure("decoded PNG sample ended within a channel".to_owned())
            }),
            BitDepth::Sixteen => {
                let end = start
                    .checked_add(2)
                    .ok_or(streamthumb_core::Error::IntegerOverflow {
                        operation: "16-bit channel end",
                    })?;
                let bytes = source.get(start..end).ok_or_else(|| {
                    Error::DecodeFailure("decoded PNG sample ended within a channel".to_owned())
                })?;
                let value = u16::from_be_bytes([bytes[0], bytes[1]]);
                let rounded = (u32::from(value) * 255 + 32_767) / 65_535;
                u8::try_from(rounded).map_err(|_| {
                    streamthumb_core::Error::IntegerOverflow {
                        operation: "16-bit sample normalization",
                    }
                    .into()
                })
            }
            BitDepth::One | BitDepth::Two | BitDepth::Four => Err(Error::Unsupported {
                feature: UnsupportedFeature::BitDepth,
                detail: "sub-byte direct samples are not supported",
            }),
        }
    };

    match color {
        ColorType::Grayscale => {
            let gray = channel(0)?;
            Ok([gray, gray, gray, u8::MAX])
        }
        ColorType::GrayscaleAlpha => {
            let gray = channel(0)?;
            Ok([gray, gray, gray, channel(1)?])
        }
        ColorType::Rgb => Ok([channel(0)?, channel(1)?, channel(2)?, u8::MAX]),
        ColorType::Rgba => Ok([channel(0)?, channel(1)?, channel(2)?, channel(3)?]),
        ColorType::Indexed => Err(Error::DecodeFailure(
            "palette sample reached the direct normalizer".to_owned(),
        )),
    }
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
    use flate2::{Compression, write::ZlibEncoder};
    use png::Filter;
    use std::io::Write;
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

    fn test_depth_bits(depth: BitDepth) -> usize {
        match depth {
            BitDepth::One => 1,
            BitDepth::Two => 2,
            BitDepth::Four => 4,
            BitDepth::Eight => 8,
            BitDepth::Sixteen => panic!("test palette helper does not support 16-bit indices"),
        }
    }

    fn pack_palette_row(indices: &[u8], depth: BitDepth) -> Vec<u8> {
        let bits = test_depth_bits(depth);
        let mut packed = vec![0; (indices.len() * bits).div_ceil(8)];
        let mask = u8::try_from((1_u16 << bits) - 1).unwrap();
        for (index, value) in indices.iter().copied().enumerate() {
            let bit_offset = index * bits;
            let shift = 8 - bits - bit_offset % 8;
            packed[bit_offset / 8] |= (value & mask) << shift;
        }
        packed
    }

    fn encode_palette_png(
        width: u32,
        height: u32,
        depth: BitDepth,
        indices: &[u8],
        palette: &[u8],
        transparency: &[u8],
    ) -> Vec<u8> {
        let width_usize = usize::try_from(width).unwrap();
        let mut packed = Vec::new();
        for row in indices.chunks_exact(width_usize) {
            packed.extend_from_slice(&pack_palette_row(row, depth));
        }
        assert_eq!(
            indices.len(),
            width_usize * usize::try_from(height).unwrap()
        );

        let mut encoded = Vec::new();
        let mut encoder = png::Encoder::new(&mut encoded, width, height);
        encoder.set_color(ColorType::Indexed);
        encoder.set_depth(depth);
        encoder.set_palette(palette.to_vec());
        if !transparency.is_empty() {
            encoder.set_trns(transparency.to_vec());
        }
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&packed).unwrap();
        writer.finish().unwrap();
        encoded
    }

    fn encode_adam7_png(width: u32, height: u32, color: ColorType, pixels: &[u8]) -> Vec<u8> {
        encode_adam7_direct_png(width, height, color, BitDepth::Eight, pixels)
    }

    fn encode_adam7_direct_png(
        width: u32,
        height: u32,
        color: ColorType,
        depth: BitDepth,
        pixels: &[u8],
    ) -> Vec<u8> {
        let channels = match color {
            ColorType::Grayscale => 1_usize,
            ColorType::GrayscaleAlpha => 2_usize,
            ColorType::Rgb => 3_usize,
            ColorType::Rgba => 4_usize,
            _ => panic!("test helper does not support this color type"),
        };
        let sample_bytes = channels
            * match depth {
                BitDepth::Eight => 1,
                BitDepth::Sixteen => 2,
                _ => panic!("test direct helper supports only 8- and 16-bit samples"),
            };
        let mut filtered = Vec::new();
        for pass in ADAM7_PASSES {
            let samples = pass_sample_count(width, pass.x_offset, pass.x_stride);
            let lines = pass_sample_count(height, pass.y_offset, pass.y_stride);
            if samples == 0 || lines == 0 {
                continue;
            }
            for line in 0..lines {
                filtered.push(0);
                let y = pass.y_offset + line * pass.y_stride;
                for sample in 0..samples {
                    let x = pass.x_offset + sample * pass.x_stride;
                    let offset = usize::try_from(
                        (u64::from(y) * u64::from(width) + u64::from(x))
                            * u64::try_from(sample_bytes).unwrap(),
                    )
                    .unwrap();
                    filtered.extend_from_slice(&pixels[offset..offset + sample_bytes]);
                }
            }
        }

        let mut compressor = ZlibEncoder::new(Vec::new(), Compression::default());
        compressor.write_all(&filtered).unwrap();
        let compressed = compressor.finish().unwrap();
        let mut encoded = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.push(match depth {
            BitDepth::Eight => 8,
            BitDepth::Sixteen => 16,
            _ => unreachable!(),
        });
        ihdr.push(match color {
            ColorType::Grayscale => 0,
            ColorType::Rgb => 2,
            ColorType::GrayscaleAlpha => 4,
            ColorType::Rgba => 6,
            _ => unreachable!(),
        });
        ihdr.extend_from_slice(&[0, 0, 1]);
        append_chunk(&mut encoded, *b"IHDR", &ihdr);
        append_chunk(&mut encoded, *b"IDAT", &compressed);
        append_chunk(&mut encoded, *b"IEND", &[]);
        encoded
    }

    fn encode_adam7_palette_png(
        width: u32,
        height: u32,
        depth: BitDepth,
        indices: &[u8],
        palette: &[u8],
        transparency: &[u8],
    ) -> Vec<u8> {
        let mut filtered = Vec::new();
        for pass in ADAM7_PASSES {
            let samples = pass_sample_count(width, pass.x_offset, pass.x_stride);
            let lines = pass_sample_count(height, pass.y_offset, pass.y_stride);
            if samples == 0 || lines == 0 {
                continue;
            }
            for line in 0..lines {
                filtered.push(0);
                let y = pass.y_offset + line * pass.y_stride;
                let mut pass_indices = Vec::new();
                for sample in 0..samples {
                    let x = pass.x_offset + sample * pass.x_stride;
                    let offset =
                        usize::try_from(u64::from(y) * u64::from(width) + u64::from(x)).unwrap();
                    pass_indices.push(indices[offset]);
                }
                filtered.extend_from_slice(&pack_palette_row(&pass_indices, depth));
            }
        }

        let mut compressor = ZlibEncoder::new(Vec::new(), Compression::default());
        compressor.write_all(&filtered).unwrap();
        let compressed = compressor.finish().unwrap();
        let mut encoded = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.push(u8::try_from(test_depth_bits(depth)).unwrap());
        ihdr.push(3);
        ihdr.extend_from_slice(&[0, 0, 1]);
        append_chunk(&mut encoded, *b"IHDR", &ihdr);
        append_chunk(&mut encoded, *b"PLTE", palette);
        if !transparency.is_empty() {
            append_chunk(&mut encoded, *b"tRNS", transparency);
        }
        append_chunk(&mut encoded, *b"IDAT", &compressed);
        append_chunk(&mut encoded, *b"IEND", &[]);
        encoded
    }

    fn append_chunk(png: &mut Vec<u8>, chunk_type: [u8; 4], data: &[u8]) {
        png.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
        png.extend_from_slice(&chunk_type);
        png.extend_from_slice(data);
        let start = png.len() - data.len() - 4;
        png.extend_from_slice(&crc32(&png[start..]).to_be_bytes());
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
    fn streams_grayscale_rows_and_preserves_grayscale_alpha() {
        let grayscale = encode_png(
            3,
            1,
            ColorType::Grayscale,
            BitDepth::Eight,
            Filter::Sub,
            &[7, 80, 201],
        );
        let mut grayscale_rgba = Vec::new();
        decode_png_rows(&grayscale, &default_options(), |row| {
            grayscale_rgba.extend_from_slice(row.pixels);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            grayscale_rgba,
            [7, 7, 7, 255, 80, 80, 80, 255, 201, 201, 201, 255]
        );

        let grayscale_alpha = encode_png(
            2,
            1,
            ColorType::GrayscaleAlpha,
            BitDepth::Eight,
            Filter::Paeth,
            &[31, 0, 190, 117],
        );
        let mut grayscale_alpha_rgba = Vec::new();
        decode_png_rows(&grayscale_alpha, &default_options(), |row| {
            grayscale_alpha_rgba.extend_from_slice(row.pixels);
            Ok(())
        })
        .unwrap();
        assert_eq!(grayscale_alpha_rgba, [31, 31, 31, 0, 190, 190, 190, 117]);
    }

    #[test]
    fn rejects_separate_grayscale_transparency_before_rows() {
        let mut encoded = Vec::new();
        let mut encoder = png::Encoder::new(&mut encoded, 1, 1);
        encoder.set_color(ColorType::Grayscale);
        encoder.set_depth(BitDepth::Eight);
        encoder.set_trns(vec![0, 10]);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[10]).unwrap();
        writer.finish().unwrap();
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
                feature: UnsupportedFeature::ColorType,
                ..
            }
        ));
    }

    #[test]
    fn expands_palette_rows_with_transparency() {
        let palette = [
            255, 0, 0, // red
            0, 255, 0, // green
            0, 0, 255, // blue
            240, 240, 240, // light gray
        ];
        let encoded =
            encode_palette_png(4, 1, BitDepth::Two, &[0, 1, 2, 3], &palette, &[0, 64, 128]);
        let mut rgba = Vec::new();

        decode_png_rows(&encoded, &default_options(), |row| {
            rgba.extend_from_slice(row.pixels);
            Ok(())
        })
        .unwrap();

        assert_eq!(
            rgba,
            [
                255, 0, 0, 0, 0, 255, 0, 64, 0, 0, 255, 128, 240, 240, 240, 255,
            ]
        );
    }

    #[test]
    fn rejects_palette_indices_outside_plte_for_sequential_and_adam7() {
        let palette = [12, 34, 56];
        let sequential = encode_palette_png(1, 1, BitDepth::Two, &[3], &palette, &[]);
        let adam7 = encode_adam7_palette_png(1, 1, BitDepth::Two, &[3], &palette, &[]);
        let mut callbacks = 0;

        assert!(
            decode_png_rows(&sequential, &default_options(), |_| {
                callbacks += 1;
                Ok(())
            })
            .is_err()
        );
        assert_eq!(callbacks, 0);
        assert!(thumbnail_png_rgba(&adam7, &default_options()).is_err());
    }

    #[test]
    fn rejects_invalid_palette_and_transparency_lengths_before_rows() {
        let too_many_colors =
            encode_palette_png(1, 1, BitDepth::One, &[0], &[1, 2, 3, 4, 5, 6, 7, 8, 9], &[]);
        let too_much_transparency =
            encode_palette_png(1, 1, BitDepth::One, &[0], &[1, 2, 3], &[0, 128]);

        for encoded in [too_many_colors, too_much_transparency] {
            let mut callbacks = 0;
            assert!(
                decode_png_rows(&encoded, &default_options(), |_| {
                    callbacks += 1;
                    Ok(())
                })
                .is_err()
            );
            assert_eq!(callbacks, 0);
        }
    }

    #[test]
    fn adam7_rgb_and_rgba_match_non_interlaced_thumbnails() {
        for color in [ColorType::Rgb, ColorType::Rgba] {
            let sample_bytes = if color == ColorType::Rgb { 3 } else { 4 };
            let width = 11_u32;
            let height = 9_u32;
            let mut pixels = Vec::new();
            for y in 0..height {
                for x in 0..width {
                    pixels.extend_from_slice(&[
                        u8::try_from((x * 31 + y * 7) % 256).unwrap(),
                        u8::try_from((x * 11 + y * 43) % 256).unwrap(),
                        u8::try_from((x * 53 + y * 3) % 256).unwrap(),
                    ]);
                    if sample_bytes == 4 {
                        pixels.push(u8::try_from((x * 29 + y * 47) % 256).unwrap());
                    }
                }
            }
            let adam7 = encode_adam7_png(width, height, color, &pixels);
            let sequential = encode_png(
                width,
                height,
                color,
                BitDepth::Eight,
                Filter::Paeth,
                &pixels,
            );
            let mut options = default_options();
            options.max_width = 5;
            options.max_height = 4;

            assert_eq!(
                thumbnail_png_rgba(&adam7, &options).unwrap(),
                thumbnail_png_rgba(&sequential, &options).unwrap()
            );
        }
    }

    #[test]
    fn adam7_grayscale_formats_match_non_interlaced_thumbnails() {
        for color in [ColorType::Grayscale, ColorType::GrayscaleAlpha] {
            let sample_bytes = if color == ColorType::Grayscale { 1 } else { 2 };
            let width = 13_u32;
            let height = 10_u32;
            let mut pixels = Vec::new();
            for y in 0..height {
                for x in 0..width {
                    pixels.push(u8::try_from((x * 37 + y * 19) % 256).unwrap());
                    if sample_bytes == 2 {
                        pixels.push(u8::try_from((x * 13 + y * 41) % 256).unwrap());
                    }
                }
            }
            let adam7 = encode_adam7_png(width, height, color, &pixels);
            let sequential = encode_png(
                width,
                height,
                color,
                BitDepth::Eight,
                Filter::Paeth,
                &pixels,
            );
            let mut options = default_options();
            options.max_width = 6;
            options.max_height = 5;

            assert_eq!(
                thumbnail_png_rgba(&adam7, &options).unwrap(),
                thumbnail_png_rgba(&sequential, &options).unwrap()
            );
        }
    }

    #[test]
    fn adam7_sixteen_bit_formats_match_non_interlaced_thumbnails() {
        for (color, channels) in [
            (ColorType::Grayscale, 1_u32),
            (ColorType::GrayscaleAlpha, 2_u32),
            (ColorType::Rgb, 3_u32),
            (ColorType::Rgba, 4_u32),
        ] {
            let width = 11_u32;
            let height = 9_u32;
            let mut pixels = Vec::new();
            for y in 0..height {
                for x in 0..width {
                    for channel in 0..channels {
                        let value =
                            u16::try_from((x * 7_919 + y * 4_099 + channel * 16_381) % 65_536)
                                .unwrap();
                        pixels.extend_from_slice(&value.to_be_bytes());
                    }
                }
            }
            let sequential = encode_png(
                width,
                height,
                color,
                BitDepth::Sixteen,
                Filter::Paeth,
                &pixels,
            );
            let adam7 = encode_adam7_direct_png(width, height, color, BitDepth::Sixteen, &pixels);
            let mut options = default_options();
            options.max_width = 5;
            options.max_height = 4;

            assert_eq!(
                thumbnail_png_rgba(&adam7, &options).unwrap(),
                thumbnail_png_rgba(&sequential, &options).unwrap(),
                "16-bit Adam7 mismatch for {color:?}"
            );
        }
    }

    #[test]
    fn adam7_palette_depths_match_non_interlaced_thumbnails() {
        for depth in [
            BitDepth::One,
            BitDepth::Two,
            BitDepth::Four,
            BitDepth::Eight,
        ] {
            let capacity = 1_usize << test_depth_bits(depth);
            let entries = capacity.min(17);
            let mut palette = Vec::new();
            let mut transparency = Vec::new();
            for index in 0..entries {
                let value = u8::try_from(index).unwrap();
                palette.extend_from_slice(&[
                    value.wrapping_mul(31),
                    value.wrapping_mul(67),
                    value.wrapping_mul(113),
                ]);
                if index + 1 < entries {
                    transparency.push(value.wrapping_mul(47));
                }
            }
            let width = 13_u32;
            let height = 10_u32;
            let indices = (0..width * height)
                .map(|index| u8::try_from(index as usize % entries).unwrap())
                .collect::<Vec<_>>();
            let sequential =
                encode_palette_png(width, height, depth, &indices, &palette, &transparency);
            let adam7 =
                encode_adam7_palette_png(width, height, depth, &indices, &palette, &transparency);
            let mut options = default_options();
            options.max_width = 6;
            options.max_height = 5;

            assert_eq!(
                thumbnail_png_rgba(&adam7, &options).unwrap(),
                thumbnail_png_rgba(&sequential, &options).unwrap(),
                "palette mismatch at {depth:?}"
            );
        }
    }

    #[test]
    fn adam7_handles_small_dimensions_and_empty_passes() {
        for height in 1..=9_u32 {
            for width in 1..=9_u32 {
                let pixels = (0..width * height)
                    .flat_map(|index| {
                        let value = u8::try_from(index % 251).unwrap();
                        [value, value.wrapping_add(1), value.wrapping_add(2), 255]
                    })
                    .collect::<Vec<_>>();
                let adam7 = encode_adam7_png(width, height, ColorType::Rgba, &pixels);
                let sequential = encode_png(
                    width,
                    height,
                    ColorType::Rgba,
                    BitDepth::Eight,
                    Filter::NoFilter,
                    &pixels,
                );
                let mut options = default_options();
                options.max_width = 3;
                options.max_height = 3;
                assert_eq!(
                    thumbnail_png_rgba(&adam7, &options).unwrap(),
                    thumbnail_png_rgba(&sequential, &options).unwrap(),
                    "Adam7 mismatch for {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn adam7_enforces_sparse_memory_plan_before_decoding() {
        let width = 64;
        let height = 64;
        let encoded = encode_adam7_png(
            width,
            height,
            ColorType::Rgba,
            &vec![128; width as usize * height as usize * 4],
        );
        let mut options = default_options();
        options.max_width = width;
        options.max_height = height;
        options.output = OutputFormat::Rgba;
        let encoded_bytes = u64::try_from(encoded.len()).unwrap();
        let required = plan_thumbnail_sparse(
            InputInfo {
                dimensions: Dimensions::new(width, height).unwrap(),
                encoded_bytes,
                source_bytes_per_pixel: 4,
            },
            &options,
        )
        .unwrap()
        .memory
        .total_bytes;
        options.limits.max_working_memory_bytes = required - 1;

        assert!(matches!(
            thumbnail_png_rgba(&encoded, &options).unwrap_err(),
            Error::Core(CoreError::LimitExceeded {
                kind: LimitKind::WorkingMemory,
                ..
            })
        ));
    }

    #[test]
    fn adam7_truncation_fails_without_panicking() {
        for (color, depth, sample_bytes) in [
            (ColorType::GrayscaleAlpha, BitDepth::Eight, 2_usize),
            (ColorType::Rgba, BitDepth::Eight, 4_usize),
            (ColorType::Rgba, BitDepth::Sixteen, 8_usize),
        ] {
            let pixels = vec![42; 17 * 13 * sample_bytes];
            let mut encoded = encode_adam7_direct_png(17, 13, color, depth, &pixels);
            encoded.truncate(encoded.len() - 20);
            assert!(thumbnail_png_rgba(&encoded, &default_options()).is_err());
        }
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
    fn normalizes_sixteen_bit_direct_color_with_rounding() {
        let cases = [
            (
                ColorType::Grayscale,
                vec![0x80, 0x00],
                vec![128, 128, 128, 255],
            ),
            (
                ColorType::GrayscaleAlpha,
                vec![0x12, 0x34, 0xab, 0xcd],
                vec![18, 18, 18, 171],
            ),
            (
                ColorType::Rgb,
                vec![0, 0, 0x80, 0, 0xff, 0xff],
                vec![0, 128, 255, 255],
            ),
            (
                ColorType::Rgba,
                vec![0x01, 0x01, 0x7f, 0xff, 0x80, 0, 0xff, 0xff],
                vec![1, 127, 128, 255],
            ),
        ];

        for (color, source, expected) in cases {
            let encoded = encode_png(1, 1, color, BitDepth::Sixteen, Filter::Paeth, &source);
            let mut actual = Vec::new();
            let info = decode_png_rows(&encoded, &default_options(), |row| {
                actual.extend_from_slice(row.pixels);
                Ok(())
            })
            .unwrap();
            assert_eq!(actual, expected, "16-bit mismatch for {color:?}");
            if color == ColorType::Rgba {
                assert_eq!(info.plan.memory.decoder_rows_bytes, 27);
            }
        }
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
