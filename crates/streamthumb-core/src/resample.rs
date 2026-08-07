use crate::{Dimensions, Error, Fit, Result};

/// A completed straight-alpha RGBA8 image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RgbaImage {
    pub dimensions: Dimensions,
    pub pixels: Vec<u8>,
}

/// Consumes completed straight-alpha RGBA8 thumbnail rows.
///
/// Rows are delivered exactly once in ascending order. The row slice is valid
/// only for the duration of `push_row` and must be copied if the sink needs to
/// retain it.
pub trait RgbaRowSink {
    type Output;
    type Error: From<Error>;

    fn push_row(&mut self, y: u32, rgba: &[u8]) -> core::result::Result<(), Self::Error>;
    fn finish(self) -> core::result::Result<Self::Output, Self::Error>;
}

/// Collects completed RGBA8 rows into a full bounded image.
///
/// This sink preserves the existing raw-RGBA behavior while making full-frame
/// output storage an explicit choice.
#[derive(Debug)]
pub struct RgbaCollector {
    dimensions: Dimensions,
    next_y: u32,
    pixels: Vec<u8>,
}

impl RgbaCollector {
    pub fn new(dimensions: Dimensions) -> Result<Self> {
        let output_bytes = rgba_image_bytes(dimensions, "RGBA collector")?;
        Ok(Self {
            dimensions,
            next_y: 0,
            pixels: allocate_bytes(output_bytes)?,
        })
    }
}

impl RgbaRowSink for RgbaCollector {
    type Output = RgbaImage;
    type Error = Error;

    fn push_row(&mut self, y: u32, rgba: &[u8]) -> Result<()> {
        if y != self.next_y {
            return Err(Error::UnexpectedRow {
                expected: self.next_y,
                actual: y,
            });
        }
        let expected_len = rgba_row_bytes(self.dimensions.width, "RGBA collector row")?;
        if rgba.len() != expected_len {
            return Err(Error::InvalidRowLength {
                expected: expected_len,
                actual: rgba.len(),
            });
        }
        self.pixels.extend_from_slice(rgba);
        self.next_y = self
            .next_y
            .checked_add(1)
            .ok_or_else(|| overflow("RGBA collector row index"))?;
        Ok(())
    }

    fn finish(self) -> Result<RgbaImage> {
        if self.next_y != self.dimensions.height {
            return Err(Error::IncompleteImage {
                expected_rows: self.dimensions.height,
                actual_rows: self.next_y,
            });
        }
        Ok(RgbaImage {
            dimensions: self.dimensions,
            pixels: self.pixels,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Accumulator {
    red: u128,
    green: u128,
    blue: u128,
    alpha: u128,
    weight: u128,
}

#[derive(Clone, Copy, Debug)]
struct AxisMapping {
    // Coordinates use an exact integer lattice. Cover mappings choose a scale
    // that represents centered fractional crop boundaries without rounding.
    source_len: u32,
    output_len: u32,
    crop_start: u128,
    crop_end: u128,
    source_pixel_span: u128,
    output_pixel_span: u128,
}

impl AxisMapping {
    fn full(source_len: u32, output_len: u32) -> Self {
        let source_pixel_span = u128::from(output_len);
        let output_pixel_span = u128::from(source_len);
        Self {
            source_len,
            output_len,
            crop_start: 0,
            crop_end: u128::from(source_len) * source_pixel_span,
            source_pixel_span,
            output_pixel_span,
        }
    }

    fn cropped(
        source_len: u32,
        output_len: u32,
        crop_start: u128,
        crop_end: u128,
        source_pixel_span: u128,
    ) -> Result<Self> {
        let crop_span = crop_end
            .checked_sub(crop_start)
            .ok_or_else(|| overflow("crop axis span"))?;
        let output_pixel_span = crop_span
            .checked_div(u128::from(output_len))
            .filter(|span| *span != 0 && *span * u128::from(output_len) == crop_span)
            .ok_or_else(|| overflow("crop output pixel span"))?;
        Ok(Self {
            source_len,
            output_len,
            crop_start,
            crop_end,
            source_pixel_span,
            output_pixel_span,
        })
    }

    fn sample(self, index: u32) -> Result<Option<AxisSample>> {
        debug_assert!(index < self.source_len);
        if index >= self.source_len {
            return Ok(None);
        }
        let source_start = u128::from(index)
            .checked_mul(self.source_pixel_span)
            .ok_or_else(|| overflow("crop source interval"))?;
        let source_end = source_start
            .checked_add(self.source_pixel_span)
            .ok_or_else(|| overflow("crop source interval"))?;
        let clipped_start = source_start.max(self.crop_start);
        let clipped_end = source_end.min(self.crop_end);
        if clipped_start >= clipped_end {
            return Ok(None);
        }
        let relative_start = clipped_start - self.crop_start;
        let relative_end = clipped_end - self.crop_start;
        let first_output = relative_start / self.output_pixel_span;
        let last_output = div_ceil_u128(relative_end, self.output_pixel_span)?;
        Ok(Some(AxisSample {
            clipped_start,
            clipped_end,
            first_output: u32::try_from(first_output)
                .map_err(|_| overflow("crop first output conversion"))?,
            last_output: u32::try_from(last_output.min(u128::from(self.output_len)))
                .map_err(|_| overflow("crop last output conversion"))?,
        }))
    }

    fn output_interval(self, index: u32) -> Result<(u128, u128)> {
        let start = u128::from(index)
            .checked_mul(self.output_pixel_span)
            .and_then(|offset| self.crop_start.checked_add(offset))
            .ok_or_else(|| overflow("crop output interval"))?;
        let end = start
            .checked_add(self.output_pixel_span)
            .ok_or_else(|| overflow("crop output interval"))?;
        Ok((start, end))
    }
}

#[derive(Clone, Copy, Debug)]
struct AxisSample {
    clipped_start: u128,
    clipped_end: u128,
    first_output: u32,
    last_output: u32,
}

fn fit_mappings(
    source: Dimensions,
    output: Dimensions,
    fit: Fit,
) -> Result<(AxisMapping, AxisMapping)> {
    if fit == Fit::Contain {
        return Ok((
            AxisMapping::full(source.width, output.width),
            AxisMapping::full(source.height, output.height),
        ));
    }

    let source_aspect = u128::from(source.width) * u128::from(output.height);
    let output_aspect = u128::from(source.height) * u128::from(output.width);
    if source_aspect > output_aspect {
        let scale = u128::from(output.height) * 2;
        let center = u128::from(source.width) * u128::from(output.height);
        let half_crop = u128::from(source.height) * u128::from(output.width);
        Ok((
            AxisMapping::cropped(
                source.width,
                output.width,
                center - half_crop,
                center + half_crop,
                scale,
            )?,
            AxisMapping::full(source.height, output.height),
        ))
    } else if source_aspect < output_aspect {
        let scale = u128::from(output.width) * 2;
        let center = u128::from(source.height) * u128::from(output.width);
        let half_crop = u128::from(source.width) * u128::from(output.height);
        Ok((
            AxisMapping::full(source.width, output.width),
            AxisMapping::cropped(
                source.height,
                output.height,
                center - half_crop,
                center + half_crop,
                scale,
            )?,
        ))
    } else {
        Ok((
            AxisMapping::full(source.width, output.width),
            AxisMapping::full(source.height, output.height),
        ))
    }
}

/// A row-oriented exact area downsampler using premultiplied-alpha accumulation.
///
/// Source rows must be pushed once in ascending order. Memory use depends on
/// source width and output size, but not on source height.
#[derive(Debug)]
pub struct AreaDownsampler<S = RgbaCollector> {
    source: Dimensions,
    output: Dimensions,
    horizontal_mapping: AxisMapping,
    vertical_mapping: AxisMapping,
    next_source_y: u32,
    current_output_y: u32,
    horizontal: Vec<Accumulator>,
    vertical: Vec<Accumulator>,
    output_row: Vec<u8>,
    sink: S,
}

/// An exact area downsampler that accepts source pixels in arbitrary order.
///
/// This representation is intended for sparse interlace passes. Memory use is
/// proportional to the bounded output area and does not depend on source area.
#[derive(Debug)]
pub struct SparseAreaDownsampler {
    source: Dimensions,
    output: Dimensions,
    horizontal_mapping: AxisMapping,
    vertical_mapping: AxisMapping,
    samples_received: u64,
    accumulators: Vec<Accumulator>,
}

impl SparseAreaDownsampler {
    /// Creates a sparse downsampler for fixed source and output dimensions.
    pub fn new(source: Dimensions, output: Dimensions) -> Result<Self> {
        Self::new_with_fit(source, output, Fit::Contain)
    }

    /// Creates a sparse downsampler with the selected fit strategy.
    pub fn new_with_fit(source: Dimensions, output: Dimensions, fit: Fit) -> Result<Self> {
        let output_pixels = usize::try_from(output.pixels()?)
            .map_err(|_| overflow("sparse output pixel count conversion"))?;
        let (horizontal_mapping, vertical_mapping) = fit_mappings(source, output, fit)?;
        Ok(Self {
            source,
            output,
            horizontal_mapping,
            vertical_mapping,
            samples_received: 0,
            accumulators: allocate_accumulators(output_pixels)?,
        })
    }

    /// Adds one normalized straight-alpha RGBA8 source pixel.
    pub fn push_pixel(&mut self, x: u32, y: u32, pixel: [u8; 4]) -> Result<()> {
        if x >= self.source.width || y >= self.source.height {
            return Err(Error::InvalidPixelCoordinate { x, y });
        }

        self.samples_received = self
            .samples_received
            .checked_add(1)
            .ok_or_else(|| overflow("sparse sample count"))?;
        let Some(horizontal) = self.horizontal_mapping.sample(x)? else {
            return Ok(());
        };
        let Some(vertical) = self.vertical_mapping.sample(y)? else {
            return Ok(());
        };
        let output_width = u64::from(self.output.width);
        let red = u128::from(pixel[0]);
        let green = u128::from(pixel[1]);
        let blue = u128::from(pixel[2]);
        let alpha = u128::from(pixel[3]);

        for output_y in vertical.first_output..vertical.last_output {
            let (output_y_start, output_y_end) = self.vertical_mapping.output_interval(output_y)?;
            let y_overlap = interval_overlap(
                vertical.clipped_start,
                vertical.clipped_end,
                output_y_start,
                output_y_end,
            );
            for output_x in horizontal.first_output..horizontal.last_output {
                let (output_x_start, output_x_end) =
                    self.horizontal_mapping.output_interval(output_x)?;
                let x_overlap = interval_overlap(
                    horizontal.clipped_start,
                    horizontal.clipped_end,
                    output_x_start,
                    output_x_end,
                );
                let weight = x_overlap * y_overlap;
                if weight == 0 {
                    continue;
                }
                let index = u64::from(output_y)
                    .checked_mul(output_width)
                    .and_then(|row| row.checked_add(u64::from(output_x)))
                    .ok_or_else(|| overflow("sparse output pixel index"))?;
                let accumulator = &mut self.accumulators[usize::try_from(index)
                    .map_err(|_| overflow("sparse output index conversion"))?];
                accumulator.red += red * alpha * weight;
                accumulator.green += green * alpha * weight;
                accumulator.blue += blue * alpha * weight;
                accumulator.alpha += alpha * weight;
                accumulator.weight += weight;
            }
        }

        Ok(())
    }

    /// Completes the thumbnail after every source pixel has been supplied.
    pub fn finish(self) -> Result<RgbaImage> {
        let sink = RgbaCollector::new(self.output)?;
        self.finish_into(sink)
    }

    /// Emits completed rows after every sparse source sample has arrived.
    pub fn finish_into<S>(self, mut sink: S) -> core::result::Result<S::Output, S::Error>
    where
        S: RgbaRowSink,
    {
        let expected_samples = self.source.pixels()?;
        if self.samples_received != expected_samples {
            return Err(Error::IncompleteSamples {
                expected: expected_samples,
                actual: self.samples_received,
            }
            .into());
        }

        let output_width = usize::try_from(self.output.width)
            .map_err(|_| overflow("sparse output width conversion"))?;
        let output_row_bytes = rgba_row_bytes(self.output.width, "sparse output row")?;
        let mut output_row = allocate_initialized_bytes(output_row_bytes)?;
        let expected_weight =
            self.horizontal_mapping.output_pixel_span * self.vertical_mapping.output_pixel_span;
        for (y, accumulators) in self.accumulators.chunks_exact(output_width).enumerate() {
            let y = u32::try_from(y).map_err(|_| overflow("sparse output y conversion"))?;
            for (x, accumulator) in accumulators.iter().enumerate() {
                if accumulator.weight != expected_weight {
                    return Err(Error::InvalidCoverage {
                        x: u32::try_from(x)
                            .map_err(|_| overflow("sparse coverage x conversion"))?,
                        y,
                        expected: expected_weight,
                        actual: accumulator.weight,
                    }
                    .into());
                }
                write_normalized_pixel(&mut output_row, x, accumulator)?;
            }
            sink.push_row(y, &output_row)?;
        }
        sink.finish()
    }
}

impl AreaDownsampler<RgbaCollector> {
    /// Creates an area downsampler for fixed source and output dimensions.
    pub fn new(source: Dimensions, output: Dimensions) -> Result<Self> {
        Self::new_with_fit(source, output, Fit::Contain)
    }

    /// Creates an area downsampler with the selected fit strategy.
    pub fn new_with_fit(source: Dimensions, output: Dimensions, fit: Fit) -> Result<Self> {
        let sink = RgbaCollector::new(output)?;
        Self::with_sink_and_fit(source, output, fit, sink)
    }
}

impl<S> AreaDownsampler<S>
where
    S: RgbaRowSink,
{
    /// Creates an area downsampler that emits rows to the supplied sink.
    pub fn with_sink(source: Dimensions, output: Dimensions, sink: S) -> Result<Self> {
        Self::with_sink_and_fit(source, output, Fit::Contain, sink)
    }

    /// Creates an area downsampler with a fit strategy and row sink.
    pub fn with_sink_and_fit(
        source: Dimensions,
        output: Dimensions,
        fit: Fit,
        sink: S,
    ) -> Result<Self> {
        let output_width = usize::try_from(output.width).map_err(|_| overflow("output width"))?;
        let output_row_bytes = rgba_row_bytes(output.width, "output row")?;
        let (horizontal_mapping, vertical_mapping) = fit_mappings(source, output, fit)?;

        Ok(Self {
            source,
            output,
            horizontal_mapping,
            vertical_mapping,
            next_source_y: 0,
            current_output_y: 0,
            horizontal: allocate_accumulators(output_width)?,
            vertical: allocate_accumulators(output_width)?,
            output_row: allocate_initialized_bytes(output_row_bytes)?,
            sink,
        })
    }

    /// Adds the next normalized straight-alpha RGBA8 source row.
    pub fn push_row(&mut self, y: u32, pixels: &[u8]) -> core::result::Result<(), S::Error> {
        if y != self.next_source_y {
            return Err(Error::UnexpectedRow {
                expected: self.next_source_y,
                actual: y,
            }
            .into());
        }
        let expected_len = usize::try_from(self.source.width)
            .map_err(|_| overflow("source width"))?
            .checked_mul(4)
            .ok_or_else(|| overflow("source RGBA row length"))?;
        if pixels.len() != expected_len {
            return Err(Error::InvalidRowLength {
                expected: expected_len,
                actual: pixels.len(),
            }
            .into());
        }

        self.reduce_horizontal(pixels)?;
        self.accumulate_vertical(y)?;
        self.next_source_y = self
            .next_source_y
            .checked_add(1)
            .ok_or_else(|| overflow("source row index"))?;
        Ok(())
    }

    /// Completes all output rows and finishes the configured sink.
    pub fn finish(mut self) -> core::result::Result<S::Output, S::Error> {
        if self.next_source_y != self.source.height {
            return Err(Error::IncompleteImage {
                expected_rows: self.source.height,
                actual_rows: self.next_source_y,
            }
            .into());
        }
        while self.current_output_y < self.output.height {
            self.finalize_output_row()?;
        }

        self.sink.finish()
    }

    fn reduce_horizontal(&mut self, pixels: &[u8]) -> Result<()> {
        self.horizontal.fill(Accumulator::default());
        for source_x in 0..self.source.width {
            let Some(sample) = self.horizontal_mapping.sample(source_x)? else {
                continue;
            };
            let pixel_offset = usize::try_from(source_x)
                .map_err(|_| overflow("source pixel index"))?
                .checked_mul(4)
                .ok_or_else(|| overflow("source pixel offset"))?;
            let red = u128::from(pixels[pixel_offset]);
            let green = u128::from(pixels[pixel_offset + 1]);
            let blue = u128::from(pixels[pixel_offset + 2]);
            let alpha = u128::from(pixels[pixel_offset + 3]);

            for output_x in sample.first_output..sample.last_output {
                let (output_start, output_end) =
                    self.horizontal_mapping.output_interval(output_x)?;
                let overlap = interval_overlap(
                    sample.clipped_start,
                    sample.clipped_end,
                    output_start,
                    output_end,
                );
                if overlap == 0 {
                    continue;
                }
                let accumulator = &mut self.horizontal
                    [usize::try_from(output_x).map_err(|_| overflow("output pixel index"))?];
                let weight = overlap;
                accumulator.red += red * alpha * weight;
                accumulator.green += green * alpha * weight;
                accumulator.blue += blue * alpha * weight;
                accumulator.alpha += alpha * weight;
                accumulator.weight += weight;
            }
        }
        Ok(())
    }

    fn accumulate_vertical(&mut self, source_y: u32) -> core::result::Result<(), S::Error> {
        let Some(sample) = self.vertical_mapping.sample(source_y)? else {
            return Ok(());
        };

        for output_y in sample.first_output..sample.last_output {
            while self.current_output_y < output_y {
                self.finalize_output_row()?;
            }

            let (output_start, output_end) = self.vertical_mapping.output_interval(output_y)?;
            let overlap = interval_overlap(
                sample.clipped_start,
                sample.clipped_end,
                output_start,
                output_end,
            );
            for (vertical, horizontal) in self.vertical.iter_mut().zip(&self.horizontal) {
                vertical.red += horizontal.red * overlap;
                vertical.green += horizontal.green * overlap;
                vertical.blue += horizontal.blue * overlap;
                vertical.alpha += horizontal.alpha * overlap;
                vertical.weight += horizontal.weight * overlap;
            }
            if sample.clipped_end >= output_end {
                self.finalize_output_row()?;
            }
        }
        Ok(())
    }

    fn finalize_output_row(&mut self) -> core::result::Result<(), S::Error> {
        let expected_weight =
            self.horizontal_mapping.output_pixel_span * self.vertical_mapping.output_pixel_span;
        for (x, accumulator) in self.vertical.iter().enumerate() {
            if accumulator.weight != expected_weight {
                return Err(Error::InvalidCoverage {
                    x: u32::try_from(x).map_err(|_| overflow("output x conversion"))?,
                    y: self.current_output_y,
                    expected: expected_weight,
                    actual: accumulator.weight,
                }
                .into());
            }
            write_normalized_pixel(&mut self.output_row, x, accumulator)?;
        }
        self.sink
            .push_row(self.current_output_y, &self.output_row)?;
        self.vertical.fill(Accumulator::default());
        self.current_output_y = self
            .current_output_y
            .checked_add(1)
            .ok_or_else(|| overflow("output row index"))?;
        Ok(())
    }
}

fn write_normalized_pixel(
    output_row: &mut [u8],
    x: usize,
    accumulator: &Accumulator,
) -> Result<()> {
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
    let offset = x
        .checked_mul(4)
        .ok_or_else(|| overflow("normalized output pixel offset"))?;
    let end = offset
        .checked_add(4)
        .ok_or_else(|| overflow("normalized output pixel end"))?;
    let pixel = output_row
        .get_mut(offset..end)
        .ok_or_else(|| overflow("normalized output row access"))?;
    pixel.copy_from_slice(&[to_u8(red)?, to_u8(green)?, to_u8(blue)?, to_u8(alpha)?]);
    Ok(())
}

fn rgba_row_bytes(width: u32, operation: &'static str) -> Result<usize> {
    usize::try_from(width)
        .map_err(|_| overflow(operation))?
        .checked_mul(4)
        .ok_or_else(|| overflow(operation))
}

fn rgba_image_bytes(dimensions: Dimensions, operation: &'static str) -> Result<usize> {
    usize::try_from(dimensions.pixels()?)
        .map_err(|_| overflow(operation))?
        .checked_mul(4)
        .ok_or_else(|| overflow(operation))
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

fn allocate_initialized_bytes(len: usize) -> Result<Vec<u8>> {
    let mut bytes = allocate_bytes(len)?;
    bytes.resize(len, 0);
    Ok(bytes)
}

const fn interval_overlap(a_start: u128, a_end: u128, b_start: u128, b_end: u128) -> u128 {
    let start = if a_start > b_start { a_start } else { b_start };
    let end = if a_end < b_end { a_end } else { b_end };
    end.saturating_sub(start)
}

fn div_ceil_u128(numerator: u128, denominator: u128) -> Result<u128> {
    if denominator == 0 {
        return Err(Error::IntegerOverflow {
            operation: "division by zero while mapping crop coverage",
        });
    }
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    quotient
        .checked_add(u128::from(remainder != 0))
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
    use std::{cell::RefCell, rc::Rc};

    use super::*;

    type RecordedRow = (u32, Vec<u8>);
    type SharedRows = Rc<RefCell<Vec<RecordedRow>>>;

    #[derive(Debug)]
    struct RecordingSink {
        row_bytes: usize,
        next_y: u32,
        rows: SharedRows,
    }

    impl RecordingSink {
        fn new(output: Dimensions) -> (Self, SharedRows) {
            let rows = Rc::new(RefCell::new(Vec::new()));
            (
                Self {
                    row_bytes: usize::try_from(output.width).unwrap() * 4,
                    next_y: 0,
                    rows: Rc::clone(&rows),
                },
                rows,
            )
        }
    }

    impl RgbaRowSink for RecordingSink {
        type Output = Vec<(u32, Vec<u8>)>;
        type Error = Error;

        fn push_row(&mut self, y: u32, rgba: &[u8]) -> Result<()> {
            assert_eq!(y, self.next_y);
            assert_eq!(rgba.len(), self.row_bytes);
            self.rows.borrow_mut().push((y, rgba.to_vec()));
            self.next_y += 1;
            Ok(())
        }

        fn finish(self) -> Result<Self::Output> {
            Ok(self.rows.borrow().clone())
        }
    }

    #[derive(Debug, Eq, PartialEq)]
    enum SinkTestError {
        Core(Error),
        Rejected,
    }

    impl From<Error> for SinkTestError {
        fn from(error: Error) -> Self {
            Self::Core(error)
        }
    }

    struct RejectingSink;

    impl RgbaRowSink for RejectingSink {
        type Output = ();
        type Error = SinkTestError;

        fn push_row(&mut self, _y: u32, _rgba: &[u8]) -> core::result::Result<(), Self::Error> {
            Err(SinkTestError::Rejected)
        }

        fn finish(self) -> core::result::Result<(), Self::Error> {
            Ok(())
        }
    }

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

    fn resize_sparse(source: Dimensions, output: Dimensions, pixels: &[u8]) -> RgbaImage {
        let mut downsampler = SparseAreaDownsampler::new(source, output).unwrap();
        for y in (0..source.height).rev() {
            for x in (0..source.width).rev() {
                let offset =
                    usize::try_from((u64::from(y) * u64::from(source.width) + u64::from(x)) * 4)
                        .unwrap();
                downsampler
                    .push_pixel(x, y, pixels[offset..offset + 4].try_into().unwrap())
                    .unwrap();
            }
        }
        downsampler.finish().unwrap()
    }

    fn resize_with_fit(
        source: Dimensions,
        output: Dimensions,
        fit: Fit,
        pixels: &[u8],
    ) -> RgbaImage {
        let mut downsampler = AreaDownsampler::new_with_fit(source, output, fit).unwrap();
        let row_len = usize::try_from(source.width).unwrap() * 4;
        for (y, row) in pixels.chunks_exact(row_len).enumerate() {
            downsampler
                .push_row(u32::try_from(y).unwrap(), row)
                .unwrap();
        }
        downsampler.finish().unwrap()
    }

    fn resize_sparse_with_fit(
        source: Dimensions,
        output: Dimensions,
        fit: Fit,
        pixels: &[u8],
    ) -> RgbaImage {
        let mut downsampler = SparseAreaDownsampler::new_with_fit(source, output, fit).unwrap();
        for y in (0..source.height).rev() {
            for x in (0..source.width).rev() {
                let offset =
                    usize::try_from((u64::from(y) * u64::from(source.width) + u64::from(x)) * 4)
                        .unwrap();
                downsampler
                    .push_pixel(x, y, pixels[offset..offset + 4].try_into().unwrap())
                    .unwrap();
            }
        }
        downsampler.finish().unwrap()
    }

    fn recorded_pixels(rows: &[(u32, Vec<u8>)]) -> Vec<u8> {
        rows.iter()
            .flat_map(|(_, row)| row.iter().copied())
            .collect()
    }

    #[test]
    fn ordered_downsampler_emits_completed_rows_once_in_order() {
        let source = Dimensions::new(4, 4).unwrap();
        let output = Dimensions::new(2, 2).unwrap();
        let pixels = (0..source.pixels().unwrap() * 4)
            .map(|value| u8::try_from(value % 251).unwrap())
            .collect::<Vec<_>>();
        let expected = resize(source, output, &pixels);
        let (sink, observed) = RecordingSink::new(output);
        let mut downsampler = AreaDownsampler::with_sink(source, output, sink).unwrap();
        let row_bytes = usize::try_from(source.width).unwrap() * 4;

        for (y, row) in pixels.chunks_exact(row_bytes).enumerate() {
            downsampler
                .push_row(u32::try_from(y).unwrap(), row)
                .unwrap();
            if y == 1 {
                assert_eq!(observed.borrow().len(), 1);
            }
        }

        let rows = downsampler.finish().unwrap();
        assert_eq!(rows.iter().map(|(y, _)| *y).collect::<Vec<_>>(), [0, 1]);
        assert!(rows.iter().all(|(_, row)| row.len() == 2 * 4));
        assert_eq!(recorded_pixels(&rows), expected.pixels);
    }

    #[test]
    fn sparse_downsampler_finishes_into_rows_equivalent_to_collected_output() {
        let source = Dimensions::new(3, 3).unwrap();
        let output = Dimensions::new(2, 2).unwrap();
        let pixels = (0..source.pixels().unwrap() * 4)
            .map(|value| u8::try_from((value * 17) % 256).unwrap())
            .collect::<Vec<_>>();
        let expected = resize_sparse(source, output, &pixels);
        let mut downsampler = SparseAreaDownsampler::new(source, output).unwrap();
        for y in (0..source.height).rev() {
            for x in (0..source.width).rev() {
                let offset =
                    usize::try_from((u64::from(y) * u64::from(source.width) + u64::from(x)) * 4)
                        .unwrap();
                downsampler
                    .push_pixel(x, y, pixels[offset..offset + 4].try_into().unwrap())
                    .unwrap();
            }
        }
        let (sink, observed) = RecordingSink::new(output);
        assert!(observed.borrow().is_empty());
        let rows = downsampler.finish_into(sink).unwrap();

        assert_eq!(rows.iter().map(|(y, _)| *y).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(recorded_pixels(&rows), expected.pixels);
    }

    #[test]
    fn rgba_collector_validates_row_contract() {
        let dimensions = Dimensions::new(2, 2).unwrap();
        let mut collector = RgbaCollector::new(dimensions).unwrap();
        assert_eq!(
            collector.push_row(1, &[0; 8]),
            Err(Error::UnexpectedRow {
                expected: 0,
                actual: 1
            })
        );
        assert_eq!(
            collector.push_row(0, &[0; 7]),
            Err(Error::InvalidRowLength {
                expected: 8,
                actual: 7
            })
        );
        collector.push_row(0, &[1; 8]).unwrap();
        assert_eq!(
            collector.finish(),
            Err(Error::IncompleteImage {
                expected_rows: 2,
                actual_rows: 1
            })
        );
    }

    #[test]
    fn ordered_downsampler_preserves_sink_errors() {
        let dimensions = Dimensions::new(1, 1).unwrap();
        let mut downsampler =
            AreaDownsampler::with_sink(dimensions, dimensions, RejectingSink).unwrap();
        assert_eq!(
            downsampler.push_row(0, &[1, 2, 3, 4]),
            Err(SinkTestError::Rejected)
        );
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
    fn cover_crops_equal_horizontal_margins() {
        let source = Dimensions::new(4, 2).unwrap();
        let output = Dimensions::new(2, 2).unwrap();
        let row = [10, 0, 0, 255, 20, 0, 0, 255, 30, 0, 0, 255, 40, 0, 0, 255];
        let pixels = [row, row].concat();
        let image = resize_with_fit(source, output, Fit::Cover, &pixels);
        assert_eq!(image.pixels, [20, 0, 0, 255, 30, 0, 0, 255].repeat(2));
    }

    #[test]
    fn cover_crops_equal_vertical_margins() {
        let source = Dimensions::new(2, 4).unwrap();
        let output = Dimensions::new(2, 2).unwrap();
        let mut pixels = Vec::new();
        for value in [10, 20, 30, 40] {
            pixels.extend_from_slice(&[value, 0, 0, 255].repeat(2));
        }
        let image = resize_with_fit(source, output, Fit::Cover, &pixels);
        assert_eq!(
            image.pixels,
            [20, 0, 0, 255, 20, 0, 0, 255, 30, 0, 0, 255, 30, 0, 0, 255]
        );
    }

    #[test]
    fn cover_preserves_fractional_center_boundaries() {
        let source = Dimensions::new(5, 2).unwrap();
        let output = Dimensions::new(2, 2).unwrap();
        let row = [
            0, 0, 0, 255, 40, 0, 0, 255, 80, 0, 0, 255, 120, 0, 0, 255, 160, 0, 0, 255,
        ];
        let pixels = [row, row].concat();
        let ordered = resize_with_fit(source, output, Fit::Cover, &pixels);
        let sparse = resize_sparse_with_fit(source, output, Fit::Cover, &pixels);
        assert_eq!(ordered.pixels, [60, 0, 0, 255, 100, 0, 0, 255].repeat(2));
        assert_eq!(sparse, ordered);
    }

    #[test]
    fn cover_matches_an_independent_fractional_reference() {
        let source = Dimensions::new(7, 5).unwrap();
        let pixels = (0..source.pixels().unwrap())
            .flat_map(|index| {
                let value = u8::try_from(index).unwrap();
                [
                    value.wrapping_mul(17),
                    value.wrapping_mul(29),
                    value.wrapping_mul(43),
                    value.wrapping_mul(7),
                ]
            })
            .collect::<Vec<_>>();

        for output in [
            Dimensions::new(3, 3).unwrap(),
            Dimensions::new(4, 2).unwrap(),
            Dimensions::new(2, 4).unwrap(),
            Dimensions::new(9, 6).unwrap(),
        ] {
            let actual = resize_with_fit(source, output, Fit::Cover, &pixels);
            let expected = reference_cover_resize(source, output, &pixels);
            for (index, (actual, expected)) in actual.pixels.iter().zip(expected).enumerate() {
                assert!(
                    actual.abs_diff(expected) <= 1,
                    "byte {index} differs for {output:?}: actual {actual}, expected {expected}"
                );
            }
        }
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

    #[test]
    fn sparse_accumulation_matches_ordered_rows_for_arbitrary_ratios() {
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
            assert_eq!(
                resize_sparse(source, output, &pixels),
                resize(source, output, &pixels)
            );
        }
    }

    #[test]
    fn sparse_accumulation_rejects_invalid_coordinates_and_missing_samples() {
        let source = Dimensions::new(2, 1).unwrap();
        let output = Dimensions::new(1, 1).unwrap();
        let mut downsampler = SparseAreaDownsampler::new(source, output).unwrap();
        assert_eq!(
            downsampler.push_pixel(2, 0, [0; 4]),
            Err(Error::InvalidPixelCoordinate { x: 2, y: 0 })
        );
        downsampler.push_pixel(0, 0, [1, 2, 3, 4]).unwrap();
        assert_eq!(
            downsampler.finish(),
            Err(Error::IncompleteSamples {
                expected: 2,
                actual: 1
            })
        );
    }

    fn reference_area_resize(source: Dimensions, output: Dimensions, pixels: &[u8]) -> Vec<u8> {
        reference_area_resize_region(
            source,
            output,
            pixels,
            0.0,
            0.0,
            f64::from(source.width),
            f64::from(source.height),
        )
    }

    fn reference_cover_resize(source: Dimensions, output: Dimensions, pixels: &[u8]) -> Vec<u8> {
        let source_aspect = f64::from(source.width) / f64::from(source.height);
        let output_aspect = f64::from(output.width) / f64::from(output.height);
        let (crop_width, crop_height) = if source_aspect > output_aspect {
            (
                f64::from(source.height) * output_aspect,
                f64::from(source.height),
            )
        } else {
            (
                f64::from(source.width),
                f64::from(source.width) / output_aspect,
            )
        };
        reference_area_resize_region(
            source,
            output,
            pixels,
            (f64::from(source.width) - crop_width) / 2.0,
            (f64::from(source.height) - crop_height) / 2.0,
            crop_width,
            crop_height,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn reference_area_resize_region(
        source: Dimensions,
        output: Dimensions,
        pixels: &[u8],
        crop_left: f64,
        crop_top: f64,
        crop_width: f64,
        crop_height: f64,
    ) -> Vec<u8> {
        let mut result = Vec::new();
        for output_y in 0..output.height {
            let top = crop_top + f64::from(output_y) * crop_height / f64::from(output.height);
            let bottom =
                crop_top + f64::from(output_y + 1) * crop_height / f64::from(output.height);
            for output_x in 0..output.width {
                let left = crop_left + f64::from(output_x) * crop_width / f64::from(output.width);
                let right =
                    crop_left + f64::from(output_x + 1) * crop_width / f64::from(output.width);
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
