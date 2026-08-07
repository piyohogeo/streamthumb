use std::{
    cell::RefCell,
    io::{self, Write},
    rc::Rc,
};

use streamthumb_core::OutputFormat;

use crate::{Error, Result};

/// A growing encoded-output buffer with a strict allocation cap.
#[derive(Clone)]
pub struct BoundedWriter {
    state: Rc<RefCell<BoundedWriterState>>,
}

struct BoundedWriterState {
    bytes: Vec<u8>,
    limit: usize,
    format: OutputFormat,
    failure: Option<WriterFailure>,
}

#[derive(Clone, Copy)]
enum WriterFailure {
    LimitExceeded,
    AllocationFailed { bytes: usize },
}

impl BoundedWriter {
    pub fn new(limit: usize, format: OutputFormat) -> Result<Self> {
        let initial_capacity = limit.min(64 * 1024);
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(initial_capacity)
            .map_err(|_| Error::AllocationFailed {
                bytes: initial_capacity,
            })?;
        Ok(Self {
            state: Rc::new(RefCell::new(BoundedWriterState {
                bytes,
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

    pub fn into_bytes(self) -> Result<Vec<u8>> {
        let format = self.state.borrow().format;
        Rc::try_unwrap(self.state)
            .map_err(|_| Error::EncodeFailure {
                format,
                message: "encoded output writer is still shared".to_owned(),
            })
            .map(RefCell::into_inner)
            .map(|state| state.bytes)
    }

    fn map_failure(&self) -> Option<Error> {
        let state = self.state.borrow();
        match state.failure {
            Some(WriterFailure::LimitExceeded) => Some(Error::EncodedOutputLimitExceeded {
                format: state.format,
                limit: state.limit,
            }),
            Some(WriterFailure::AllocationFailed { bytes }) => {
                Some(Error::AllocationFailed { bytes })
            }
            None => None,
        }
    }
}

impl Write for BoundedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let mut state = self.state.borrow_mut();
        let required = state.bytes.len().checked_add(buffer.len()).ok_or_else(|| {
            state.failure = Some(WriterFailure::LimitExceeded);
            io::Error::other("encoded output size overflow")
        })?;
        if required > state.limit {
            state.failure = Some(WriterFailure::LimitExceeded);
            return Err(io::Error::other("encoded output limit exceeded"));
        }
        if state.bytes.try_reserve_exact(buffer.len()).is_err() {
            state.failure = Some(WriterFailure::AllocationFailed { bytes: required });
            return Err(io::Error::other("encoded output allocation failed"));
        }
        state.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_to_grow_past_the_cap() {
        let mut writer = BoundedWriter::new(3, OutputFormat::Jpeg).unwrap();
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
}
