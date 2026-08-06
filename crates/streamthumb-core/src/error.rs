use core::fmt;

/// The result type returned by streamthumb operations.
pub type Result<T> = core::result::Result<T, Error>;

/// Identifies the configured resource limit that was exceeded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LimitKind {
    InputBytes,
    InputWidth,
    InputHeight,
    InputPixels,
    OutputWidth,
    OutputHeight,
    OutputPixels,
    WorkingMemory,
}

/// An error produced while validating or planning a thumbnail operation.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Error {
    /// An input or configured output dimension was zero.
    ZeroDimension { field: &'static str },
    /// A configured resource limit was zero and therefore unusable.
    InvalidLimit { field: &'static str },
    /// The decoded source pixel size was zero.
    InvalidSourceBytesPerPixel,
    /// A configured resource limit was exceeded.
    LimitExceeded {
        kind: LimitKind,
        actual: u64,
        limit: u64,
    },
    /// Checked arithmetic failed while calculating resource requirements.
    IntegerOverflow { operation: &'static str },
    /// A source row arrived out of order.
    UnexpectedRow { expected: u32, actual: u32 },
    /// A normalized source row had the wrong byte length.
    InvalidRowLength { expected: usize, actual: usize },
    /// A sparse source sample was outside the declared source dimensions.
    InvalidPixelCoordinate { x: u32, y: u32 },
    /// Fewer source rows were provided than the declared source height.
    IncompleteImage {
        expected_rows: u32,
        actual_rows: u32,
    },
    /// Fewer sparse source samples were provided than the declared source area.
    IncompleteSamples { expected: u64, actual: u64 },
    /// A bounded internal allocation could not be satisfied.
    AllocationFailed { bytes: usize },
    /// The exact area weights did not cover an output pixel as expected.
    InvalidCoverage {
        x: u32,
        y: u32,
        expected: u128,
        actual: u128,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroDimension { field } => write!(formatter, "{field} must be greater than zero"),
            Self::InvalidLimit { field } => {
                write!(
                    formatter,
                    "configured limit {field} must be greater than zero"
                )
            }
            Self::InvalidSourceBytesPerPixel => {
                formatter.write_str("source bytes per pixel must be greater than zero")
            }
            Self::LimitExceeded {
                kind,
                actual,
                limit,
            } => write!(
                formatter,
                "{kind:?} limit exceeded: actual {actual}, limit {limit}"
            ),
            Self::IntegerOverflow { operation } => {
                write!(formatter, "integer overflow while calculating {operation}")
            }
            Self::UnexpectedRow { expected, actual } => {
                write!(
                    formatter,
                    "expected source row {expected}, received {actual}"
                )
            }
            Self::InvalidRowLength { expected, actual } => write!(
                formatter,
                "invalid RGBA row length: expected {expected} bytes, received {actual}"
            ),
            Self::InvalidPixelCoordinate { x, y } => {
                write!(
                    formatter,
                    "source pixel coordinate ({x}, {y}) is out of bounds"
                )
            }
            Self::IncompleteImage {
                expected_rows,
                actual_rows,
            } => write!(
                formatter,
                "incomplete image: expected {expected_rows} rows, received {actual_rows}"
            ),
            Self::IncompleteSamples { expected, actual } => write!(
                formatter,
                "incomplete sparse image: expected {expected} samples, received {actual}"
            ),
            Self::AllocationFailed { bytes } => {
                write!(formatter, "failed to allocate a {bytes}-byte buffer")
            }
            Self::InvalidCoverage {
                x,
                y,
                expected,
                actual,
            } => write!(
                formatter,
                "invalid area coverage at ({x}, {y}): expected {expected}, received {actual}"
            ),
        }
    }
}

impl std::error::Error for Error {}
