//! Platform-independent planning primitives for `streamthumb`.
//!
//! This crate validates resource limits, calculates contain-fit output
//! dimensions, estimates working memory, and provides the codec-independent
//! streaming area downsampler.

mod error;
mod geometry;
mod limits;
mod memory;
mod options;
mod plan;
mod resample;

pub use error::{Error, LimitKind, Result};
pub use geometry::{Dimensions, contain_dimensions};
pub use limits::Limits;
pub use memory::{MemoryEstimate, estimate_working_memory, estimate_working_memory_for_output};
pub use options::{Filter, Fit, OutputFormat, ThumbnailOptions};
pub use plan::{InputInfo, ProcessingPlan, ThumbnailInfo, plan_thumbnail};
pub use resample::{AreaDownsampler, RgbaImage};
