//! Isolated browser `Blob` seekable-input experiment.

#[cfg(any(target_arch = "wasm32", test))]
use std::io::SeekFrom;

#[cfg(any(target_arch = "wasm32", test))]
const MAX_SAFE_INTEGER: u64 = (1_u64 << 53) - 1;

#[cfg(any(target_arch = "wasm32", test))]
fn checked_seek_position(length: u64, current: u64, position: SeekFrom) -> std::io::Result<u64> {
    let next = match position {
        SeekFrom::Start(offset) => i128::from(offset),
        SeekFrom::Current(delta) => i128::from(current) + i128::from(delta),
        SeekFrom::End(delta) => i128::from(length) + i128::from(delta),
    };
    if !(0..=i128::from(length)).contains(&next) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "seek position is outside the encoded input",
        ));
    }
    u64::try_from(next).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "seek position cannot be represented",
        )
    })
}

#[cfg(any(target_arch = "wasm32", test))]
fn checked_input_length(value: f64) -> Result<u64, &'static str> {
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 {
        return Err("inputLength must be a non-negative integer");
    }
    if value > MAX_SAFE_INTEGER as f64 {
        return Err("inputLength exceeds the JavaScript safe-integer range");
    }
    Ok(value as u64)
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    #[cfg(test)]
    use std::io::{self, Write};
    use std::{
        cell::RefCell,
        io::{Read, Seek, SeekFrom},
        rc::Rc,
    };

    use js_sys::{Function, Uint8Array};
    use streamthumb_core::ThumbnailOptions;
    use streamthumb_png::thumbnail_png_rgba_from_reader;
    use wasm_bindgen::{JsCast, JsError, JsValue, prelude::wasm_bindgen};

    use super::{checked_input_length, checked_seek_position};

    #[cfg(test)]
    const OUTPUT_CHUNK_BYTES: usize = 64 * 1024;

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    struct ReadStats {
        read_calls: u32,
        bytes_read_total: u64,
        largest_read: u32,
        seek_calls: u32,
        minimum_offset: u64,
        maximum_offset: u64,
    }

    struct JsSeekableReader {
        length: u64,
        position: u64,
        read_at: Function,
        callback_error: Rc<RefCell<Option<JsValue>>>,
        stats: Rc<RefCell<ReadStats>>,
    }

    #[cfg(test)]
    struct ChunkCallbackWriter<F> {
        state: Rc<RefCell<ChunkCallbackState<F>>>,
    }

    #[cfg(test)]
    struct ChunkCallbackState<F> {
        buffer: Vec<u8>,
        callback: F,
    }

    #[cfg(test)]
    impl<F> Clone for ChunkCallbackWriter<F> {
        fn clone(&self) -> Self {
            Self {
                state: Rc::clone(&self.state),
            }
        }
    }

    #[cfg(test)]
    impl<F> ChunkCallbackWriter<F>
    where
        F: FnMut(&[u8]) -> io::Result<()>,
    {
        fn new(callback: F) -> io::Result<Self> {
            let mut buffer = Vec::new();
            buffer
                .try_reserve_exact(OUTPUT_CHUNK_BYTES)
                .map_err(|_| io::Error::other("could not allocate the output chunk buffer"))?;
            Ok(Self {
                state: Rc::new(RefCell::new(ChunkCallbackState { buffer, callback })),
            })
        }

        fn finish(&self) -> io::Result<()> {
            self.state.borrow_mut().emit()
        }
    }

    #[cfg(test)]
    impl<F> ChunkCallbackState<F>
    where
        F: FnMut(&[u8]) -> io::Result<()>,
    {
        fn emit(&mut self) -> io::Result<()> {
            if self.buffer.is_empty() {
                return Ok(());
            }
            (self.callback)(&self.buffer)?;
            self.buffer.clear();
            Ok(())
        }
    }

    #[cfg(test)]
    impl<F> Write for ChunkCallbackWriter<F>
    where
        F: FnMut(&[u8]) -> io::Result<()>,
    {
        fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
            let original_len = bytes.len();
            let mut state = self.state.borrow_mut();
            while !bytes.is_empty() {
                let available = OUTPUT_CHUNK_BYTES - state.buffer.len();
                let take = available.min(bytes.len());
                state.buffer.extend_from_slice(&bytes[..take]);
                bytes = &bytes[take..];
                if state.buffer.len() == OUTPUT_CHUNK_BYTES {
                    state.emit()?;
                }
            }
            Ok(original_len)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.finish()
        }
    }

    impl JsSeekableReader {
        fn new(
            length: u64,
            read_at: Function,
            callback_error: Rc<RefCell<Option<JsValue>>>,
            stats: Rc<RefCell<ReadStats>>,
        ) -> Self {
            Self {
                length,
                position: 0,
                read_at,
                callback_error,
                stats,
            }
        }

        fn callback_failure(&self, error: JsValue) -> std::io::Error {
            *self.callback_error.borrow_mut() = Some(error);
            std::io::Error::other("JavaScript input callback failed")
        }
    }

    impl Read for JsSeekableReader {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            if output.is_empty() || self.position == self.length {
                return Ok(0);
            }
            let remaining = self.length.checked_sub(self.position).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "reader position exceeds input length",
                )
            })?;
            let requested = usize::try_from(remaining.min(output.len() as u64)).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "requested read cannot be represented",
                )
            })?;
            let requested_u32 = u32::try_from(requested).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "requested read exceeds the adapter range",
                )
            })?;
            let value = self
                .read_at
                .call2(
                    &JsValue::UNDEFINED,
                    &JsValue::from_f64(self.position as f64),
                    &JsValue::from_f64(requested as f64),
                )
                .map_err(|error| self.callback_failure(error))?;
            if !value.is_instance_of::<Uint8Array>() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "readAt must return a Uint8Array",
                ));
            }
            let bytes = value.unchecked_into::<Uint8Array>();
            if bytes.length() != requested_u32 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "readAt returned a different byte length than requested",
                ));
            }
            bytes.copy_to(&mut output[..requested]);

            let offset = self.position;
            self.position = self.position.checked_add(requested as u64).ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "reader position overflow")
            })?;
            let mut stats = self.stats.borrow_mut();
            stats.read_calls = stats
                .read_calls
                .checked_add(1)
                .ok_or_else(|| std::io::Error::other("read call count overflow"))?;
            stats.bytes_read_total = stats
                .bytes_read_total
                .checked_add(requested as u64)
                .ok_or_else(|| std::io::Error::other("read byte count overflow"))?;
            stats.largest_read = stats.largest_read.max(requested_u32);
            if stats.read_calls == 1 {
                stats.minimum_offset = offset;
            } else {
                stats.minimum_offset = stats.minimum_offset.min(offset);
            }
            stats.maximum_offset = stats.maximum_offset.max(offset);
            Ok(requested)
        }
    }

    impl Seek for JsSeekableReader {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.position = checked_seek_position(self.length, self.position, position)?;
            let mut stats = self.stats.borrow_mut();
            stats.seek_calls = stats
                .seek_calls
                .checked_add(1)
                .ok_or_else(|| std::io::Error::other("seek call count overflow"))?;
            Ok(self.position)
        }
    }

    /// RGBA result and input-I/O measurements for the isolated spike.
    #[derive(Debug)]
    #[wasm_bindgen]
    pub struct SpikeRgbaResult {
        pixels: Vec<u8>,
        width: u32,
        height: u32,
        stats: ReadStats,
    }

    #[wasm_bindgen]
    impl SpikeRgbaResult {
        #[wasm_bindgen(getter)]
        pub fn pixels(&self) -> Uint8Array {
            Uint8Array::from(self.pixels.as_slice())
        }

        #[wasm_bindgen(getter)]
        pub fn width(&self) -> u32 {
            self.width
        }

        #[wasm_bindgen(getter)]
        pub fn height(&self) -> u32 {
            self.height
        }

        #[wasm_bindgen(getter, js_name = readCalls)]
        pub fn read_calls(&self) -> u32 {
            self.stats.read_calls
        }

        #[wasm_bindgen(getter, js_name = bytesReadTotal)]
        pub fn bytes_read_total(&self) -> f64 {
            self.stats.bytes_read_total as f64
        }

        #[wasm_bindgen(getter, js_name = largestRead)]
        pub fn largest_read(&self) -> u32 {
            self.stats.largest_read
        }

        #[wasm_bindgen(getter, js_name = seekCalls)]
        pub fn seek_calls(&self) -> u32 {
            self.stats.seek_calls
        }
    }

    fn run_rgba(
        input_length: f64,
        read_at: &Function,
        options: &ThumbnailOptions,
    ) -> Result<SpikeRgbaResult, JsValue> {
        let length = checked_input_length(input_length).map_err(JsError::new)?;
        let callback_error = Rc::new(RefCell::new(None));
        let stats = Rc::new(RefCell::new(ReadStats::default()));
        let reader = JsSeekableReader::new(
            length,
            read_at.clone(),
            Rc::clone(&callback_error),
            Rc::clone(&stats),
        );
        let image = match thumbnail_png_rgba_from_reader(reader, options) {
            Ok(image) => image,
            Err(error) => {
                if let Some(callback_error) = callback_error.borrow_mut().take() {
                    return Err(callback_error);
                }
                return Err(JsError::new(&error.to_string()).into());
            }
        };
        let stats = *stats.borrow();
        Ok(SpikeRgbaResult {
            pixels: image.pixels,
            width: image.dimensions.width,
            height: image.dimensions.height,
            stats,
        })
    }

    /// Runs the first browser File/Blob input experiment through production decoding.
    #[wasm_bindgen(js_name = spikeThumbnailRgbaFromSeekable)]
    pub fn spike_thumbnail_rgba_from_seekable(
        input_length: f64,
        read_at: &Function,
        max_input_bytes: f64,
    ) -> Result<SpikeRgbaResult, JsValue> {
        let max_input_bytes = checked_input_length(max_input_bytes).map_err(JsError::new)?;
        let mut options = ThumbnailOptions::default();
        options.limits.max_input_bytes = max_input_bytes;
        run_rgba(input_length, read_at, &options)
    }

    #[cfg(test)]
    mod tests {
        use std::io::{Read, Seek, SeekFrom, Write};

        use flate2::{Compression, write::ZlibEncoder};
        use js_sys::{Date, Function, Object, Reflect, Uint8Array, WebAssembly};
        use streamthumb_core::{OutputFormat, ThumbnailOptions};
        use streamthumb_png::{
            JpegOptions, JpegSubsampling, PngCompression, PngOptions, ThumbnailOutput,
            thumbnail_jpeg_from_reader_to_writer_with_options_and_buffer,
            thumbnail_png_from_reader_to_writer_with_encoder_options_and_buffer,
            thumbnail_png_from_reader_with_encoder_options,
            thumbnail_png_from_reader_with_jpeg_options, thumbnail_png_rgba,
            thumbnail_png_rgba_from_reader, thumbnail_png_with_encoder_options,
            thumbnail_png_with_jpeg_options,
        };
        use wasm_bindgen::{JsCast, JsValue};
        use wasm_bindgen_test::*;

        use super::{
            ChunkCallbackWriter, JsSeekableReader, OUTPUT_CHUNK_BYTES, ReadStats, run_rgba,
        };
        use std::{cell::RefCell, rc::Rc};

        wasm_bindgen_test_configure!(run_in_dedicated_worker);

        const RGBA_FIXTURE: &[u8] =
            include_bytes!("../../../fuzz/corpus/thumbnail_png/pngsuite_basn6a08.png");
        const INVALID_SIGNATURE: &[u8] =
            include_bytes!("../../../fuzz/corpus/thumbnail_png/invalid-signature");
        const FIXTURES: &[(&str, &[u8])] = &[
            (
                "grayscale-8",
                include_bytes!("../../../fuzz/corpus/thumbnail_png/pngsuite_basn0g08.png"),
            ),
            (
                "rgb-8",
                include_bytes!("../../../fuzz/corpus/thumbnail_png/pngsuite_basn2c08.png"),
            ),
            (
                "rgb-16",
                include_bytes!("../../../fuzz/corpus/thumbnail_png/pngsuite_basn2c16.png"),
            ),
            (
                "palette-8",
                include_bytes!("../../../fuzz/corpus/thumbnail_png/pngsuite_basn3p08.png"),
            ),
            (
                "rgba-8",
                include_bytes!("../../../fuzz/corpus/thumbnail_png/pngsuite_basn6a08.png"),
            ),
            (
                "rgba-16",
                include_bytes!("../../../fuzz/corpus/thumbnail_png/pngsuite_basn6a16.png"),
            ),
        ];
        const ADAM7_PASSES: [(u32, u32, u32, u32); 7] = [
            (0, 0, 8, 8),
            (4, 0, 8, 8),
            (0, 4, 4, 8),
            (2, 0, 4, 4),
            (0, 2, 2, 4),
            (1, 0, 2, 2),
            (0, 1, 1, 2),
        ];

        #[derive(Clone, Default)]
        struct CapturingWriter {
            bytes: Rc<RefCell<Vec<u8>>>,
        }

        impl CapturingWriter {
            fn bytes(&self) -> Vec<u8> {
                self.bytes.borrow().clone()
            }
        }

        impl Write for CapturingWriter {
            fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
                self.bytes.borrow_mut().extend_from_slice(buffer);
                Ok(buffer.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        fn encoded_bytes(output: ThumbnailOutput) -> Vec<u8> {
            match output {
                ThumbnailOutput::Encoded { bytes, .. } => bytes,
                ThumbnailOutput::Rgba { .. } => panic!("expected encoded thumbnail output"),
                _ => panic!("unexpected thumbnail output variant"),
            }
        }

        fn crc32(bytes: &[u8]) -> u32 {
            let mut crc = u32::MAX;
            for byte in bytes {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    let mask = 0_u32.wrapping_sub(crc & 1);
                    crc = (crc >> 1) ^ (0xedb8_8320 & mask);
                }
            }
            !crc
        }

        fn append_chunk(png: &mut Vec<u8>, chunk_type: [u8; 4], data: &[u8]) {
            png.extend_from_slice(&(data.len() as u32).to_be_bytes());
            png.extend_from_slice(&chunk_type);
            png.extend_from_slice(data);
            let start = png.len() - data.len() - chunk_type.len();
            png.extend_from_slice(&crc32(&png[start..]).to_be_bytes());
        }

        fn insert_chunk_after_ihdr(input: &[u8], chunk_type: [u8; 4], data: &[u8]) -> Vec<u8> {
            const AFTER_IHDR: usize = 8 + 4 + 4 + 13 + 4;
            let mut output = input[..AFTER_IHDR].to_vec();
            append_chunk(&mut output, chunk_type, data);
            output.extend_from_slice(&input[AFTER_IHDR..]);
            output
        }

        fn apng_fixture() -> Vec<u8> {
            let mut animation_control = Vec::with_capacity(8);
            animation_control.extend_from_slice(&1_u32.to_be_bytes());
            animation_control.extend_from_slice(&0_u32.to_be_bytes());
            insert_chunk_after_ihdr(RGBA_FIXTURE, *b"acTL", &animation_control)
        }

        fn truncated_ancillary_chunk_fixture() -> Vec<u8> {
            const AFTER_IHDR: usize = 8 + 4 + 4 + 13 + 4;
            let mut output = RGBA_FIXTURE[..AFTER_IHDR].to_vec();
            output.extend_from_slice(&64_u32.to_be_bytes());
            output.extend_from_slice(b"tEXt");
            output.extend_from_slice(b"incomplete");
            output
        }

        fn high_entropy_png(dimension: u32) -> Vec<u8> {
            let mut state = 0x6d2b_79f5_u32;
            let mut pixels = Vec::with_capacity((dimension * dimension * 3) as usize);
            for _ in 0..dimension * dimension * 3 {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                pixels.push(state as u8);
            }

            let mut input = Vec::new();
            {
                let mut encoder = png::Encoder::new(&mut input, dimension, dimension);
                encoder.set_color(png::ColorType::Rgb);
                encoder.set_depth(png::BitDepth::Eight);
                encoder.set_compression(png::Compression::Fast);
                let mut writer = encoder.write_header().unwrap();
                writer.write_image_data(&pixels).unwrap();
            }
            input
        }

        fn wasm_memory_bytes() -> u32 {
            let memory = wasm_bindgen::memory().unchecked_into::<WebAssembly::Memory>();
            memory
                .buffer()
                .unchecked_into::<js_sys::ArrayBuffer>()
                .byte_length()
        }

        fn pass_sample_count(extent: u32, offset: u32, stride: u32) -> u32 {
            if extent <= offset {
                0
            } else {
                (extent - offset).div_ceil(stride)
            }
        }

        fn adam7_rgba_fixture() -> Vec<u8> {
            const WIDTH: u32 = 17;
            const HEIGHT: u32 = 13;
            let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    pixels.extend_from_slice(&[
                        (x * 13 + y * 3) as u8,
                        (x * 5 + y * 17) as u8,
                        (x * 19 + y * 7) as u8,
                        255_u8.wrapping_sub((x * 9 + y * 11) as u8),
                    ]);
                }
            }

            let mut filtered = Vec::new();
            for (x_offset, y_offset, x_stride, y_stride) in ADAM7_PASSES {
                let samples = pass_sample_count(WIDTH, x_offset, x_stride);
                let lines = pass_sample_count(HEIGHT, y_offset, y_stride);
                for line in 0..lines {
                    filtered.push(0);
                    let y = y_offset + line * y_stride;
                    for sample in 0..samples {
                        let x = x_offset + sample * x_stride;
                        let offset = ((y * WIDTH + x) * 4) as usize;
                        filtered.extend_from_slice(&pixels[offset..offset + 4]);
                    }
                }
            }

            let mut compressor = ZlibEncoder::new(Vec::new(), Compression::default());
            compressor.write_all(&filtered).unwrap();
            let compressed = compressor.finish().unwrap();
            let mut encoded = b"\x89PNG\r\n\x1a\n".to_vec();
            let mut ihdr = Vec::with_capacity(13);
            ihdr.extend_from_slice(&WIDTH.to_be_bytes());
            ihdr.extend_from_slice(&HEIGHT.to_be_bytes());
            ihdr.extend_from_slice(&[8, 6, 0, 0, 1]);
            append_chunk(&mut encoded, *b"IHDR", &ihdr);
            append_chunk(&mut encoded, *b"IDAT", &compressed);
            append_chunk(&mut encoded, *b"IEND", &[]);
            encoded
        }

        fn blob_harness(input: &[u8]) -> (Function, Object) {
            let factory = Function::new_with_args(
                "input",
                r#"
                    const blob = new Blob([input]);
                    const reader = new FileReaderSync();
                    const stats = { calls: 0, largest: 0 };
                    const readAt = (offset, length) => {
                        stats.calls += 1;
                        stats.largest = Math.max(stats.largest, length);
                        return new Uint8Array(
                            reader.readAsArrayBuffer(blob.slice(offset, offset + length)),
                        );
                    };
                    return { readAt, stats };
                "#,
            );
            let input = Uint8Array::from(input);
            let harness = factory
                .call1(&JsValue::UNDEFINED, input.as_ref())
                .unwrap()
                .unchecked_into::<Object>();
            let read_at = Reflect::get(&harness, &JsValue::from_str("readAt"))
                .unwrap()
                .unchecked_into::<Function>();
            let stats = Reflect::get(&harness, &JsValue::from_str("stats"))
                .unwrap()
                .unchecked_into::<Object>();
            (read_at, stats)
        }

        fn new_reader(length: usize, read_at: Function) -> JsSeekableReader {
            JsSeekableReader::new(
                length as u64,
                read_at,
                Rc::new(RefCell::new(None)),
                Rc::new(RefCell::new(ReadStats::default())),
            )
        }

        fn error_message(error: &JsValue) -> String {
            error
                .as_string()
                .or_else(|| {
                    Reflect::get(error, &JsValue::from_str("message"))
                        .ok()
                        .and_then(|message| message.as_string())
                })
                .expect("error must expose a diagnostic message")
        }

        #[wasm_bindgen_test]
        fn blob_file_reader_sync_matches_the_slice_rgba_path() {
            let (read_at, js_stats) = blob_harness(RGBA_FIXTURE);
            let options = ThumbnailOptions::default();
            let expected = thumbnail_png_rgba(RGBA_FIXTURE, &options).unwrap();
            let actual = run_rgba(RGBA_FIXTURE.len() as f64, &read_at, &options).unwrap();
            assert_eq!(actual.width, expected.dimensions.width);
            assert_eq!(actual.height, expected.dimensions.height);
            assert_eq!(actual.pixels, expected.pixels);
            assert!(actual.stats.read_calls > 0);
            assert!(actual.stats.seek_calls >= 3);
            assert!(actual.stats.largest_read <= 8 * 1024);
            assert!(actual.stats.bytes_read_total >= RGBA_FIXTURE.len() as u64);
            assert_eq!(
                Reflect::get(&js_stats, &JsValue::from_str("calls"))
                    .unwrap()
                    .as_f64()
                    .unwrap() as u32,
                actual.stats.read_calls,
            );
        }

        #[wasm_bindgen_test]
        fn all_corpus_color_and_depth_fixtures_match_the_slice_rgba_path() {
            let options = ThumbnailOptions::default();
            for (name, input) in FIXTURES {
                let expected = thumbnail_png_rgba(input, &options)
                    .unwrap_or_else(|error| panic!("{name} slice decode failed: {error}"));
                let (read_at, _) = blob_harness(input);
                let actual = run_rgba(input.len() as f64, &read_at, &options)
                    .unwrap_or_else(|error| panic!("{name} seekable decode failed: {error:?}"));
                assert_eq!(actual.width, expected.dimensions.width, "{name} width");
                assert_eq!(actual.height, expected.dimensions.height, "{name} height");
                assert_eq!(actual.pixels, expected.pixels, "{name} pixels");
                assert!(actual.stats.read_calls > 0, "{name} read calls");
                assert!(actual.stats.largest_read <= 8 * 1024, "{name} largest read");
            }
        }

        #[wasm_bindgen_test]
        fn buffered_png_and_jpeg_outputs_match_the_slice_paths() {
            for (name, input) in FIXTURES {
                let mut options = ThumbnailOptions {
                    output: OutputFormat::Png,
                    ..ThumbnailOptions::default()
                };
                let expected =
                    thumbnail_png_with_encoder_options(input, &options, &PngOptions::default())
                        .unwrap_or_else(|error| panic!("{name} slice PNG failed: {error}"));
                let (read_at, _) = blob_harness(input);
                let reader = new_reader(input.len(), read_at);
                let actual = thumbnail_png_from_reader_with_encoder_options(
                    reader,
                    &options,
                    &PngOptions::default(),
                )
                .unwrap_or_else(|error| panic!("{name} seekable PNG failed: {error}"));
                assert_eq!(actual, expected, "{name} PNG output");

                options.output = OutputFormat::Jpeg;
                let expected =
                    thumbnail_png_with_jpeg_options(input, &options, &JpegOptions::default())
                        .unwrap_or_else(|error| panic!("{name} slice JPEG failed: {error}"));
                let (read_at, _) = blob_harness(input);
                let reader = new_reader(input.len(), read_at);
                let actual = thumbnail_png_from_reader_with_jpeg_options(
                    reader,
                    &options,
                    &JpegOptions::default(),
                )
                .unwrap_or_else(|error| panic!("{name} seekable JPEG failed: {error}"));
                assert_eq!(actual, expected, "{name} JPEG output");
            }
        }

        #[wasm_bindgen_test]
        fn adam7_rgba_png_and_jpeg_outputs_match_the_slice_paths() {
            let input = adam7_rgba_fixture();
            let rgba_options = ThumbnailOptions::default();
            let expected = thumbnail_png_rgba(&input, &rgba_options).unwrap();
            let (read_at, _) = blob_harness(&input);
            let actual = run_rgba(input.len() as f64, &read_at, &rgba_options).unwrap();
            assert_eq!(actual.width, expected.dimensions.width);
            assert_eq!(actual.height, expected.dimensions.height);
            assert_eq!(actual.pixels, expected.pixels);

            let png_options = ThumbnailOptions {
                output: OutputFormat::Png,
                ..ThumbnailOptions::default()
            };
            let expected =
                thumbnail_png_with_encoder_options(&input, &png_options, &PngOptions::default())
                    .unwrap();
            let (read_at, _) = blob_harness(&input);
            let actual = thumbnail_png_from_reader_with_encoder_options(
                new_reader(input.len(), read_at),
                &png_options,
                &PngOptions::default(),
            )
            .unwrap();
            assert_eq!(actual, expected);

            let jpeg_options = ThumbnailOptions {
                output: OutputFormat::Jpeg,
                ..ThumbnailOptions::default()
            };
            let expected =
                thumbnail_png_with_jpeg_options(&input, &jpeg_options, &JpegOptions::default())
                    .unwrap();
            let (read_at, _) = blob_harness(&input);
            let actual = thumbnail_png_from_reader_with_jpeg_options(
                new_reader(input.len(), read_at),
                &jpeg_options,
                &JpegOptions::default(),
            )
            .unwrap();
            assert_eq!(actual, expected);
        }

        #[wasm_bindgen_test]
        fn direct_png_and_jpeg_writers_match_buffered_output() {
            let adam7 = adam7_rgba_fixture();
            for (name, input) in [("ordered", RGBA_FIXTURE), ("adam7", adam7.as_slice())] {
                let png_options = ThumbnailOptions {
                    output: OutputFormat::Png,
                    ..ThumbnailOptions::default()
                };
                let expected = encoded_bytes(
                    thumbnail_png_with_encoder_options(input, &png_options, &PngOptions::default())
                        .unwrap(),
                );
                let (read_at, _) = blob_harness(input);
                let writer = CapturingWriter::default();
                let captured = writer.clone();
                thumbnail_png_from_reader_to_writer_with_encoder_options_and_buffer(
                    new_reader(input.len(), read_at),
                    &png_options,
                    &PngOptions::default(),
                    OUTPUT_CHUNK_BYTES,
                    writer,
                )
                .unwrap_or_else(|error| panic!("{name} seekable PNG writer failed: {error}"));
                assert_eq!(captured.bytes(), expected, "{name} PNG writer output");

                let jpeg_options = ThumbnailOptions {
                    output: OutputFormat::Jpeg,
                    ..ThumbnailOptions::default()
                };
                let expected = encoded_bytes(
                    thumbnail_png_with_jpeg_options(input, &jpeg_options, &JpegOptions::default())
                        .unwrap(),
                );
                let (read_at, _) = blob_harness(input);
                let writer = CapturingWriter::default();
                let captured = writer.clone();
                thumbnail_jpeg_from_reader_to_writer_with_options_and_buffer(
                    new_reader(input.len(), read_at),
                    &jpeg_options,
                    &JpegOptions::default(),
                    OUTPUT_CHUNK_BYTES,
                    writer,
                )
                .unwrap_or_else(|error| panic!("{name} seekable JPEG writer failed: {error}"));
                assert_eq!(captured.bytes(), expected, "{name} JPEG writer output");
            }
        }

        #[wasm_bindgen_test]
        fn chunked_png_and_jpeg_outputs_match_buffered_output() {
            let adam7 = adam7_rgba_fixture();
            for (name, input) in [("ordered", RGBA_FIXTURE), ("adam7", adam7.as_slice())] {
                let png_options = ThumbnailOptions {
                    output: OutputFormat::Png,
                    ..ThumbnailOptions::default()
                };
                let expected = encoded_bytes(
                    thumbnail_png_with_encoder_options(input, &png_options, &PngOptions::default())
                        .unwrap(),
                );
                let chunks = Rc::new(RefCell::new(Vec::<Vec<u8>>::new()));
                let writer = ChunkCallbackWriter::new({
                    let chunks = Rc::clone(&chunks);
                    move |chunk: &[u8]| {
                        chunks.borrow_mut().push(chunk.to_vec());
                        Ok(())
                    }
                })
                .unwrap();
                let finalizer = writer.clone();
                let (read_at, _) = blob_harness(input);
                thumbnail_png_from_reader_to_writer_with_encoder_options_and_buffer(
                    new_reader(input.len(), read_at),
                    &png_options,
                    &PngOptions::default(),
                    OUTPUT_CHUNK_BYTES,
                    writer,
                )
                .unwrap_or_else(|error| panic!("{name} seekable chunked PNG failed: {error}"));
                finalizer.finish().unwrap();
                assert_eq!(chunks.borrow().concat(), expected, "{name} PNG chunks");
                assert!(
                    chunks
                        .borrow()
                        .iter()
                        .all(|chunk| chunk.len() <= OUTPUT_CHUNK_BYTES)
                );

                let jpeg_options = ThumbnailOptions {
                    output: OutputFormat::Jpeg,
                    ..ThumbnailOptions::default()
                };
                let expected = encoded_bytes(
                    thumbnail_png_with_jpeg_options(input, &jpeg_options, &JpegOptions::default())
                        .unwrap(),
                );
                let chunks = Rc::new(RefCell::new(Vec::<Vec<u8>>::new()));
                let writer = ChunkCallbackWriter::new({
                    let chunks = Rc::clone(&chunks);
                    move |chunk: &[u8]| {
                        chunks.borrow_mut().push(chunk.to_vec());
                        Ok(())
                    }
                })
                .unwrap();
                let finalizer = writer.clone();
                let (read_at, _) = blob_harness(input);
                thumbnail_jpeg_from_reader_to_writer_with_options_and_buffer(
                    new_reader(input.len(), read_at),
                    &jpeg_options,
                    &JpegOptions::default(),
                    OUTPUT_CHUNK_BYTES,
                    writer,
                )
                .unwrap_or_else(|error| panic!("{name} seekable chunked JPEG failed: {error}"));
                finalizer.finish().unwrap();
                assert_eq!(chunks.borrow().concat(), expected, "{name} JPEG chunks");
                assert!(
                    chunks
                        .borrow()
                        .iter()
                        .all(|chunk| chunk.len() <= OUTPUT_CHUNK_BYTES)
                );
            }
        }

        #[wasm_bindgen_test]
        fn output_callback_exception_is_preserved_by_identity() {
            let factory = Function::new_no_args(
                "const marker = { kind: 'write-failure' }; return { marker, onChunk() { throw marker; } };",
            );
            let harness = factory
                .call0(&JsValue::UNDEFINED)
                .unwrap()
                .unchecked_into::<Object>();
            let marker = Reflect::get(&harness, &JsValue::from_str("marker")).unwrap();
            let on_chunk = Reflect::get(&harness, &JsValue::from_str("onChunk"))
                .unwrap()
                .unchecked_into::<Function>();
            let callback_error = Rc::new(RefCell::new(None));
            let writer = ChunkCallbackWriter::new({
                let callback_error = Rc::clone(&callback_error);
                move |chunk: &[u8]| {
                    let bytes = Uint8Array::from(chunk);
                    on_chunk
                        .call1(&JsValue::UNDEFINED, bytes.as_ref())
                        .map(|_| ())
                        .map_err(|error| {
                            *callback_error.borrow_mut() = Some(error);
                            std::io::Error::other("JavaScript output callback failed")
                        })
                }
            })
            .unwrap();
            let (read_at, _) = blob_harness(RGBA_FIXTURE);
            let options = ThumbnailOptions {
                output: OutputFormat::Png,
                ..ThumbnailOptions::default()
            };
            let result = thumbnail_png_from_reader_to_writer_with_encoder_options_and_buffer(
                new_reader(RGBA_FIXTURE.len(), read_at),
                &options,
                &PngOptions::default(),
                OUTPUT_CHUNK_BYTES,
                writer,
            );
            assert!(result.is_err());
            let error = callback_error.borrow_mut().take().unwrap();
            assert!(Object::is(&error, &marker));
        }

        #[wasm_bindgen_test]
        fn adapter_supports_checked_start_current_end_and_eof() {
            let (read_at, _) = blob_harness(&[10, 20, 30, 40, 50]);
            let mut reader = new_reader(5, read_at);
            assert_eq!(reader.seek(SeekFrom::End(-2)).unwrap(), 3);
            let mut tail = [0_u8; 2];
            assert_eq!(reader.read(&mut tail).unwrap(), 2);
            assert_eq!(tail, [40, 50]);
            assert_eq!(reader.read(&mut tail).unwrap(), 0);
            assert_eq!(reader.seek(SeekFrom::Start(1)).unwrap(), 1);
            assert_eq!(reader.seek(SeekFrom::Current(2)).unwrap(), 3);
            assert!(reader.seek(SeekFrom::Current(-4)).is_err());
            assert!(reader.seek(SeekFrom::End(1)).is_err());
        }

        #[wasm_bindgen_test]
        fn input_limit_rejects_before_the_first_content_read() {
            let (read_at, js_stats) = blob_harness(RGBA_FIXTURE);
            let mut options = ThumbnailOptions::default();
            options.limits.max_input_bytes = RGBA_FIXTURE.len() as u64 - 1;
            let error = run_rgba(RGBA_FIXTURE.len() as f64, &read_at, &options).unwrap_err();
            assert!(error_message(&error).contains("InputBytes limit exceeded"));
            assert_eq!(
                Reflect::get(&js_stats, &JsValue::from_str("calls"))
                    .unwrap()
                    .as_f64(),
                Some(0.0),
            );
        }

        #[wasm_bindgen_test]
        fn callback_exception_is_returned_by_identity() {
            let factory = Function::new_no_args(
                "const marker = { kind: 'read-failure' }; return { marker, readAt() { throw marker; } };",
            );
            let harness = factory
                .call0(&JsValue::UNDEFINED)
                .unwrap()
                .unchecked_into::<Object>();
            let marker = Reflect::get(&harness, &JsValue::from_str("marker")).unwrap();
            let read_at = Reflect::get(&harness, &JsValue::from_str("readAt"))
                .unwrap()
                .unchecked_into::<Function>();
            let error = run_rgba(16.0, &read_at, &ThumbnailOptions::default()).unwrap_err();
            assert!(Object::is(&error, &marker));
        }

        #[wasm_bindgen_test]
        fn callback_return_type_and_exact_length_are_enforced() {
            let wrong_type = Function::new_with_args("offset, length", "return [offset, length];");
            let mut reader = new_reader(16, wrong_type);
            let mut output = [0_u8; 16];
            let error = reader.read(&mut output).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);

            let short = Function::new_with_args(
                "offset, length",
                "return new Uint8Array(Math.max(0, length - 1));",
            );
            let mut reader = new_reader(16, short);
            let error = reader.read(&mut output).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);

            let long =
                Function::new_with_args("offset, length", "return new Uint8Array(length + 1);");
            let mut reader = new_reader(16, long);
            let error = reader.read(&mut output).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);

            let promise = Function::new_with_args(
                "offset, length",
                "return Promise.resolve(new Uint8Array(length));",
            );
            let mut reader = new_reader(16, promise);
            let error = reader.read(&mut output).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        }

        #[wasm_bindgen_test]
        fn invalid_and_truncated_inputs_match_slice_diagnostics() {
            let truncated = &RGBA_FIXTURE[..RGBA_FIXTURE.len() - 1];
            let apng = apng_fixture();
            let malformed = truncated_ancillary_chunk_fixture();
            for (name, input) in [
                ("invalid signature", INVALID_SIGNATURE),
                ("truncated PNG", truncated),
                ("APNG", apng.as_slice()),
                ("truncated ancillary chunk", malformed.as_slice()),
            ] {
                let options = ThumbnailOptions::default();
                let expected = thumbnail_png_rgba(input, &options).unwrap_err().to_string();
                let (read_at, _) = blob_harness(input);
                let actual = run_rgba(input.len() as f64, &read_at, &options).unwrap_err();
                assert_eq!(error_message(&actual), expected, "{name} diagnostic");
            }
        }

        #[wasm_bindgen_test]
        fn invalid_input_lengths_are_rejected_before_callback_reads() {
            for input_length in [f64::NAN, f64::INFINITY, -1.0, 1.5, 9_007_199_254_740_992.0] {
                let (read_at, stats) = blob_harness(RGBA_FIXTURE);
                assert!(run_rgba(input_length, &read_at, &ThumbnailOptions::default()).is_err());
                assert_eq!(
                    Reflect::get(&stats, &JsValue::from_str("calls"))
                        .unwrap()
                        .as_f64(),
                    Some(0.0),
                );
            }
        }

        #[wasm_bindgen_test]
        fn high_entropy_png_and_jpeg_cross_multiple_output_chunks() {
            let input = high_entropy_png(256);

            let png_options = ThumbnailOptions {
                output: OutputFormat::Png,
                ..ThumbnailOptions::default()
            };
            let png_encoder_options = PngOptions {
                compression: PngCompression::NoCompression,
                ..PngOptions::default()
            };
            let expected = encoded_bytes(
                thumbnail_png_with_encoder_options(&input, &png_options, &png_encoder_options)
                    .unwrap(),
            );
            let chunks = Rc::new(RefCell::new(Vec::<Vec<u8>>::new()));
            let writer = ChunkCallbackWriter::new({
                let chunks = Rc::clone(&chunks);
                move |chunk: &[u8]| {
                    chunks.borrow_mut().push(chunk.to_vec());
                    Ok(())
                }
            })
            .unwrap();
            let finalizer = writer.clone();
            let (read_at, _) = blob_harness(&input);
            thumbnail_png_from_reader_to_writer_with_encoder_options_and_buffer(
                new_reader(input.len(), read_at),
                &png_options,
                &png_encoder_options,
                OUTPUT_CHUNK_BYTES,
                writer,
            )
            .unwrap();
            finalizer.finish().unwrap();
            assert!(chunks.borrow().len() > 1);
            assert_eq!(chunks.borrow().concat(), expected);

            let jpeg_options = ThumbnailOptions {
                output: OutputFormat::Jpeg,
                ..ThumbnailOptions::default()
            };
            let jpeg_encoder_options = JpegOptions {
                quality: 100,
                subsampling: JpegSubsampling::S444,
                ..JpegOptions::default()
            };
            let expected = encoded_bytes(
                thumbnail_png_with_jpeg_options(&input, &jpeg_options, &jpeg_encoder_options)
                    .unwrap(),
            );
            let chunks = Rc::new(RefCell::new(Vec::<Vec<u8>>::new()));
            let writer = ChunkCallbackWriter::new({
                let chunks = Rc::clone(&chunks);
                move |chunk: &[u8]| {
                    chunks.borrow_mut().push(chunk.to_vec());
                    Ok(())
                }
            })
            .unwrap();
            let finalizer = writer.clone();
            let (read_at, _) = blob_harness(&input);
            thumbnail_jpeg_from_reader_to_writer_with_options_and_buffer(
                new_reader(input.len(), read_at),
                &jpeg_options,
                &jpeg_encoder_options,
                OUTPUT_CHUNK_BYTES,
                writer,
            )
            .unwrap();
            finalizer.finish().unwrap();
            assert!(chunks.borrow().len() > 1);
            assert_eq!(chunks.borrow().concat(), expected);
        }

        #[wasm_bindgen_test]
        fn reports_seekable_read_and_runtime_measurements() {
            let input = high_entropy_png(1024);
            let (read_at, _) = blob_harness(&input);
            let callback_error = Rc::new(RefCell::new(None));
            let stats = Rc::new(RefCell::new(ReadStats::default()));
            let reader = JsSeekableReader::new(
                input.len() as u64,
                read_at,
                Rc::clone(&callback_error),
                Rc::clone(&stats),
            );
            let before = wasm_memory_bytes();
            let started = Date::now();
            let image = thumbnail_png_rgba_from_reader(reader, &ThumbnailOptions::default())
                .expect("seekable measurement decode must succeed");
            let seekable_millis = Date::now() - started;
            let after = wasm_memory_bytes();
            assert!(callback_error.borrow().is_none());
            let stats = *stats.borrow();

            let started = Date::now();
            let slice_image = thumbnail_png_rgba(&input, &ThumbnailOptions::default())
                .expect("slice measurement decode must succeed");
            let slice_millis = Date::now() - started;
            assert_eq!(image, slice_image);
            assert!(stats.read_calls > 0);
            assert!(stats.largest_read <= 8 * 1024);
            wasm_bindgen_test::console_log!(
                "BROWSER_FILE_INPUT_METRIC input={} reads={} bytes={} largest={} seeks={} seekable_ms={:.3} slice_ms={:.3} wasm_before={} wasm_after={}",
                input.len(),
                stats.read_calls,
                stats.bytes_read_total,
                stats.largest_read,
                stats.seek_calls,
                seekable_millis,
                slice_millis,
                before,
                after,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::SeekFrom;

    use super::{checked_input_length, checked_seek_position};

    #[test]
    fn checked_seek_rejects_positions_outside_the_input() {
        assert_eq!(checked_seek_position(10, 5, SeekFrom::Start(0)).unwrap(), 0);
        assert_eq!(
            checked_seek_position(10, 5, SeekFrom::Current(5)).unwrap(),
            10
        );
        assert_eq!(checked_seek_position(10, 5, SeekFrom::End(-3)).unwrap(), 7);
        assert!(checked_seek_position(10, 0, SeekFrom::Current(-1)).is_err());
        assert!(checked_seek_position(10, 10, SeekFrom::End(1)).is_err());
    }

    #[test]
    fn input_length_requires_a_non_negative_safe_integer() {
        assert_eq!(checked_input_length(42.0), Ok(42));
        assert!(checked_input_length(-1.0).is_err());
        assert!(checked_input_length(1.5).is_err());
        assert!(checked_input_length(f64::NAN).is_err());
        assert!(checked_input_length(9_007_199_254_740_992.0).is_err());
    }
}
