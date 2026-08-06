use crate::{Error, Result};

/// A non-zero image size in pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Dimensions {
    pub width: u32,
    pub height: u32,
}

impl Dimensions {
    /// Creates dimensions after rejecting zero width or height.
    pub fn new(width: u32, height: u32) -> Result<Self> {
        if width == 0 {
            return Err(Error::ZeroDimension { field: "width" });
        }
        if height == 0 {
            return Err(Error::ZeroDimension { field: "height" });
        }
        Ok(Self { width, height })
    }

    /// Returns the number of pixels using checked arithmetic.
    pub fn pixels(self) -> Result<u64> {
        u64::from(self.width)
            .checked_mul(u64::from(self.height))
            .ok_or(Error::IntegerOverflow {
                operation: "pixel count",
            })
    }
}

/// Calculates aspect-preserving dimensions that fit within the requested box.
///
/// Integer division rounds the unconstrained axis down so the result never
/// exceeds either bound. A non-zero source always produces non-zero output.
pub fn contain_dimensions(
    source: Dimensions,
    bounds: Dimensions,
    allow_upscale: bool,
) -> Result<Dimensions> {
    let bound_width = if allow_upscale {
        bounds.width
    } else {
        bounds.width.min(source.width)
    };
    let bound_height = if allow_upscale {
        bounds.height
    } else {
        bounds.height.min(source.height)
    };

    let width_limited = u64::from(bound_width)
        .checked_mul(u64::from(source.height))
        .ok_or(Error::IntegerOverflow {
            operation: "contain-fit aspect comparison",
        })?
        <= u64::from(bound_height)
            .checked_mul(u64::from(source.width))
            .ok_or(Error::IntegerOverflow {
                operation: "contain-fit aspect comparison",
            })?;

    if width_limited {
        let height = u64::from(source.height)
            .checked_mul(u64::from(bound_width))
            .ok_or(Error::IntegerOverflow {
                operation: "contain-fit height",
            })?
            / u64::from(source.width);
        Dimensions::new(
            bound_width,
            u32::try_from(height.max(1)).map_err(|_| Error::IntegerOverflow {
                operation: "contain-fit height conversion",
            })?,
        )
    } else {
        let width = u64::from(source.width)
            .checked_mul(u64::from(bound_height))
            .ok_or(Error::IntegerOverflow {
                operation: "contain-fit width",
            })?
            / u64::from(source.height);
        Dimensions::new(
            u32::try_from(width.max(1)).map_err(|_| Error::IntegerOverflow {
                operation: "contain-fit width conversion",
            })?,
            bound_height,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dimensions(width: u32, height: u32) -> Dimensions {
        Dimensions::new(width, height).unwrap()
    }

    #[test]
    fn rejects_zero_dimensions() {
        assert_eq!(
            Dimensions::new(0, 1),
            Err(Error::ZeroDimension { field: "width" })
        );
        assert_eq!(
            Dimensions::new(1, 0),
            Err(Error::ZeroDimension { field: "height" })
        );
    }

    #[test]
    fn contains_landscape_portrait_and_square_sources() {
        let bounds = dimensions(100, 100);
        assert_eq!(
            contain_dimensions(dimensions(400, 200), bounds, false).unwrap(),
            dimensions(100, 50)
        );
        assert_eq!(
            contain_dimensions(dimensions(200, 400), bounds, false).unwrap(),
            dimensions(50, 100)
        );
        assert_eq!(
            contain_dimensions(dimensions(400, 400), bounds, false).unwrap(),
            bounds
        );
    }

    #[test]
    fn supports_non_square_bounds() {
        assert_eq!(
            contain_dimensions(dimensions(1600, 900), dimensions(320, 100), false).unwrap(),
            dimensions(177, 100)
        );
        assert_eq!(
            contain_dimensions(dimensions(900, 1600), dimensions(100, 320), false).unwrap(),
            dimensions(100, 177)
        );
    }

    #[test]
    fn no_upscale_preserves_a_smaller_source() {
        assert_eq!(
            contain_dimensions(dimensions(32, 16), dimensions(512, 512), false).unwrap(),
            dimensions(32, 16)
        );
    }

    #[test]
    fn upscale_uses_the_largest_fitting_size() {
        assert_eq!(
            contain_dimensions(dimensions(32, 16), dimensions(512, 512), true).unwrap(),
            dimensions(512, 256)
        );
    }

    #[test]
    fn output_axes_never_round_to_zero() {
        assert_eq!(
            contain_dimensions(dimensions(100_000, 1), dimensions(1, 1), false).unwrap(),
            dimensions(1, 1)
        );
        assert_eq!(
            contain_dimensions(dimensions(1, 100_000), dimensions(1, 1), false).unwrap(),
            dimensions(1, 1)
        );
    }

    #[test]
    fn handles_maximum_u32_dimensions() {
        let maximum = dimensions(u32::MAX, u32::MAX);
        assert_eq!(
            contain_dimensions(maximum, dimensions(1, 1), false).unwrap(),
            dimensions(1, 1)
        );
        assert_eq!(maximum.pixels().unwrap(), u64::from(u32::MAX).pow(2));
    }
}
