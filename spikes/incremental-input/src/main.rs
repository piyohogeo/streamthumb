use std::{
    cell::RefCell,
    env,
    error::Error as StdError,
    fmt,
    fs::File,
    io::{self, BufReader, Read, Seek, SeekFrom},
    path::PathBuf,
    rc::Rc,
};

const DEFAULT_INPUT_LIMIT: u64 = 64 * 1024 * 1024;
const DEFAULT_READ_CHUNK: usize = 4 * 1024;
const DECODER_LIMIT: usize = 8 * 1024 * 1024;

fn main() -> Result<(), Box<dyn StdError>> {
    let path = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: streamthumb-incremental-input-spike <input.png>")?;
    let result = decode_seekable_png(File::open(path)?, DEFAULT_INPUT_LIMIT, DEFAULT_READ_CHUNK)?;
    println!(
        "decoded {}x{} in {} rows from {} bytes using {} reads (largest read: {} bytes)",
        result.width,
        result.height,
        result.rows,
        result.encoded_bytes,
        result.read_calls,
        result.largest_read,
    );
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DecodeResult {
    width: u32,
    height: u32,
    rows: u32,
    encoded_bytes: u64,
    read_calls: usize,
    largest_read: usize,
}

#[derive(Debug)]
enum SpikeError {
    Io(io::Error),
    Decode(png::DecodingError),
    InputLimitExceeded { actual: u64, limit: u64 },
    InvalidReaderPosition,
}

impl fmt::Display for SpikeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Decode(error) => error.fmt(formatter),
            Self::InputLimitExceeded { actual, limit } => {
                write!(
                    formatter,
                    "input uses {actual} bytes, exceeding the {limit}-byte limit"
                )
            }
            Self::InvalidReaderPosition => {
                formatter.write_str("reader end position precedes its start position")
            }
        }
    }
}

impl StdError for SpikeError {}

impl From<io::Error> for SpikeError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<png::DecodingError> for SpikeError {
    fn from(error: png::DecodingError) -> Self {
        Self::Decode(error)
    }
}

#[derive(Default)]
struct ReadStats {
    calls: usize,
    largest: usize,
}

struct ChunkedReader<R> {
    inner: R,
    max_read: usize,
    stats: Rc<RefCell<ReadStats>>,
}

impl<R> ChunkedReader<R> {
    fn new(inner: R, max_read: usize, stats: Rc<RefCell<ReadStats>>) -> Self {
        assert!(max_read > 0);
        Self {
            inner,
            max_read,
            stats,
        }
    }
}

impl<R: Read> Read for ChunkedReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let requested = output.len().min(self.max_read);
        let read = self.inner.read(&mut output[..requested])?;
        if read > 0 {
            let mut stats = self.stats.borrow_mut();
            stats.calls += 1;
            stats.largest = stats.largest.max(read);
        }
        Ok(read)
    }
}

impl<R: Seek> Seek for ChunkedReader<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}

fn decode_seekable_png<R: Read + Seek>(
    mut input: R,
    input_limit: u64,
    max_read: usize,
) -> Result<DecodeResult, SpikeError> {
    let start = input.stream_position()?;
    let end = input.seek(SeekFrom::End(0))?;
    let encoded_bytes = end
        .checked_sub(start)
        .ok_or(SpikeError::InvalidReaderPosition)?;
    input.seek(SeekFrom::Start(start))?;
    if encoded_bytes > input_limit {
        return Err(SpikeError::InputLimitExceeded {
            actual: encoded_bytes,
            limit: input_limit,
        });
    }

    let stats = Rc::new(RefCell::new(ReadStats::default()));
    let input = ChunkedReader::new(input, max_read, Rc::clone(&stats));
    let buffered = BufReader::with_capacity(max_read, input);
    let decoder_limits = png::Limits {
        bytes: DECODER_LIMIT,
    };
    let mut reader = png::Decoder::new_with_limits(buffered, decoder_limits).read_info()?;
    let width = reader.info().width;
    let height = reader.info().height;
    let mut rows = 0_u32;
    while reader.next_row()?.is_some() {
        rows = rows
            .checked_add(1)
            .ok_or(SpikeError::InvalidReaderPosition)?;
    }
    reader.finish()?;

    let stats = stats.borrow();
    Ok(DecodeResult {
        width,
        height,
        rows,
        encoded_bytes,
        read_calls: stats.calls,
        largest_read: stats.largest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn high_entropy_png() -> Vec<u8> {
        const DIMENSION: u32 = 256;
        let mut state = 0xa341_316c_u32;
        let mut pixels = Vec::with_capacity((DIMENSION * DIMENSION * 3) as usize);
        for _ in 0..DIMENSION * DIMENSION * 3 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            pixels.push(state as u8);
        }

        let mut encoded = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut encoded, DIMENSION, DIMENSION);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::Fast);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&pixels).unwrap();
        }
        encoded
    }

    #[test]
    fn decodes_rows_without_reading_the_complete_input_at_once() {
        let input = high_entropy_png();
        let result = decode_seekable_png(Cursor::new(&input), input.len() as u64, 23).unwrap();

        assert_eq!(result.width, 256);
        assert_eq!(result.height, 256);
        assert_eq!(result.rows, 256);
        assert_eq!(result.encoded_bytes, input.len() as u64);
        assert!(result.read_calls > 1_000);
        assert!(result.largest_read <= 23);
    }

    #[test]
    fn rejects_the_encoded_length_before_decoding() {
        let input = high_entropy_png();
        let error = decode_seekable_png(Cursor::new(&input), input.len() as u64 - 1, 23)
            .expect_err("the encoded byte limit must reject the input");

        assert!(matches!(
            error,
            SpikeError::InputLimitExceeded { actual, limit }
                if actual == input.len() as u64 && limit == input.len() as u64 - 1
        ));
    }
}
