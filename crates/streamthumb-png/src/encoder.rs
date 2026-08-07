use std::{
    cell::RefCell,
    io::{self, Write},
    rc::Rc,
};

use streamthumb_core::{Dimensions, Error as CoreError, RgbaRowSink};

#[cfg(test)]
use streamthumb_core::RgbaImage;

use crate::{Error, Result};

#[cfg(test)]
pub(crate) fn encode_rgba_png(image: &RgbaImage, byte_limit: usize) -> Result<Vec<u8>> {
    let mut sink = PngRowSink::new(image.dimensions, byte_limit)?;
    let row_bytes = rgba_row_bytes(image.dimensions)?;
    for (y, row) in image.pixels.chunks_exact(row_bytes).enumerate() {
        sink.push_row(
            u32::try_from(y).map_err(|_| CoreError::IntegerOverflow {
                operation: "PNG encoder row index",
            })?,
            row,
        )?;
    }
    sink.finish()
}

pub(crate) struct PngRowSink {
    dimensions: Dimensions,
    next_y: u32,
    stream: png::StreamWriter<'static, BoundedWriter>,
    output: BoundedWriter,
}

impl PngRowSink {
    pub(crate) fn new(dimensions: Dimensions, byte_limit: usize) -> Result<Self> {
        let output = BoundedWriter::new(byte_limit)?;
        let mut encoder = png::Encoder::new(output.clone(), dimensions.width, dimensions.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let writer = encoder
            .write_header()
            .map_err(|error| output.map_encoding_error(error))?;
        let stream = writer
            .into_stream_writer()
            .map_err(|error| output.map_encoding_error(error))?;
        Ok(Self {
            dimensions,
            next_y: 0,
            stream,
            output,
        })
    }
}

impl RgbaRowSink for PngRowSink {
    type Output = Vec<u8>;
    type Error = Error;

    fn push_row(&mut self, y: u32, rgba: &[u8]) -> Result<()> {
        if y != self.next_y {
            return Err(CoreError::UnexpectedRow {
                expected: self.next_y,
                actual: y,
            }
            .into());
        }
        let expected_len = rgba_row_bytes(self.dimensions)?;
        if rgba.len() != expected_len {
            return Err(CoreError::InvalidRowLength {
                expected: expected_len,
                actual: rgba.len(),
            }
            .into());
        }
        self.stream
            .write_all(rgba)
            .map_err(|error| self.output.map_io_error(error))?;
        self.next_y = self
            .next_y
            .checked_add(1)
            .ok_or(CoreError::IntegerOverflow {
                operation: "PNG encoder row index",
            })?;
        Ok(())
    }

    fn finish(self) -> Result<Vec<u8>> {
        if self.next_y != self.dimensions.height {
            return Err(CoreError::IncompleteImage {
                expected_rows: self.dimensions.height,
                actual_rows: self.next_y,
            }
            .into());
        }
        self.stream
            .finish()
            .map_err(|error| self.output.map_encoding_error(error))?;
        self.output.into_bytes()
    }
}

fn rgba_row_bytes(dimensions: Dimensions) -> Result<usize> {
    usize::try_from(dimensions.width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or_else(|| {
            CoreError::IntegerOverflow {
                operation: "PNG encoder row size",
            }
            .into()
        })
}

#[derive(Clone)]
struct BoundedWriter {
    state: Rc<RefCell<BoundedWriterState>>,
}

struct BoundedWriterState {
    bytes: Vec<u8>,
    limit: usize,
    failure: Option<WriterFailure>,
}

#[derive(Clone, Copy)]
enum WriterFailure {
    LimitExceeded,
    AllocationFailed { bytes: usize },
}

impl BoundedWriter {
    fn new(limit: usize) -> Result<Self> {
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
                failure: None,
            })),
        })
    }

    fn map_encoding_error(&self, error: png::EncodingError) -> Error {
        self.map_failure()
            .unwrap_or_else(|| Error::EncodeFailure(error.to_string()))
    }

    fn map_io_error(&self, error: io::Error) -> Error {
        self.map_failure()
            .unwrap_or_else(|| Error::EncodeFailure(error.to_string()))
    }

    fn map_failure(&self) -> Option<Error> {
        let state = self.state.borrow();
        match state.failure {
            Some(WriterFailure::LimitExceeded) => {
                Some(Error::EncodedOutputLimitExceeded { limit: state.limit })
            }
            Some(WriterFailure::AllocationFailed { bytes }) => {
                Some(Error::AllocationFailed { bytes })
            }
            None => None,
        }
    }

    fn into_bytes(self) -> Result<Vec<u8>> {
        Rc::try_unwrap(self.state)
            .map_err(|_| Error::EncodeFailure("PNG output writer is still shared".to_owned()))
            .map(RefCell::into_inner)
            .map(|state| state.bytes)
    }
}

impl Write for BoundedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let mut state = self.state.borrow_mut();
        let required = state.bytes.len().checked_add(buffer.len()).ok_or_else(|| {
            state.failure = Some(WriterFailure::LimitExceeded);
            io::Error::other("encoded PNG output size overflow")
        })?;
        if required > state.limit {
            state.failure = Some(WriterFailure::LimitExceeded);
            return Err(io::Error::other("encoded PNG output limit exceeded"));
        }
        if let Err(_error) = state.bytes.try_reserve_exact(buffer.len()) {
            state.failure = Some(WriterFailure::AllocationFailed { bytes: required });
            return Err(io::Error::other("encoded PNG output allocation failed"));
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
    fn encoded_output_cannot_grow_past_its_limit() {
        let image = RgbaImage {
            dimensions: Dimensions::new(1, 1).unwrap(),
            pixels: vec![1, 2, 3, 4],
        };
        assert!(matches!(
            encode_rgba_png(&image, 1),
            Err(Error::EncodedOutputLimitExceeded { limit: 1 })
        ));
    }

    #[test]
    fn row_sink_rejects_out_of_order_rows() {
        let dimensions = Dimensions::new(1, 1).unwrap();
        let mut sink = PngRowSink::new(dimensions, 1024).unwrap();
        assert!(matches!(
            sink.push_row(1, &[0, 0, 0, 255]),
            Err(Error::Core(CoreError::UnexpectedRow {
                expected: 0,
                actual: 1
            }))
        ));
    }
}
