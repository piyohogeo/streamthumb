use core::fmt;

/// The result type returned by PNG adapter operations.
pub type Result<T> = core::result::Result<T, Error>;

/// A PNG feature that is intentionally outside the current implementation phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UnsupportedFeature {
    Animation,
    Interlacing,
    ColorType,
    BitDepth,
}

/// An error produced while reading PNG metadata or decoded rows.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    Core(streamthumb_core::Error),
    Unsupported {
        feature: UnsupportedFeature,
        detail: &'static str,
    },
    TruncatedInput,
    DecoderMemoryLimitExceeded {
        limit: usize,
    },
    AllocationFailed {
        bytes: usize,
    },
    DecodeFailure(String),
    EncodeFailure(String),
    EncodedOutputLimitExceeded {
        limit: usize,
    },
    RowConsumer(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core(error) => error.fmt(formatter),
            Self::Unsupported { feature, detail } => {
                write!(formatter, "unsupported PNG {feature:?}: {detail}")
            }
            Self::TruncatedInput => formatter.write_str("truncated PNG input"),
            Self::DecoderMemoryLimitExceeded { limit } => write!(
                formatter,
                "PNG decoder exceeded its {limit}-byte memory allowance"
            ),
            Self::AllocationFailed { bytes } => {
                write!(formatter, "failed to allocate a {bytes}-byte buffer")
            }
            Self::DecodeFailure(message) => write!(formatter, "PNG decode failure: {message}"),
            Self::EncodeFailure(message) => write!(formatter, "PNG encode failure: {message}"),
            Self::EncodedOutputLimitExceeded { limit } => {
                write!(formatter, "encoded PNG exceeded its {limit}-byte limit")
            }
            Self::RowConsumer(message) => write!(formatter, "row consumer failed: {message}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            _ => None,
        }
    }
}

impl From<streamthumb_core::Error> for Error {
    fn from(error: streamthumb_core::Error) -> Self {
        Self::Core(error)
    }
}
