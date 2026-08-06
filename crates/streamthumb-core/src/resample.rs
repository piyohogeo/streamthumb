use crate::{Dimensions, Error, Result};

/// A completed straight-alpha RGBA8 image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RgbaImage {
    pub dimensions: Dimensions,
    pub pixels: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Accumulator {
    red: u128,
    green: u128,
    blue: u128,
    alpha: u128,
    weight: u128,
}

/// A row-oriented exact area downsampler using premultiplied-alpha accumulation.
///
/// Source rows must be pushed once in ascending order. Memory use depends on
/// source width and output size, but not on source height.
#[derive(Debug)]
pub struct AreaDownsampler {
    source: Dimensions,
    output: Dimensions,
    next_source_y: u32,
    current_output_y: u32,
    horizontal: Vec<Accumulator>,
    vertical: Vec<Accumulator>,
    output_pixels: Vec<u8>,
}

impl AreaDownsampler {
    /// Creates an area downsampler for fixed source and output dimensions.
    pub fn new(source: Dimensions, output: Dimensions) -> Result<Self> {
        let output_width = usize::try_from(output.width).map_err(|_| overflow("output width"))?;
        let output_bytes = usize::try_from(output.pixels()?)
            .map_err(|_| overflow("output pixel count conversion"))?
            .checked_mul(4)
            .ok_or_else(|| overflow("output RGBA byte count"))?;

        Ok(Self {
            source,
            output,
            next_source_y: 0,
            current_output_y: 0,
            horizontal: allocate_accumulators(output_width)?,
            vertical: allocate_accumulators(output_width)?,
            output_pixels: allocate_bytes(output_bytes)?,
        })
    }

    /// Adds the next normalized straight-alpha RGBA8 source row.
    pub fn push_row(&mut self, y: u32, pixels: &[u8]) -> Result<()> {
        if y != self.next_source_y {
            return Err(Error::UnexpectedRow {
                expected: self.next_source_y,
                actual: y,
            });
        }
        let expected_len = usize::try_from(self.source.width)
            .map_err(|_| overflow("source width"))?
            .checked_mul(4)
            .ok_or_else(|| overflow("source RGBA row length"))?;
        if pixels.len() != expected_len {
            return Err(Error::InvalidRowLength {
                expected: expected_len,
                actual: pixels.len(),
            });
        }

        self.reduce_horizontal(pixels)?;
        self.accumulate_vertical(y)?;
        self.next_source_y = self
            .next_source_y
            .checked_add(1)
            .ok_or_else(|| overflow("source row index"))?;
        Ok(())
    }

    /// Completes all output rows and returns the thumbnail buffer.
    pub fn finish(mut self) -> Result<RgbaImage> {
        if self.next_source_y != self.source.height {
            return Err(Error::IncompleteImage {
                expected_rows: self.source.height,
                actual_rows: self.next_source_y,
            });
        }
        while self.current_output_y < self.output.height {
            self.finalize_output_row()?;
        }

        Ok(RgbaImage {
            dimensions: self.output,
            pixels: self.output_pixels,
        })
    }

    fn reduce_horizontal(&mut self, pixels: &[u8]) -> Result<()> {
        self.horizontal.fill(Accumulator::default());
        let source_width = u64::from(self.source.width);
        let output_width = u64::from(self.output.width);

        for source_x in 0..source_width {
            let source_start = source_x
                .checked_mul(output_width)
                .ok_or_else(|| overflow("horizontal source interval"))?;
            let source_end = source_start
                .checked_add(output_width)
                .ok_or_else(|| overflow("horizontal source interval"))?;
            let first_output_x = source_start / source_width;
            let last_output_x = div_ceil(source_end, source_width)?;
            let pixel_offset = usize::try_from(source_x)
                .map_err(|_| overflow("source pixel index"))?
                .checked_mul(4)
                .ok_or_else(|| overflow("source pixel offset"))?;
            let red = u128::from(pixels[pixel_offset]);
            let green = u128::from(pixels[pixel_offset + 1]);
            let blue = u128::from(pixels[pixel_offset + 2]);
            let alpha = u128::from(pixels[pixel_offset + 3]);

            for output_x in first_output_x..last_output_x {
                let output_start = output_x
                    .checked_mul(source_width)
                    .ok_or_else(|| overflow("horizontal output interval"))?;
                let output_end = output_start
                    .checked_add(source_width)
                    .ok_or_else(|| overflow("horizontal output interval"))?;
                let overlap = interval_overlap(source_start, source_end, output_start, output_end);
                if overlap == 0 {
                    continue;
                }
                let accumulator = &mut self.horizontal
                    [usize::try_from(output_x).map_err(|_| overflow("output pixel index"))?];
                let weight = u128::from(overlap);
                accumulator.red += red * alpha * weight;
                accumulator.green += green * alpha * weight;
                accumulator.blue += blue * alpha * weight;
                accumulator.alpha += alpha * weight;
                accumulator.weight += weight;
            }
        }
        Ok(())
    }

    fn accumulate_vertical(&mut self, source_y: u32) -> Result<()> {
        let source_height = u64::from(self.source.height);
        let output_height = u64::from(self.output.height);
        let source_start = u64::from(source_y)
            .checked_mul(output_height)
            .ok_or_else(|| overflow("vertical source interval"))?;
        let source_end = source_start
            .checked_add(output_height)
            .ok_or_else(|| overflow("vertical source interval"))?;
        let first_output_y = source_start / source_height;
        let last_output_y = div_ceil(source_end, source_height)?;

        for output_y in first_output_y..last_output_y {
            let output_y_u32 =
                u32::try_from(output_y).map_err(|_| overflow("vertical output row conversion"))?;
            while self.current_output_y < output_y_u32 {
                self.finalize_output_row()?;
            }

            let output_start = output_y
                .checked_mul(source_height)
                .ok_or_else(|| overflow("vertical output interval"))?;
            let output_end = output_start
                .checked_add(source_height)
                .ok_or_else(|| overflow("vertical output interval"))?;
            let overlap = u128::from(interval_overlap(
                source_start,
                source_end,
                output_start,
                output_end,
            ));
            for (vertical, horizontal) in self.vertical.iter_mut().zip(&self.horizontal) {
                vertical.red += horizontal.red * overlap;
                vertical.green += horizontal.green * overlap;
                vertical.blue += horizontal.blue * overlap;
                vertical.alpha += horizontal.alpha * overlap;
                vertical.weight += horizontal.weight * overlap;
            }
        }
        Ok(())
    }

    fn finalize_output_row(&mut self) -> Result<()> {
        let expected_weight = u128::from(self.source.width) * u128::from(self.source.height);
        for (x, accumulator) in self.vertical.iter().enumerate() {
            if accumulator.weight != expected_weight {
                return Err(Error::InvalidCoverage {
                    x: u32::try_from(x).map_err(|_| overflow("output x conversion"))?,
                    y: self.current_output_y,
                    expected: expected_weight,
                    actual: accumulator.weight,
                });
            }
            let alpha = rounded_div(accumulator.alpha, accumulator.weight)?;
            let (red, green, blue) = if accumulator.alpha == 0 {
                (0, 0, 0)
            } else {
                (
                    rounded_div(accumulator.red, accumulator.alpha)?,
                    rounded_div(accumulator.green, accumulator.alpha)?,
                    rounded_div(accumulator.blue, accumulator.alpha)?,
                )
            };
            self.output_pixels.extend_from_slice(&[
                to_u8(red)?,
                to_u8(green)?,
                to_u8(blue)?,
                to_u8(alpha)?,
            ]);
        }
        self.vertical.fill(Accumulator::default());
        self.current_output_y = self
            .current_output_y
            .checked_add(1)
            .ok_or_else(|| overflow("output row index"))?;
        Ok(())
    }
}

fn allocate_accumulators(len: usize) -> Result<Vec<Accumulator>> {
    let bytes = len
        .checked_mul(size_of::<Accumulator>())
        .ok_or_else(|| overflow("accumulator allocation size"))?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(len)
        .map_err(|_| Error::AllocationFailed { bytes })?;
    values.resize(len, Accumulator::default());
    Ok(values)
}

fn allocate_bytes(capacity: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(|_| Error::AllocationFailed { bytes: capacity })?;
    Ok(bytes)
}

const fn interval_overlap(a_start: u64, a_end: u64, b_start: u64, b_end: u64) -> u64 {
    let start = if a_start > b_start { a_start } else { b_start };
    let end = if a_end < b_end { a_end } else { b_end };
    end.saturating_sub(start)
}

fn div_ceil(numerator: u64, denominator: u64) -> Result<u64> {
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    quotient
        .checked_add(u64::from(remainder != 0))
        .ok_or_else(|| overflow("ceiling division"))
}

fn rounded_div(numerator: u128, denominator: u128) -> Result<u128> {
    if denominator == 0 {
        return Err(Error::IntegerOverflow {
            operation: "division by zero while normalizing area weights",
        });
    }
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    quotient
        .checked_add(u128::from(remainder >= denominator.div_ceil(2)))
        .ok_or_else(|| overflow("rounded division"))
}

fn to_u8(value: u128) -> Result<u8> {
    u8::try_from(value).map_err(|_| overflow("normalized channel conversion"))
}

const fn overflow(operation: &'static str) -> Error {
    Error::IntegerOverflow { operation }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resize(source: Dimensions, output: Dimensions, pixels: &[u8]) -> RgbaImage {
        let mut downsampler = AreaDownsampler::new(source, output).unwrap();
        let row_len = usize::try_from(source.width).unwrap() * 4;
        for (y, row) in pixels.chunks_exact(row_len).enumerate() {
            downsampler
                .push_row(u32::try_from(y).unwrap(), row)
                .unwrap();
        }
        downsampler.finish().unwrap()
    }

    #[test]
    fn averages_an_opaque_two_by_two_image() {
        let image = resize(
            Dimensions::new(2, 2).unwrap(),
            Dimensions::new(1, 1).unwrap(),
            &[
                0, 0, 0, 255, 100, 0, 0, 255, // row 0
                0, 100, 0, 255, 100, 100, 100, 255, // row 1
            ],
        );
        assert_eq!(image.pixels, [50, 50, 25, 255]);
    }

    #[test]
    fn handles_non_integer_horizontal_and_vertical_ratios() {
        let horizontal = resize(
            Dimensions::new(3, 1).unwrap(),
            Dimensions::new(2, 1).unwrap(),
            &[0, 0, 0, 255, 60, 60, 60, 255, 120, 120, 120, 255],
        );
        assert_eq!(horizontal.pixels, [20, 20, 20, 255, 100, 100, 100, 255]);

        let vertical = resize(
            Dimensions::new(1, 3).unwrap(),
            Dimensions::new(1, 2).unwrap(),
            &[0, 0, 0, 255, 60, 60, 60, 255, 120, 120, 120, 255],
        );
        assert_eq!(vertical.pixels, horizontal.pixels);
    }

    #[test]
    fn uses_premultiplied_alpha_without_transparent_color_bleed() {
        let image = resize(
            Dimensions::new(2, 1).unwrap(),
            Dimensions::new(1, 1).unwrap(),
            &[255, 0, 0, 0, 0, 0, 255, 255],
        );
        assert_eq!(image.pixels, [0, 0, 255, 128]);
    }

    #[test]
    fn gives_fully_transparent_pixels_canonical_zero_rgb() {
        let image = resize(
            Dimensions::new(2, 1).unwrap(),
            Dimensions::new(1, 1).unwrap(),
            &[255, 0, 0, 0, 0, 255, 0, 0],
        );
        assert_eq!(image.pixels, [0, 0, 0, 0]);
    }

    #[test]
    fn area_sampling_can_upscale_a_single_pixel() {
        let image = resize(
            Dimensions::new(1, 1).unwrap(),
            Dimensions::new(3, 2).unwrap(),
            &[10, 20, 30, 40],
        );
        assert_eq!(image.pixels, [10, 20, 30, 40].repeat(6));
    }

    #[test]
    fn matches_an_independent_full_frame_reference_for_arbitrary_ratios() {
        let source = Dimensions::new(7, 5).unwrap();
        let mut pixels = Vec::new();
        for y in 0..source.height {
            for x in 0..source.width {
                pixels.extend_from_slice(&[
                    u8::try_from((x * 31 + y * 7) % 256).unwrap(),
                    u8::try_from((x * 11 + y * 43) % 256).unwrap(),
                    u8::try_from((x * 53 + y * 3) % 256).unwrap(),
                    u8::try_from((x * 29 + y * 47) % 256).unwrap(),
                ]);
            }
        }

        for output in [
            Dimensions::new(3, 2).unwrap(),
            Dimensions::new(4, 3).unwrap(),
            Dimensions::new(1, 1).unwrap(),
            Dimensions::new(9, 6).unwrap(),
        ] {
            let actual = resize(source, output, &pixels);
            let expected = reference_area_resize(source, output, &pixels);
            assert_eq!(actual.pixels.len(), expected.len());
            for (index, (actual, expected)) in actual.pixels.iter().zip(expected.iter()).enumerate()
            {
                assert!(
                    actual.abs_diff(*expected) <= 1,
                    "byte {index} differs for {output:?}: actual {actual}, expected {expected}"
                );
            }
        }
    }

    #[test]
    fn rejects_out_of_order_and_wrong_length_rows() {
        let source = Dimensions::new(2, 2).unwrap();
        let output = Dimensions::new(1, 1).unwrap();
        let mut downsampler = AreaDownsampler::new(source, output).unwrap();
        assert_eq!(
            downsampler.push_row(1, &[0; 8]),
            Err(Error::UnexpectedRow {
                expected: 0,
                actual: 1
            })
        );
        assert_eq!(
            downsampler.push_row(0, &[0; 7]),
            Err(Error::InvalidRowLength {
                expected: 8,
                actual: 7
            })
        );
    }

    #[test]
    fn rejects_incomplete_images() {
        let downsampler = AreaDownsampler::new(
            Dimensions::new(1, 2).unwrap(),
            Dimensions::new(1, 1).unwrap(),
        )
        .unwrap();
        assert_eq!(
            downsampler.finish(),
            Err(Error::IncompleteImage {
                expected_rows: 2,
                actual_rows: 0
            })
        );
    }

    fn reference_area_resize(source: Dimensions, output: Dimensions, pixels: &[u8]) -> Vec<u8> {
        let mut result = Vec::new();
        for output_y in 0..output.height {
            let top = f64::from(output_y) * f64::from(source.height) / f64::from(output.height);
            let bottom =
                f64::from(output_y + 1) * f64::from(source.height) / f64::from(output.height);
            for output_x in 0..output.width {
                let left = f64::from(output_x) * f64::from(source.width) / f64::from(output.width);
                let right =
                    f64::from(output_x + 1) * f64::from(source.width) / f64::from(output.width);
                let mut premultiplied = [0.0_f64; 3];
                let mut alpha_sum = 0.0_f64;
                let mut weight_sum = 0.0_f64;

                for source_y in 0..source.height {
                    let overlap_y = (bottom.min(f64::from(source_y + 1))
                        - top.max(f64::from(source_y)))
                    .max(0.0);
                    for source_x in 0..source.width {
                        let overlap_x = (right.min(f64::from(source_x + 1))
                            - left.max(f64::from(source_x)))
                        .max(0.0);
                        let weight = overlap_x * overlap_y;
                        let offset = usize::try_from(
                            (u64::from(source_y) * u64::from(source.width) + u64::from(source_x))
                                * 4,
                        )
                        .unwrap();
                        let alpha = f64::from(pixels[offset + 3]);
                        for channel in 0..3 {
                            premultiplied[channel] +=
                                f64::from(pixels[offset + channel]) * alpha * weight;
                        }
                        alpha_sum += alpha * weight;
                        weight_sum += weight;
                    }
                }

                for value in premultiplied {
                    result.push(if alpha_sum == 0.0 {
                        0
                    } else {
                        u8::try_from((value / alpha_sum).round() as u16).unwrap()
                    });
                }
                result.push(u8::try_from((alpha_sum / weight_sum).round() as u16).unwrap());
            }
        }
        result
    }
}
