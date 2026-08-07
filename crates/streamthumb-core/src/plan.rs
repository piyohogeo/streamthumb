use crate::memory::{
    estimate_encoded_output_limit_bytes, estimate_sparse_writer_working_memory_for_output,
    estimate_writer_working_memory_for_output,
};
use crate::{
    Dimensions, Error, Fit, LimitKind, MemoryEstimate, Result, ThumbnailOptions,
    contain_dimensions, cover_dimensions, estimate_sparse_working_memory_for_output,
    estimate_working_memory_for_output,
};

/// Header-level information needed to plan processing before large allocations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputInfo {
    pub dimensions: Dimensions,
    pub encoded_bytes: u64,
    pub source_bytes_per_pixel: u8,
}

/// Metadata describing a completed thumbnail.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThumbnailInfo {
    pub width: u32,
    pub height: u32,
    pub format: crate::OutputFormat,
}

/// The validated geometry and memory budget for a thumbnail operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessingPlan {
    pub source: Dimensions,
    pub output: Dimensions,
    pub memory: MemoryEstimate,
    /// Maximum encoded bytes accepted by either buffered or direct output.
    pub encoded_output_limit_bytes: usize,
}

/// Validates limits and creates a processing plan before decoding begins.
pub fn plan_thumbnail(input: InputInfo, options: &ThumbnailOptions) -> Result<ProcessingPlan> {
    plan_thumbnail_with_layout(input, options, false, true)
}

/// Creates a processing plan for arbitrary-order sparse source samples.
pub fn plan_thumbnail_sparse(
    input: InputInfo,
    options: &ThumbnailOptions,
) -> Result<ProcessingPlan> {
    plan_thumbnail_with_layout(input, options, true, true)
}

/// Creates a plan whose encoded result is forwarded to a caller-owned writer.
pub fn plan_thumbnail_to_writer(
    input: InputInfo,
    options: &ThumbnailOptions,
) -> Result<ProcessingPlan> {
    plan_thumbnail_with_layout(input, options, false, false)
}

/// Creates a sparse-input plan whose encoded result is forwarded to a writer.
pub fn plan_thumbnail_sparse_to_writer(
    input: InputInfo,
    options: &ThumbnailOptions,
) -> Result<ProcessingPlan> {
    plan_thumbnail_with_layout(input, options, true, false)
}

fn plan_thumbnail_with_layout(
    input: InputInfo,
    options: &ThumbnailOptions,
    sparse: bool,
    retain_encoded_output: bool,
) -> Result<ProcessingPlan> {
    validate_non_zero_limits(options)?;
    validate_input(input, options)?;

    let requested_bounds = Dimensions::new(options.max_width, options.max_height)?;
    let output = match options.fit {
        Fit::Contain => {
            contain_dimensions(input.dimensions, requested_bounds, options.allow_upscale)?
        }
        Fit::Cover => cover_dimensions(input.dimensions, requested_bounds, options.allow_upscale)?,
    };
    validate_output(output, options)?;

    let memory = match (sparse, retain_encoded_output) {
        (true, true) => estimate_sparse_working_memory_for_output(
            input.dimensions,
            output,
            input.source_bytes_per_pixel,
            options.output,
        )?,
        (false, true) => estimate_working_memory_for_output(
            input.dimensions,
            output,
            input.source_bytes_per_pixel,
            options.output,
        )?,
        (true, false) => estimate_sparse_writer_working_memory_for_output(
            input.dimensions,
            output,
            input.source_bytes_per_pixel,
            options.output,
        )?,
        (false, false) => estimate_writer_working_memory_for_output(
            input.dimensions,
            output,
            input.source_bytes_per_pixel,
            options.output,
        )?,
    };
    let encoded_output_limit_bytes = estimate_encoded_output_limit_bytes(output, options.output)?;
    enforce(
        LimitKind::WorkingMemory,
        usize_to_u64(memory.total_bytes)?,
        usize_to_u64(options.limits.max_working_memory_bytes)?,
    )?;

    Ok(ProcessingPlan {
        source: input.dimensions,
        output,
        memory,
        encoded_output_limit_bytes,
    })
}

fn validate_non_zero_limits(options: &ThumbnailOptions) -> Result<()> {
    let limits = &options.limits;
    for (field, value) in [
        ("limits.max_input_bytes", limits.max_input_bytes),
        ("limits.max_width", u64::from(limits.max_width)),
        ("limits.max_height", u64::from(limits.max_height)),
        ("limits.max_pixels", limits.max_pixels),
        (
            "limits.max_output_width",
            u64::from(limits.max_output_width),
        ),
        (
            "limits.max_output_height",
            u64::from(limits.max_output_height),
        ),
        ("limits.max_output_pixels", limits.max_output_pixels),
    ] {
        if value == 0 {
            return Err(Error::InvalidLimit { field });
        }
    }
    if limits.max_working_memory_bytes == 0 {
        return Err(Error::InvalidLimit {
            field: "limits.max_working_memory_bytes",
        });
    }
    Ok(())
}

fn validate_input(input: InputInfo, options: &ThumbnailOptions) -> Result<()> {
    let limits = &options.limits;
    enforce(
        LimitKind::InputBytes,
        input.encoded_bytes,
        limits.max_input_bytes,
    )?;
    enforce(
        LimitKind::InputWidth,
        u64::from(input.dimensions.width),
        u64::from(limits.max_width),
    )?;
    enforce(
        LimitKind::InputHeight,
        u64::from(input.dimensions.height),
        u64::from(limits.max_height),
    )?;
    enforce(
        LimitKind::InputPixels,
        input.dimensions.pixels()?,
        limits.max_pixels,
    )
}

fn validate_output(output: Dimensions, options: &ThumbnailOptions) -> Result<()> {
    let limits = &options.limits;
    enforce(
        LimitKind::OutputWidth,
        u64::from(output.width),
        u64::from(limits.max_output_width),
    )?;
    enforce(
        LimitKind::OutputHeight,
        u64::from(output.height),
        u64::from(limits.max_output_height),
    )?;
    enforce(
        LimitKind::OutputPixels,
        output.pixels()?,
        limits.max_output_pixels,
    )
}

fn enforce(kind: LimitKind, actual: u64, limit: u64) -> Result<()> {
    if actual > limit {
        Err(Error::LimitExceeded {
            kind,
            actual,
            limit,
        })
    } else {
        Ok(())
    }
}

fn usize_to_u64(value: usize) -> Result<u64> {
    u64::try_from(value).map_err(|_| Error::IntegerOverflow {
        operation: "memory size conversion",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Limits;

    fn input(width: u32, height: u32) -> InputInfo {
        InputInfo {
            dimensions: Dimensions::new(width, height).unwrap(),
            encoded_bytes: 1_024,
            source_bytes_per_pixel: 4,
        }
    }

    fn assert_limit(error: Error, expected: LimitKind) {
        assert!(
            matches!(error, Error::LimitExceeded { kind, .. } if kind == expected),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn creates_a_complete_plan() {
        let plan = plan_thumbnail(input(4_000, 2_000), &ThumbnailOptions::default()).unwrap();
        assert_eq!(plan.source, Dimensions::new(4_000, 2_000).unwrap());
        assert_eq!(plan.output, Dimensions::new(512, 256).unwrap());
        assert!(plan.memory.total_bytes > plan.memory.output_rgba_bytes);
    }

    #[test]
    fn accepts_values_equal_to_each_limit() {
        let dimensions = Dimensions::new(10, 10).unwrap();
        let required_memory = crate::estimate_working_memory_for_output(
            dimensions,
            dimensions,
            4,
            crate::OutputFormat::Png,
        )
        .unwrap();
        let mut options = ThumbnailOptions {
            max_width: 10,
            max_height: 10,
            ..ThumbnailOptions::default()
        };
        options.limits = Limits {
            max_input_bytes: 100,
            max_width: 10,
            max_height: 10,
            max_pixels: 100,
            max_output_width: 10,
            max_output_height: 10,
            max_output_pixels: 100,
            max_working_memory_bytes: required_memory.total_bytes,
        };
        let value = InputInfo {
            dimensions,
            encoded_bytes: 100,
            source_bytes_per_pixel: 4,
        };
        assert!(plan_thumbnail(value, &options).is_ok());
    }

    #[test]
    fn rejects_each_input_limit() {
        let cases = [
            (
                InputInfo {
                    encoded_bytes: Limits::default().max_input_bytes + 1,
                    ..input(1, 1)
                },
                LimitKind::InputBytes,
            ),
            (
                input(Limits::default().max_width + 1, 1),
                LimitKind::InputWidth,
            ),
            (
                input(1, Limits::default().max_height + 1),
                LimitKind::InputHeight,
            ),
            (input(50_000, 20_000), LimitKind::InputPixels),
        ];

        for (value, expected) in cases {
            assert_limit(
                plan_thumbnail(value, &ThumbnailOptions::default()).unwrap_err(),
                expected,
            );
        }
    }

    #[test]
    fn rejects_each_output_limit() {
        let mut options = ThumbnailOptions {
            max_width: 100,
            max_height: 100,
            allow_upscale: true,
            ..ThumbnailOptions::default()
        };

        options.limits.max_output_width = 99;
        assert_limit(
            plan_thumbnail(input(1, 1), &options).unwrap_err(),
            LimitKind::OutputWidth,
        );

        options.limits.max_output_width = 100;
        options.limits.max_output_height = 99;
        assert_limit(
            plan_thumbnail(input(1, 1), &options).unwrap_err(),
            LimitKind::OutputHeight,
        );

        options.limits.max_output_height = 100;
        options.limits.max_output_pixels = 9_999;
        assert_limit(
            plan_thumbnail(input(1, 1), &options).unwrap_err(),
            LimitKind::OutputPixels,
        );
    }

    #[test]
    fn rejects_a_plan_over_the_memory_budget() {
        let mut options = ThumbnailOptions::default();
        options.limits.max_working_memory_bytes = 1;
        assert_limit(
            plan_thumbnail(input(10, 10), &options).unwrap_err(),
            LimitKind::WorkingMemory,
        );
    }

    #[test]
    fn rejects_zero_requested_bounds() {
        let options = ThumbnailOptions {
            max_width: 0,
            ..ThumbnailOptions::default()
        };
        assert_eq!(
            plan_thumbnail(input(1, 1), &options),
            Err(Error::ZeroDimension { field: "width" })
        );
    }

    #[test]
    fn rejects_zero_configured_limits() {
        let mut options = ThumbnailOptions::default();
        options.limits.max_pixels = 0;
        assert_eq!(
            plan_thumbnail(input(1, 1), &options),
            Err(Error::InvalidLimit {
                field: "limits.max_pixels"
            })
        );
    }

    #[test]
    fn plans_cover_output_without_changing_the_source_allocation_shape() {
        let options = ThumbnailOptions {
            max_width: 320,
            max_height: 180,
            fit: Fit::Cover,
            ..ThumbnailOptions::default()
        };
        let plan = plan_thumbnail(input(1_000, 1_000), &options).unwrap();
        assert_eq!(plan.source, Dimensions::new(1_000, 1_000).unwrap());
        assert_eq!(plan.output, Dimensions::new(320, 180).unwrap());
    }

    #[test]
    fn writer_plan_preserves_the_encoded_limit_without_reserving_the_result() {
        let options = ThumbnailOptions {
            max_width: 512,
            max_height: 512,
            output: crate::OutputFormat::Png,
            ..ThumbnailOptions::default()
        };
        let buffered = plan_thumbnail(input(512, 512), &options).unwrap();
        let writer = plan_thumbnail_to_writer(input(512, 512), &options).unwrap();

        assert_eq!(
            writer.encoded_output_limit_bytes,
            buffered.encoded_output_limit_bytes
        );
        assert_eq!(writer.memory.encoded_output_bytes, 0);
        assert_eq!(
            writer.memory.total_bytes + buffered.memory.encoded_output_bytes,
            buffered.memory.total_bytes
        );
    }

    #[test]
    fn writer_plan_can_fit_a_budget_that_rejects_buffered_output() {
        let mut options = ThumbnailOptions {
            max_width: 512,
            max_height: 512,
            output: crate::OutputFormat::Jpeg,
            ..ThumbnailOptions::default()
        };
        let writer = plan_thumbnail_to_writer(input(512, 512), &options).unwrap();
        options.limits.max_working_memory_bytes = writer.memory.total_bytes;

        assert!(plan_thumbnail_to_writer(input(512, 512), &options).is_ok());
        assert_limit(
            plan_thumbnail(input(512, 512), &options).unwrap_err(),
            LimitKind::WorkingMemory,
        );
    }
}
