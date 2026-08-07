//! Bounded row-streaming output encoders for Streamthumb.

mod error;
mod jpeg;
mod writer;

pub use error::{Error, Result};
pub use jpeg::{JpegOptions, JpegRowSink, JpegSubsampling, JpegWriterRowSink};
#[doc(hidden)]
pub use writer::{BoundedWriter, BufferedOutput, ExternalOutput, OutputTarget};
