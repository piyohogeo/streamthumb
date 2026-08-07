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
        use std::io::{Read, Seek, SeekFrom};

        use js_sys::{Function, Object, Reflect, Uint8Array};
        use streamthumb_core::ThumbnailOptions;
        use streamthumb_png::thumbnail_png_rgba;
        use wasm_bindgen::{JsCast, JsValue};
        use wasm_bindgen_test::*;

        use super::{JsSeekableReader, ReadStats, run_rgba};
        use std::{cell::RefCell, rc::Rc};

        wasm_bindgen_test_configure!(run_in_dedicated_worker);

        const RGBA_FIXTURE: &[u8] =
            include_bytes!("../../../fuzz/corpus/thumbnail_png/pngsuite_basn6a08.png");

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
