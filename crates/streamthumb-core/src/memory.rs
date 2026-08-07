use crate::{Dimensions, Error, OutputFormat, Result};

const NORMALIZED_PIXEL_BYTES: usize = 4;
const HORIZONTAL_ACCUMULATOR_BYTES_PER_PIXEL: usize = 5 * size_of::<u128>();
const VERTICAL_ACCUMULATOR_BYTES_PER_PIXEL: usize = 5 * size_of::<u128>();
const SPARSE_ACCUMULATOR_BYTES_PER_PIXEL: usize = 5 * size_of::<u128>();
const DECODER_STAGING_BYTES: usize = 160 * 1024;
const PNG_ENCODER_STATE_BYTES: usize = 128 * 1024;
const JPEG_FIXED_ENCODER_STATE_BYTES: usize = 64 * 1024;
const JPEG_MCU_ROWS: usize = 16;
const JPEG_RGB_PIXEL_BYTES: usize = 3;
const JPEG_INTERNAL_BYTES_PER_PIXEL: usize = 12;
const JPEG_MAX_ENCODED_BYTES_PER_BLOCK: usize = 420;
const JPEG_CONTAINER_ALLOWANCE_BYTES: usize = 64 * 1024;

/// A conservative breakdown of buffers owned by the planned streaming path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryEstimate {
    pub decoder_rows_bytes: usize,
    pub decoder_staging_bytes: usize,
    pub normalized_row_bytes: usize,
    pub horizontal_accumulator_bytes: usize,
    pub vertical_accumulator_bytes: usize,
    pub sparse_accumulator_bytes: usize,
    pub output_row_bytes: usize,
    pub output_rgba_bytes: usize,
    pub encoder_state_bytes: usize,
    pub encoded_output_bytes: usize,
    pub total_bytes: usize,
}

/// Estimates working memory without including encoded input storage.
///
/// The decoder allowance includes three packed source rows and conservative
/// staging space for DEFLATE history and buffered decompressed data. Accumulator
/// constants reserve four premultiplied `u128` color channels plus one weight per
/// output pixel. Raw RGBA output retains the full result. Buffered encoded
/// output retains one completed RGBA row, codec state, and the bounded encoded
/// result; direct writer plans exclude the caller-owned encoded destination.
pub fn estimate_working_memory(
    source: Dimensions,
    output: Dimensions,
    source_bytes_per_pixel: u8,
) -> Result<MemoryEstimate> {
    estimate_working_memory_for_output(source, output, source_bytes_per_pixel, OutputFormat::Rgba)
}

/// Estimates working memory for a specific public output representation.
pub fn estimate_working_memory_for_output(
    source: Dimensions,
    output: Dimensions,
    source_bytes_per_pixel: u8,
    output_format: OutputFormat,
) -> Result<MemoryEstimate> {
    estimate_working_memory_for_output_layout(
        source,
        output,
        source_bytes_per_pixel,
        output_format,
        true,
    )
}

pub(crate) fn estimate_writer_working_memory_for_output(
    source: Dimensions,
    output: Dimensions,
    source_bytes_per_pixel: u8,
    output_format: OutputFormat,
) -> Result<MemoryEstimate> {
    estimate_working_memory_for_output_layout(
        source,
        output,
        source_bytes_per_pixel,
        output_format,
        false,
    )
}

fn estimate_working_memory_for_output_layout(
    source: Dimensions,
    output: Dimensions,
    source_bytes_per_pixel: u8,
    output_format: OutputFormat,
    retain_encoded_output: bool,
) -> Result<MemoryEstimate> {
    if source_bytes_per_pixel == 0 {
        return Err(Error::InvalidSourceBytesPerPixel);
    }

    let source_width = usize::try_from(source.width).map_err(|_| overflow("source width"))?;
    let output_width = usize::try_from(output.width).map_err(|_| overflow("output width"))?;
    let output_pixels =
        usize::try_from(output.pixels()?).map_err(|_| overflow("output pixel count conversion"))?;

    let packed_row_bytes = checked_product(
        &[source_width, usize::from(source_bytes_per_pixel)],
        "packed decoder row",
    )?
    .checked_add(1)
    .ok_or_else(|| overflow("PNG filter byte"))?;
    let decoder_rows_bytes = checked_product(&[packed_row_bytes, 3], "decoder row buffers")?;
    let decoder_staging_bytes = DECODER_STAGING_BYTES;
    let normalized_row_bytes = checked_product(
        &[source_width, NORMALIZED_PIXEL_BYTES],
        "normalized row buffer",
    )?;
    let horizontal_accumulator_bytes = checked_product(
        &[output_width, HORIZONTAL_ACCUMULATOR_BYTES_PER_PIXEL],
        "horizontal accumulator",
    )?;
    let vertical_accumulator_bytes = checked_product(
        &[output_width, VERTICAL_ACCUMULATOR_BYTES_PER_PIXEL],
        "vertical accumulator",
    )?;
    let sparse_accumulator_bytes = 0;
    let output_row_bytes =
        checked_product(&[output_width, NORMALIZED_PIXEL_BYTES], "output RGBA row")?;
    let raw_rgba_bytes = checked_product(
        &[output_pixels, NORMALIZED_PIXEL_BYTES],
        "output RGBA buffer",
    )?;
    let (output_rgba_bytes, encoder_state_bytes, encoded_output_limit_bytes) = match output_format {
        OutputFormat::Rgba => (raw_rgba_bytes, 0, 0),
        OutputFormat::Png => (
            0,
            PNG_ENCODER_STATE_BYTES,
            estimate_encoded_png_bytes(output, raw_rgba_bytes)?,
        ),
        OutputFormat::Jpeg => (
            0,
            estimate_jpeg_encoder_state_bytes(output)?,
            estimate_encoded_jpeg_bytes(output)?,
        ),
    };
    let encoded_output_bytes = if retain_encoded_output {
        encoded_output_limit_bytes
    } else {
        0
    };

    let total_bytes = checked_sum(
        &[
            decoder_rows_bytes,
            decoder_staging_bytes,
            normalized_row_bytes,
            horizontal_accumulator_bytes,
            vertical_accumulator_bytes,
            sparse_accumulator_bytes,
            output_row_bytes,
            output_rgba_bytes,
            encoder_state_bytes,
            encoded_output_bytes,
        ],
        "total working memory",
    )?;

    Ok(MemoryEstimate {
        decoder_rows_bytes,
        decoder_staging_bytes,
        normalized_row_bytes,
        horizontal_accumulator_bytes,
        vertical_accumulator_bytes,
        sparse_accumulator_bytes,
        output_row_bytes,
        output_rgba_bytes,
        encoder_state_bytes,
        encoded_output_bytes,
        total_bytes,
    })
}

/// Estimates working memory for arbitrary-order sparse source samples.
///
/// Sparse accumulation replaces the two row accumulators with one accumulator
/// per bounded output pixel. This supports Adam7 pass order without retaining a
/// full-resolution source frame.
pub fn estimate_sparse_working_memory_for_output(
    source: Dimensions,
    output: Dimensions,
    source_bytes_per_pixel: u8,
    output_format: OutputFormat,
) -> Result<MemoryEstimate> {
    let mut estimate =
        estimate_working_memory_for_output(source, output, source_bytes_per_pixel, output_format)?;
    let output_pixels =
        usize::try_from(output.pixels()?).map_err(|_| overflow("sparse output pixel count"))?;
    let sparse_accumulator_bytes = checked_product(
        &[output_pixels, SPARSE_ACCUMULATOR_BYTES_PER_PIXEL],
        "sparse output accumulator",
    )?;
    estimate.total_bytes = estimate
        .total_bytes
        .checked_sub(estimate.horizontal_accumulator_bytes)
        .and_then(|value| value.checked_sub(estimate.vertical_accumulator_bytes))
        .and_then(|value| value.checked_add(sparse_accumulator_bytes))
        .ok_or_else(|| overflow("sparse total working memory"))?;
    estimate.horizontal_accumulator_bytes = 0;
    estimate.vertical_accumulator_bytes = 0;
    estimate.sparse_accumulator_bytes = sparse_accumulator_bytes;
    Ok(estimate)
}

pub(crate) fn estimate_sparse_writer_working_memory_for_output(
    source: Dimensions,
    output: Dimensions,
    source_bytes_per_pixel: u8,
    output_format: OutputFormat,
) -> Result<MemoryEstimate> {
    let mut estimate = estimate_writer_working_memory_for_output(
        source,
        output,
        source_bytes_per_pixel,
        output_format,
    )?;
    let output_pixels =
        usize::try_from(output.pixels()?).map_err(|_| overflow("sparse output pixel count"))?;
    let sparse_accumulator_bytes = checked_product(
        &[output_pixels, SPARSE_ACCUMULATOR_BYTES_PER_PIXEL],
        "sparse output accumulator",
    )?;
    estimate.total_bytes = estimate
        .total_bytes
        .checked_sub(estimate.horizontal_accumulator_bytes)
        .and_then(|value| value.checked_sub(estimate.vertical_accumulator_bytes))
        .and_then(|value| value.checked_add(sparse_accumulator_bytes))
        .ok_or_else(|| overflow("sparse writer total working memory"))?;
    estimate.horizontal_accumulator_bytes = 0;
    estimate.vertical_accumulator_bytes = 0;
    estimate.sparse_accumulator_bytes = sparse_accumulator_bytes;
    Ok(estimate)
}

pub(crate) fn estimate_encoded_output_limit_bytes(
    output: Dimensions,
    output_format: OutputFormat,
) -> Result<usize> {
    match output_format {
        OutputFormat::Png => {
            let pixels = usize::try_from(output.pixels()?)
                .map_err(|_| overflow("output pixel count conversion"))?;
            let rgba_bytes = checked_product(&[pixels, NORMALIZED_PIXEL_BYTES], "output RGBA")?;
            estimate_encoded_png_bytes(output, rgba_bytes)
        }
        OutputFormat::Jpeg => estimate_encoded_jpeg_bytes(output),
        OutputFormat::Rgba => Ok(0),
    }
}

fn estimate_encoded_png_bytes(output: Dimensions, rgba_bytes: usize) -> Result<usize> {
    let filtered_bytes = rgba_bytes
        .checked_add(usize::try_from(output.height).map_err(|_| overflow("output height"))?)
        .ok_or_else(|| overflow("filtered PNG bytes"))?;
    let deflate_allowance = filtered_bytes
        .checked_add(filtered_bytes / 8)
        .ok_or_else(|| overflow("encoded PNG DEFLATE allowance"))?;
    deflate_allowance
        .checked_add(64 * 1024)
        .ok_or_else(|| overflow("encoded PNG container allowance"))
}

fn estimate_jpeg_encoder_state_bytes(output: Dimensions) -> Result<usize> {
    let width = usize::try_from(output.width).map_err(|_| overflow("JPEG output width"))?;
    let mcu_rows_bytes = checked_product(
        &[
            width,
            JPEG_MCU_ROWS,
            JPEG_RGB_PIXEL_BYTES + JPEG_INTERNAL_BYTES_PER_PIXEL,
        ],
        "JPEG MCU row buffer",
    )?;
    let temporary_segment_bytes = estimate_encoded_jpeg_mcu_row_bytes(output)?;
    checked_sum(
        &[
            JPEG_FIXED_ENCODER_STATE_BYTES,
            mcu_rows_bytes,
            temporary_segment_bytes,
        ],
        "JPEG encoder state",
    )
}

fn estimate_encoded_jpeg_mcu_row_bytes(output: Dimensions) -> Result<usize> {
    let block_columns = usize::try_from(output.width.div_ceil(8))
        .map_err(|_| overflow("JPEG segment block columns"))?;
    let color_blocks = checked_product(&[block_columns, 2, 3], "JPEG segment color block count")?;
    checked_product(
        &[color_blocks, JPEG_MAX_ENCODED_BYTES_PER_BLOCK],
        "JPEG segment entropy allowance",
    )?
    .checked_add(JPEG_CONTAINER_ALLOWANCE_BYTES)
    .ok_or_else(|| overflow("JPEG segment container allowance"))
}

fn estimate_encoded_jpeg_bytes(output: Dimensions) -> Result<usize> {
    let block_columns =
        usize::try_from(output.width.div_ceil(8)).map_err(|_| overflow("JPEG block columns"))?;
    let block_rows =
        usize::try_from(output.height.div_ceil(8)).map_err(|_| overflow("JPEG block rows"))?;
    let color_blocks = checked_product(&[block_columns, block_rows, 3], "JPEG color block count")?;
    checked_product(
        &[color_blocks, JPEG_MAX_ENCODED_BYTES_PER_BLOCK],
        "JPEG entropy allowance",
    )?
    .checked_add(JPEG_CONTAINER_ALLOWANCE_BYTES)
    .ok_or_else(|| overflow("JPEG container allowance"))
}

fn checked_product(values: &[usize], operation: &'static str) -> Result<usize> {
    values.iter().try_fold(1_usize, |product, value| {
        product
            .checked_mul(*value)
            .ok_or_else(|| overflow(operation))
    })
}

fn checked_sum(values: &[usize], operation: &'static str) -> Result<usize> {
    values.iter().try_fold(0_usize, |sum, value| {
        sum.checked_add(*value).ok_or_else(|| overflow(operation))
    })
}

const fn overflow(operation: &'static str) -> Error {
    Error::IntegerOverflow { operation }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_has_an_auditable_breakdown() {
        let estimate = estimate_working_memory(
            Dimensions::new(1_000, 500).unwrap(),
            Dimensions::new(100, 50).unwrap(),
            4,
        )
        .unwrap();

        assert_eq!(estimate.decoder_rows_bytes, 12_003);
        assert_eq!(estimate.decoder_staging_bytes, 163_840);
        assert_eq!(estimate.normalized_row_bytes, 4_000);
        assert_eq!(estimate.horizontal_accumulator_bytes, 8_000);
        assert_eq!(estimate.vertical_accumulator_bytes, 8_000);
        assert_eq!(estimate.sparse_accumulator_bytes, 0);
        assert_eq!(estimate.output_row_bytes, 400);
        assert_eq!(estimate.output_rgba_bytes, 20_000);
        assert_eq!(estimate.encoder_state_bytes, 0);
        assert_eq!(estimate.encoded_output_bytes, 0);
        assert_eq!(estimate.total_bytes, 216_243);
    }

    #[test]
    fn png_output_includes_encoder_and_bounded_output_storage() {
        let source = Dimensions::new(1_000, 500).unwrap();
        let output = Dimensions::new(100, 50).unwrap();
        let rgba = estimate_working_memory(source, output, 4).unwrap();
        let png = estimate_working_memory_for_output(source, output, 4, OutputFormat::Png).unwrap();

        assert_eq!(png.output_rgba_bytes, 0);
        assert_eq!(png.encoder_state_bytes, 128 * 1024);
        assert!(png.encoded_output_bytes > rgba.output_rgba_bytes);
        assert_eq!(
            png.total_bytes,
            rgba.total_bytes - rgba.output_rgba_bytes
                + png.encoder_state_bytes
                + png.encoded_output_bytes
        );
    }

    #[test]
    fn png_output_does_not_retain_a_full_rgba_frame() {
        let source = Dimensions::new(2_048, 2_048).unwrap();
        let output = Dimensions::new(2_048, 2_048).unwrap();
        let estimate =
            estimate_working_memory_for_output(source, output, 4, OutputFormat::Png).unwrap();

        assert_eq!(estimate.output_row_bytes, 2_048 * 4);
        assert_eq!(estimate.output_rgba_bytes, 0);
        assert_eq!(estimate.total_bytes, 19_605_763);
    }

    #[test]
    fn jpeg_output_is_bounded_by_width_and_encoded_size() {
        let source = Dimensions::new(2_048, 2_048).unwrap();
        let short = estimate_working_memory_for_output(
            source,
            Dimensions::new(2_048, 16).unwrap(),
            4,
            OutputFormat::Jpeg,
        )
        .unwrap();
        let tall = estimate_working_memory_for_output(
            source,
            Dimensions::new(2_048, 2_048).unwrap(),
            4,
            OutputFormat::Jpeg,
        )
        .unwrap();

        assert_eq!(short.output_rgba_bytes, 0);
        assert_eq!(short.encoder_state_bytes, tall.encoder_state_bytes);
        assert!(tall.encoded_output_bytes > short.encoded_output_bytes);
    }

    #[test]
    fn rejects_zero_source_bytes_per_pixel() {
        assert_eq!(
            estimate_working_memory(
                Dimensions::new(1, 1).unwrap(),
                Dimensions::new(1, 1).unwrap(),
                0,
            ),
            Err(Error::InvalidSourceBytesPerPixel)
        );
    }

    #[test]
    fn estimate_does_not_scale_with_source_height() {
        let output = Dimensions::new(100, 100).unwrap();
        let short = estimate_working_memory(Dimensions::new(4_000, 1).unwrap(), output, 4).unwrap();
        let tall =
            estimate_working_memory(Dimensions::new(4_000, 100_000).unwrap(), output, 4).unwrap();

        assert_eq!(short, tall);
    }

    #[test]
    fn checked_arithmetic_reports_overflow() {
        assert_eq!(
            checked_product(&[usize::MAX, 2], "test product"),
            Err(Error::IntegerOverflow {
                operation: "test product"
            })
        );
        assert_eq!(
            checked_sum(&[usize::MAX, 1], "test sum"),
            Err(Error::IntegerOverflow {
                operation: "test sum"
            })
        );
    }

    #[test]
    fn sparse_estimate_scales_with_output_area_not_source_area() {
        let output = Dimensions::new(100, 50).unwrap();
        let short = estimate_sparse_working_memory_for_output(
            Dimensions::new(4_000, 1).unwrap(),
            output,
            4,
            OutputFormat::Rgba,
        )
        .unwrap();
        let tall = estimate_sparse_working_memory_for_output(
            Dimensions::new(4_000, 100_000).unwrap(),
            output,
            4,
            OutputFormat::Rgba,
        )
        .unwrap();

        assert_eq!(short, tall);
        assert_eq!(short.horizontal_accumulator_bytes, 0);
        assert_eq!(short.vertical_accumulator_bytes, 0);
        assert_eq!(short.sparse_accumulator_bytes, 100 * 50 * 5 * 16);
    }
}
