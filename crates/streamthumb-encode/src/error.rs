use core::fmt;

use streamthumb_core::OutputFormat;

/// The result type returned by output encoder operations.
pub type Result<T> = core::result::Result<T, Error>;

/// An error produced while configuring or running an output encoder.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    Core(streamthumb_core::Error),
    InvalidJpegOptions(&'static str),
    AllocationFailed {
        bytes: usize,
    },
    EncodedOutputLimitExceeded {
        format: OutputFormat,
        limit: usize,
    },
    EncodeFailure {
        format: OutputFormat,
        message: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core(error) => error.fmt(formatter),
            Self::InvalidJpegOptions(message) => {
                write!(formatter, "invalid JPEG encoder options: {message}")
            }
            Self::AllocationFailed { bytes } => {
                write!(formatter, "failed to allocate a {bytes}-byte buffer")
            }
            Self::EncodedOutputLimitExceeded { format, limit } => write!(
                formatter,
                "encoded {} exceeded its {limit}-byte limit",
                format_name(*format)
            ),
            Self::EncodeFailure { format, message } => {
                write!(
                    formatter,
                    "{} encode failure: {message}",
                    format_name(*format)
                )
            }
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

const fn format_name(format: OutputFormat) -> &'static str {
    match format {
        OutputFormat::Png => "PNG",
        OutputFormat::Jpeg => "JPEG",
        OutputFormat::Rgba => "RGBA",
    }
}
