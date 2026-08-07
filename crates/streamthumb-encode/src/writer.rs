use std::{
    cell::RefCell,
    io::{self, Write},
    rc::Rc,
};

use streamthumb_core::OutputFormat;

use crate::{Error, Result};

/// Internal target abstraction shared by buffered and direct output encoders.
#[doc(hidden)]
pub trait OutputTarget: Write {
    type Finished;

    fn prepare_write(&mut self, additional: usize, required: usize) -> Result<()>;
    fn finish(self) -> io::Result<Self::Finished>;
}

/// A fallibly growing in-memory output target.
#[doc(hidden)]
pub struct BufferedOutput {
    bytes: Vec<u8>,
    limit: usize,
}

impl BufferedOutput {
    pub fn new(limit: usize) -> Result<Self> {
        let initial_capacity = limit.min(64 * 1024);
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(initial_capacity)
            .map_err(|_| Error::AllocationFailed {
                bytes: initial_capacity,
            })?;
        Ok(Self { bytes, limit })
    }
}

impl Write for BufferedOutput {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl OutputTarget for BufferedOutput {
    type Finished = Vec<u8>;

    fn prepare_write(&mut self, _additional: usize, required: usize) -> Result<()> {
        if required <= self.bytes.capacity() {
            return Ok(());
        }
        let doubled = self.bytes.capacity().saturating_mul(2).min(self.limit);
        let target_capacity = required.max(doubled);
        let additional_capacity = target_capacity.saturating_sub(self.bytes.len());
        self.bytes
            .try_reserve_exact(additional_capacity)
            .map_err(|_| Error::AllocationFailed { bytes: required })
    }

    fn finish(self) -> io::Result<Self::Finished> {
        Ok(self.bytes)
    }
}

/// An output target that forwards encoded bytes to a caller-owned writer.
#[doc(hidden)]
pub struct ExternalOutput<W> {
    writer: W,
}

impl<W> ExternalOutput<W> {
    pub const fn new(writer: W) -> Self {
        Self { writer }
    }
}

impl<W: Write> Write for ExternalOutput<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.writer.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

impl<W: Write> OutputTarget for ExternalOutput<W> {
    type Finished = ();

    fn prepare_write(&mut self, _additional: usize, _required: usize) -> Result<()> {
        Ok(())
    }

    fn finish(mut self) -> io::Result<Self::Finished> {
        self.writer.flush()
    }
}

/// An encoded-output writer with a strict byte cap.
#[doc(hidden)]
pub struct BoundedWriter<T: OutputTarget> {
    state: Rc<RefCell<BoundedWriterState<T>>>,
}

impl<T: OutputTarget> Clone for BoundedWriter<T> {
    fn clone(&self) -> Self {
        Self {
            state: Rc::clone(&self.state),
        }
    }
}

struct BoundedWriterState<T> {
    target: T,
    bytes_written: usize,
    limit: usize,
    format: OutputFormat,
    failure: Option<WriterFailure>,
}

enum WriterFailure {
    LimitExceeded,
    AllocationFailed { bytes: usize },
    External(String),
}

impl BoundedWriter<BufferedOutput> {
    pub fn buffered(limit: usize, format: OutputFormat) -> Result<Self> {
        Self::with_target(BufferedOutput::new(limit)?, limit, format)
    }
}

impl<W: Write> BoundedWriter<ExternalOutput<W>> {
    pub fn external(writer: W, limit: usize, format: OutputFormat) -> Result<Self> {
        Self::with_target(ExternalOutput::new(writer), limit, format)
    }
}

impl<T: OutputTarget> BoundedWriter<T> {
    fn with_target(target: T, limit: usize, format: OutputFormat) -> Result<Self> {
        Ok(Self {
            state: Rc::new(RefCell::new(BoundedWriterState {
                target,
                bytes_written: 0,
                limit,
                format,
                failure: None,
            })),
        })
    }

    pub fn map_external_error(&self, message: impl Into<String>) -> Error {
        self.map_failure().unwrap_or_else(|| {
            let state = self.state.borrow();
            Error::EncodeFailure {
                format: state.format,
                message: message.into(),
            }
        })
    }

    pub fn into_output(self) -> Result<T::Finished> {
        let format = self.state.borrow().format;
        let state = Rc::try_unwrap(self.state)
            .map_err(|_| Error::EncodeFailure {
                format,
                message: "encoded output writer is still shared".to_owned(),
            })
            .map(RefCell::into_inner)?;
        state.target.finish().map_err(|error| Error::EncodeFailure {
            format,
            message: error.to_string(),
        })
    }

    fn map_failure(&self) -> Option<Error> {
        let state = self.state.borrow();
        match &state.failure {
            Some(WriterFailure::LimitExceeded) => Some(Error::EncodedOutputLimitExceeded {
                format: state.format,
                limit: state.limit,
            }),
            Some(WriterFailure::AllocationFailed { bytes }) => {
                Some(Error::AllocationFailed { bytes: *bytes })
            }
            Some(WriterFailure::External(message)) => Some(Error::EncodeFailure {
                format: state.format,
                message: message.clone(),
            }),
            None => None,
        }
    }
}

impl<T: OutputTarget> Write for BoundedWriter<T> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let mut state = self.state.borrow_mut();
        let required = state
            .bytes_written
            .checked_add(buffer.len())
            .ok_or_else(|| {
                state.failure = Some(WriterFailure::LimitExceeded);
                io::Error::other("encoded output size overflow")
            })?;
        if required > state.limit {
            state.failure = Some(WriterFailure::LimitExceeded);
            return Err(io::Error::other("encoded output limit exceeded"));
        }
        if let Err(error) = state.target.prepare_write(buffer.len(), required) {
            state.failure = Some(match error {
                Error::AllocationFailed { bytes } => WriterFailure::AllocationFailed { bytes },
                other => WriterFailure::External(other.to_string()),
            });
            return Err(io::Error::other("encoded output target preparation failed"));
        }
        match state.target.write(buffer) {
            Ok(written) => {
                state.bytes_written = state
                    .bytes_written
                    .checked_add(written)
                    .ok_or_else(|| io::Error::other("encoded output size overflow"))?;
                Ok(written)
            }
            Err(error) => {
                state.failure = Some(WriterFailure::External(error.to_string()));
                Err(error)
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut state = self.state.borrow_mut();
        state.target.flush().inspect_err(|error| {
            state.failure = Some(WriterFailure::External(error.to_string()));
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FlushFailure;

    impl Write for FlushFailure {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("injected flush failure"))
        }
    }

    #[test]
    fn refuses_to_grow_past_the_cap() {
        let mut writer = BoundedWriter::buffered(3, OutputFormat::Jpeg).unwrap();
        writer.write_all(&[1, 2, 3]).unwrap();
        assert!(writer.write_all(&[4]).is_err());
        assert!(matches!(
            writer.map_external_error("ignored"),
            Error::EncodedOutputLimitExceeded {
                format: OutputFormat::Jpeg,
                limit: 3
            }
        ));
    }

    #[test]
    fn forwards_bytes_without_retaining_them() {
        let mut output = Vec::new();
        {
            let mut writer = BoundedWriter::external(&mut output, 4, OutputFormat::Png).unwrap();
            writer.write_all(&[1, 2, 3, 4]).unwrap();
            writer.into_output().unwrap();
        }
        assert_eq!(output, [1, 2, 3, 4]);
    }

    #[test]
    fn reports_external_flush_failures_when_output_finishes() {
        let writer = BoundedWriter::external(FlushFailure, 4, OutputFormat::Png).unwrap();

        assert!(matches!(
            writer.into_output(),
            Err(Error::EncodeFailure {
                format: OutputFormat::Png,
                message,
            }) if message == "injected flush failure"
        ));
    }
}
