use std::io::{BufRead, BufReader, Cursor, Read, Seek, SeekFrom, Write};

use png::{BitDepth, ColorType};
use streamthumb_core::{
    AreaDownsampler, Dimensions, InputInfo, OutputFormat, ProcessingPlan, RgbaImage,
    SparseAreaDownsampler, ThumbnailInfo, ThumbnailOptions, plan_thumbnail, plan_thumbnail_sparse,
    plan_thumbnail_sparse_to_writer_with_buffer, plan_thumbnail_to_writer_with_buffer,
    preflight_thumbnail, preflight_thumbnail_sparse,
    preflight_thumbnail_sparse_to_writer_with_buffer, preflight_thumbnail_to_writer_with_buffer,
};
use streamthumb_encode::{JpegOptions, JpegRowSink, JpegWriterRowSink};

use crate::{
    Error, PngColorMode, PngOptions, Result, ThumbnailOutput, UnsupportedFeature,
    encoder::{EncoderColor, PngRowSink, PngWriterRowSink},
};

#[derive(Clone, Copy)]
enum EncodedOutputStorage {
    Buffered,
    Writer(usize),
}

struct SeekableInput<R> {
    reader: R,
    start: u64,
    encoded_bytes: u64,
}

impl<R: BufRead + Seek> SeekableInput<R> {
    fn new(mut reader: R, options: &ThumbnailOptions) -> Result<Self> {
        let start = reader.stream_position().map_err(Error::InputIo)?;
        let end = reader.seek(SeekFrom::End(0)).map_err(Error::InputIo)?;
        let encoded_bytes = end.checked_sub(start).ok_or_else(|| {
            Error::InputIo(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "reader end precedes its starting position",
            ))
        })?;
        reader
            .seek(SeekFrom::Start(start))
            .map_err(Error::InputIo)?;
        validate_encoded_length(encoded_bytes, options)?;
        Ok(Self {
            reader,
            start,
            encoded_bytes,
        })
    }

    fn rewind(&mut self) -> Result<&mut R> {
        self.reader
            .seek(SeekFrom::Start(self.start))
            .map_err(Error::InputIo)?;
        Ok(&mut self.reader)
    }

    fn chunk_metadata(&mut self) -> Result<PngChunkMetadata> {
        let start = self.start;
        let end = start.checked_add(self.encoded_bytes).ok_or(
            streamthumb_core::Error::IntegerOverflow {
                operation: "encoded input end position",
            },
        )?;
        let reader = self.rewind()?;
        let mut signature = [0_u8; 8];
        if let Err(error) = reader.read_exact(&mut signature) {
            return map_metadata_io(error);
        }
        if signature != *b"\x89PNG\r\n\x1a\n" {
            return Err(Error::DecodeFailure("invalid PNG signature".to_owned()));
        }
        let mut metadata = PngChunkMetadata::default();
        loop {
            let position = reader.stream_position().map_err(Error::InputIo)?;
            if position.checked_add(12).is_none_or(|minimum| minimum > end) {
                return Err(Error::TruncatedInput);
            }
            let mut header = [0_u8; 8];
            if let Err(error) = reader.read_exact(&mut header) {
                return map_metadata_io(error);
            }
            let length = usize::try_from(u32::from_be_bytes([
                header[0], header[1], header[2], header[3],
            ]))
            .map_err(|_| streamthumb_core::Error::IntegerOverflow {
                operation: "PNG chunk length conversion",
            })?;
            let chunk_end = reader
                .stream_position()
                .map_err(Error::InputIo)?
                .checked_add(u64::try_from(length).map_err(|_| {
                    streamthumb_core::Error::IntegerOverflow {
                        operation: "PNG chunk length conversion",
                    }
                })?)
                .and_then(|position| position.checked_add(4));
            let Some(chunk_end) = chunk_end else {
                return Err(Error::TruncatedInput);
            };
            if chunk_end > end {
                return Err(Error::TruncatedInput);
            }
            if &header[4..] == b"tRNS" {
                metadata.raw_trns_length = Some(length);
            }
            if matches!(&header[4..], b"acTL" | b"fcTL" | b"fdAT") {
                metadata.animated = true;
            }
            if &header[4..] == b"IDAT" {
                return Ok(metadata);
            }
            reader
                .seek(SeekFrom::Start(chunk_end))
                .map_err(Error::InputIo)?;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PngChunkMetadata {
    raw_trns_length: Option<usize>,
    animated: bool,
}

fn buffered_input<R: Read + Seek>(
    reader: R,
    options: &ThumbnailOptions,
) -> Result<SeekableInput<BufReader<R>>> {
    SeekableInput::new(BufReader::new(reader), options)
}

fn map_metadata_io<T>(error: std::io::Error) -> Result<T> {
    if error.kind() == std::io::ErrorKind::UnexpectedEof {
        Err(Error::TruncatedInput)
    } else {
        Err(Error::InputIo(error))
    }
}

/// A normalized 8-bit RGBA source row.
#[derive(Clone, Copy, Debug)]
pub struct RgbaRow<'a> {
    pub y: u32,
    pub pixels: &'a [u8],
    pub plan: ProcessingPlan,
}

/// Metadata returned after all source rows have been decoded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodedPngInfo {
    pub dimensions: Dimensions,
    pub rows_decoded: u32,
    pub plan: ProcessingPlan,
}

/// A PNG color type reported by header-only thumbnail planning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PngInputColorType {
    Grayscale,
    Rgb,
    Indexed,
    GrayscaleAlpha,
    Rgba,
}

/// Header metadata validated before thumbnail execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PngInputInfo {
    pub dimensions: Dimensions,
    pub encoded_bytes: u64,
    pub color_type: PngInputColorType,
    pub bit_depth: u8,
    pub interlaced: bool,
}

/// A thumbnail plan that reports whether it fits the configured memory limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PngThumbnailPlan {
    pub input: PngInputInfo,
    pub processing: ProcessingPlan,
    pub output_format: OutputFormat,
    pub configured_max_working_memory_bytes: usize,
    pub within_memory_limit: bool,
}

/// Inspects and plans buffered PNG thumbnail processing without enforcing only
/// the configured working-memory limit.
///
/// Input and output limits, dimensions, PNG header constraints, APNG rejection,
/// and checked arithmetic remain enforced. Thumbnail execution performs its own
/// plan and continues to enforce the working-memory limit.
pub fn preflight_thumbnail_png(
    input: &[u8],
    options: &ThumbnailOptions,
) -> Result<PngThumbnailPlan> {
    let mut input = SeekableInput::new(Cursor::new(input), options)?;
    preflight_thumbnail_png_with_storage(&mut input, options, EncodedOutputStorage::Buffered)
}

/// Inspects and plans buffered PNG thumbnail processing from a seekable reader.
///
/// The reader is consumed from its current position and must support rewinding to
/// that position. Encoded input bytes are not retained by this function.
#[doc(hidden)]
pub fn preflight_thumbnail_png_from_reader<R: Read + Seek>(
    reader: R,
    options: &ThumbnailOptions,
) -> Result<PngThumbnailPlan> {
    let mut input = buffered_input(reader, options)?;
    preflight_thumbnail_png_with_storage(&mut input, options, EncodedOutputStorage::Buffered)
}

/// Plans PNG or JPEG delivery to a caller-owned writer with an adapter buffer.
///
/// Raw RGBA output has no encoded writer path and is rejected.
#[doc(hidden)]
pub fn preflight_thumbnail_png_to_writer_with_buffer(
    input: &[u8],
    options: &ThumbnailOptions,
    writer_buffer_bytes: usize,
) -> Result<PngThumbnailPlan> {
    if options.output == OutputFormat::Rgba {
        return Err(Error::InvalidOutputDelivery(
            "writer delivery requires PNG or JPEG output",
        ));
    }
    let mut input = SeekableInput::new(Cursor::new(input), options)?;
    preflight_thumbnail_png_with_storage(
        &mut input,
        options,
        EncodedOutputStorage::Writer(writer_buffer_bytes),
    )
}

/// Plans PNG or JPEG writer delivery from a seekable reader.
#[doc(hidden)]
pub fn preflight_thumbnail_png_from_reader_to_writer_with_buffer<R: Read + Seek>(
    reader: R,
    options: &ThumbnailOptions,
    writer_buffer_bytes: usize,
) -> Result<PngThumbnailPlan> {
    if options.output == OutputFormat::Rgba {
        return Err(Error::InvalidOutputDelivery(
            "writer delivery requires PNG or JPEG output",
        ));
    }
    let mut input = buffered_input(reader, options)?;
    preflight_thumbnail_png_with_storage(
        &mut input,
        options,
        EncodedOutputStorage::Writer(writer_buffer_bytes),
    )
}

fn preflight_thumbnail_png_with_storage<R: BufRead + Seek>(
    input: &mut SeekableInput<R>,
    options: &ThumbnailOptions,
    storage: EncodedOutputStorage,
) -> Result<PngThumbnailPlan> {
    let inspection = inspect_png_header(input, options)?;
    let input_info = InputInfo {
        dimensions: inspection.input.dimensions,
        encoded_bytes: inspection.input.encoded_bytes,
        source_bytes_per_pixel: inspection.source_bytes_per_pixel,
    };
    let processing = match (inspection.input.interlaced, storage) {
        (false, EncodedOutputStorage::Buffered) => preflight_thumbnail(input_info, options),
        (true, EncodedOutputStorage::Buffered) => preflight_thumbnail_sparse(input_info, options),
        (false, EncodedOutputStorage::Writer(buffer_bytes)) => {
            preflight_thumbnail_to_writer_with_buffer(input_info, options, buffer_bytes)
        }
        (true, EncodedOutputStorage::Writer(buffer_bytes)) => {
            preflight_thumbnail_sparse_to_writer_with_buffer(input_info, options, buffer_bytes)
        }
    }?;
    let configured_max_working_memory_bytes = options.limits.max_working_memory_bytes;
    Ok(PngThumbnailPlan {
        input: inspection.input,
        processing,
        output_format: options.output,
        configured_max_working_memory_bytes,
        within_memory_limit: processing.memory.total_bytes <= configured_max_working_memory_bytes,
    })
}

/// Decodes a supported PNG one row at a time and normalizes each row to RGBA8.
///
/// The callback's row slice is valid only for the duration of the call. This
/// function never allocates a full-resolution source image.
pub fn decode_png_rows<F>(
    input: &[u8],
    options: &ThumbnailOptions,
    consume_row: F,
) -> Result<DecodedPngInfo>
where
    F: FnMut(RgbaRow<'_>) -> Result<()>,
{
    let mut input = SeekableInput::new(Cursor::new(input), options)?;
    decode_png_rows_with_storage(
        &mut input,
        options,
        EncodedOutputStorage::Buffered,
        consume_row,
    )
}

/// Decodes a seekable PNG reader one row at a time and normalizes each row to RGBA8.
///
/// The reader is consumed from its current position and must support rewinding to
/// that position. Encoded input bytes are not retained by this function.
pub fn decode_png_rows_from_reader<R, F>(
    reader: R,
    options: &ThumbnailOptions,
    consume_row: F,
) -> Result<DecodedPngInfo>
where
    R: Read + Seek,
    F: FnMut(RgbaRow<'_>) -> Result<()>,
{
    let mut input = buffered_input(reader, options)?;
    decode_png_rows_with_storage(
        &mut input,
        options,
        EncodedOutputStorage::Buffered,
        consume_row,
    )
}

fn decode_png_rows_with_storage<R, F>(
    input: &mut SeekableInput<R>,
    options: &ThumbnailOptions,
    storage: EncodedOutputStorage,
    mut consume_row: F,
) -> Result<DecodedPngInfo>
where
    R: BufRead + Seek,
    F: FnMut(RgbaRow<'_>) -> Result<()>,
{
    let inspection = inspect_png_header(input, options)?;

    let decoder_limit = options.limits.max_working_memory_bytes;
    let mut decoder = png::Decoder::new_with_limits(
        input.rewind()?,
        png::Limits {
            bytes: decoder_limit,
        },
    );
    decoder.set_transformations(png::Transformations::IDENTITY);
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);

    let header = decoder
        .read_header_info()
        .map_err(|error| map_decode_error(error, decoder_limit))?;
    let dimensions = inspection.input.dimensions;
    let source_color = header.color_type;
    validate_source_color_depth(source_color, header.bit_depth)?;
    validate_header_matches_inspection(header, inspection)?;
    reject_interlacing(header.interlaced)?;
    let input_info = InputInfo {
        dimensions,
        encoded_bytes: inspection.input.encoded_bytes,
        source_bytes_per_pixel: inspection.source_bytes_per_pixel,
    };
    let plan = match storage {
        EncodedOutputStorage::Buffered => plan_thumbnail(input_info, options),
        EncodedOutputStorage::Writer(buffer_bytes) => {
            plan_thumbnail_to_writer_with_buffer(input_info, options, buffer_bytes)
        }
    }?;
    decoder.set_limits(png::Limits {
        bytes: plan
            .memory
            .decoder_rows_bytes
            .checked_add(plan.memory.decoder_staging_bytes)
            .ok_or(streamthumb_core::Error::IntegerOverflow {
                operation: "PNG decoder allowance",
            })?,
    });

    let mut reader = decoder
        .read_info()
        .map_err(|error| map_decode_error(error, decoder_limit))?;
    if reader.info().is_animated() {
        return Err(Error::Unsupported {
            feature: UnsupportedFeature::Animation,
            detail: "APNG is not supported",
        });
    }
    let (output_color, output_depth) = reader.output_color_type();
    validate_source_color_depth(output_color, output_depth)?;
    let source_format = SourceFormat::from_info(reader.info())?;
    reject_interlacing(reader.info().interlaced)?;

    let rgba_row_bytes = usize::try_from(dimensions.width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or(streamthumb_core::Error::IntegerOverflow {
            operation: "normalized RGBA row size",
        })?;
    let mut normalized_row = Vec::new();
    normalized_row
        .try_reserve_exact(rgba_row_bytes)
        .map_err(|_| Error::AllocationFailed {
            bytes: rgba_row_bytes,
        })?;
    normalized_row.resize(rgba_row_bytes, 0);
    let mut rows_decoded = 0_u32;

    while let Some(row) = reader
        .next_row()
        .map_err(|error| map_decode_error(error, decoder_limit))?
    {
        normalize_row(row.data(), &source_format, &mut normalized_row)?;
        consume_row(RgbaRow {
            y: rows_decoded,
            pixels: &normalized_row,
            plan,
        })?;
        rows_decoded =
            rows_decoded
                .checked_add(1)
                .ok_or(streamthumb_core::Error::IntegerOverflow {
                    operation: "decoded row count",
                })?;
    }

    if rows_decoded != dimensions.height {
        return Err(Error::TruncatedInput);
    }
    reader
        .finish()
        .map_err(|error| map_decode_error(error, decoder_limit))?;

    Ok(DecodedPngInfo {
        dimensions,
        rows_decoded,
        plan,
    })
}

/// Decodes and area-downsamples a supported PNG into a bounded RGBA8 image.
pub fn thumbnail_png_rgba(input: &[u8], options: &ThumbnailOptions) -> Result<RgbaImage> {
    let mut rgba_options = *options;
    rgba_options.output = OutputFormat::Rgba;
    let mut input = SeekableInput::new(Cursor::new(input), &rgba_options)?;
    thumbnail_png_rgba_planned(&mut input, &rgba_options).map(|(image, _)| image)
}

/// Decodes and area-downsamples a seekable PNG reader into bounded RGBA8 output.
pub fn thumbnail_png_rgba_from_reader<R: Read + Seek>(
    reader: R,
    options: &ThumbnailOptions,
) -> Result<RgbaImage> {
    let mut rgba_options = *options;
    rgba_options.output = OutputFormat::Rgba;
    let mut input = buffered_input(reader, &rgba_options)?;
    thumbnail_png_rgba_planned(&mut input, &rgba_options).map(|(image, _)| image)
}

/// Creates a thumbnail using the output representation selected in `options`.
pub fn thumbnail_png(input: &[u8], options: &ThumbnailOptions) -> Result<ThumbnailOutput> {
    match options.output {
        OutputFormat::Rgba => Ok(thumbnail_png_rgba(input, options)?.into()),
        OutputFormat::Png => {
            thumbnail_png_with_encoder_options(input, options, &PngOptions::default())
        }
        OutputFormat::Jpeg => {
            thumbnail_png_with_jpeg_options(input, options, &JpegOptions::default())
        }
    }
}

/// Creates a thumbnail from a seekable PNG reader.
pub fn thumbnail_png_from_reader<R: Read + Seek>(
    reader: R,
    options: &ThumbnailOptions,
) -> Result<ThumbnailOutput> {
    match options.output {
        OutputFormat::Rgba => {
            let mut input = buffered_input(reader, options)?;
            let (image, _) = thumbnail_png_rgba_planned(&mut input, options)?;
            Ok(image.into())
        }
        OutputFormat::Png => {
            thumbnail_png_from_reader_with_encoder_options(reader, options, &PngOptions::default())
        }
        OutputFormat::Jpeg => {
            thumbnail_png_from_reader_with_jpeg_options(reader, options, &JpegOptions::default())
        }
    }
}

/// Creates a thumbnail with codec-specific PNG encoder settings.
///
/// PNG settings are rejected when raw RGBA output is requested.
pub fn thumbnail_png_with_encoder_options(
    input: &[u8],
    options: &ThumbnailOptions,
    png_options: &PngOptions,
) -> Result<ThumbnailOutput> {
    let mut input = SeekableInput::new(Cursor::new(input), options)?;
    thumbnail_png_with_encoder_options_from_input(&mut input, options, png_options)
}

/// Creates a PNG thumbnail from a seekable reader with codec-specific settings.
pub fn thumbnail_png_from_reader_with_encoder_options<R: Read + Seek>(
    reader: R,
    options: &ThumbnailOptions,
    png_options: &PngOptions,
) -> Result<ThumbnailOutput> {
    let mut input = buffered_input(reader, options)?;
    thumbnail_png_with_encoder_options_from_input(&mut input, options, png_options)
}

fn thumbnail_png_with_encoder_options_from_input<R: BufRead + Seek>(
    input: &mut SeekableInput<R>,
    options: &ThumbnailOptions,
    png_options: &PngOptions,
) -> Result<ThumbnailOutput> {
    if options.output != OutputFormat::Png {
        return Err(Error::InvalidPngOptions("PNG settings require PNG output"));
    }
    let (bytes, plan) = thumbnail_png_encoded(input, options, *png_options)?;
    Ok(ThumbnailOutput::Encoded {
        bytes,
        width: plan.output.width,
        height: plan.output.height,
        mime_type: "image/png",
        format: OutputFormat::Png,
    })
}

/// Creates a JPEG thumbnail with codec-specific encoder settings.
///
/// JPEG settings are rejected unless JPEG output is requested.
pub fn thumbnail_png_with_jpeg_options(
    input: &[u8],
    options: &ThumbnailOptions,
    jpeg_options: &JpegOptions,
) -> Result<ThumbnailOutput> {
    let mut input = SeekableInput::new(Cursor::new(input), options)?;
    thumbnail_png_with_jpeg_options_from_input(&mut input, options, jpeg_options)
}

/// Creates a JPEG thumbnail from a seekable reader with codec-specific settings.
pub fn thumbnail_png_from_reader_with_jpeg_options<R: Read + Seek>(
    reader: R,
    options: &ThumbnailOptions,
    jpeg_options: &JpegOptions,
) -> Result<ThumbnailOutput> {
    let mut input = buffered_input(reader, options)?;
    thumbnail_png_with_jpeg_options_from_input(&mut input, options, jpeg_options)
}

fn thumbnail_png_with_jpeg_options_from_input<R: BufRead + Seek>(
    input: &mut SeekableInput<R>,
    options: &ThumbnailOptions,
    jpeg_options: &JpegOptions,
) -> Result<ThumbnailOutput> {
    if options.output != OutputFormat::Jpeg {
        return Err(Error::InvalidJpegOptions(
            "JPEG settings require JPEG output",
        ));
    }
    let (bytes, plan) = thumbnail_jpeg_encoded(input, options, *jpeg_options)?;
    Ok(ThumbnailOutput::Encoded {
        bytes,
        width: plan.output.width,
        height: plan.output.height,
        mime_type: "image/jpeg",
        format: OutputFormat::Jpeg,
    })
}

/// Writes an encoded PNG thumbnail directly to `writer` with default PNG settings.
pub fn thumbnail_png_to_writer<W: Write + 'static>(
    input: &[u8],
    options: &ThumbnailOptions,
    writer: W,
) -> Result<ThumbnailInfo> {
    thumbnail_png_to_writer_with_encoder_options(input, options, &PngOptions::default(), writer)
}

/// Writes a PNG thumbnail from a seekable reader with default PNG settings.
///
/// ```no_run
/// use std::fs::File;
/// use streamthumb_core::{OutputFormat, ThumbnailOptions};
/// use streamthumb_png::thumbnail_png_from_reader_to_writer;
///
/// let options = ThumbnailOptions {
///     output: OutputFormat::Png,
///     ..ThumbnailOptions::default()
/// };
/// let input = File::open("input.png")?;
/// let output = File::create("thumbnail.png")?;
/// let info = thumbnail_png_from_reader_to_writer(input, &options, output)?;
/// assert_eq!(info.format, OutputFormat::Png);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn thumbnail_png_from_reader_to_writer<R, W>(
    reader: R,
    options: &ThumbnailOptions,
    writer: W,
) -> Result<ThumbnailInfo>
where
    R: Read + Seek,
    W: Write + 'static,
{
    thumbnail_png_from_reader_to_writer_with_encoder_options(
        reader,
        options,
        &PngOptions::default(),
        writer,
    )
}

/// Writes an encoded PNG thumbnail directly to `writer`.
///
/// The encoded byte limit is enforced while bytes are forwarded, but the
/// completed encoded image is not retained in the working-memory estimate.
pub fn thumbnail_png_to_writer_with_encoder_options<W: Write + 'static>(
    input: &[u8],
    options: &ThumbnailOptions,
    png_options: &PngOptions,
    writer: W,
) -> Result<ThumbnailInfo> {
    thumbnail_png_to_writer_with_encoder_options_and_buffer(input, options, png_options, 0, writer)
}

/// Writes a PNG thumbnail from a seekable reader directly to `writer`.
pub fn thumbnail_png_from_reader_to_writer_with_encoder_options<R, W>(
    reader: R,
    options: &ThumbnailOptions,
    png_options: &PngOptions,
    writer: W,
) -> Result<ThumbnailInfo>
where
    R: Read + Seek,
    W: Write + 'static,
{
    thumbnail_png_from_reader_to_writer_with_encoder_options_and_buffer(
        reader,
        options,
        png_options,
        0,
        writer,
    )
}

/// Writes PNG output through an adapter that retains a bounded chunk buffer.
#[doc(hidden)]
pub fn thumbnail_png_to_writer_with_encoder_options_and_buffer<W: Write + 'static>(
    input: &[u8],
    options: &ThumbnailOptions,
    png_options: &PngOptions,
    writer_buffer_bytes: usize,
    writer: W,
) -> Result<ThumbnailInfo> {
    let mut input = SeekableInput::new(Cursor::new(input), options)?;
    thumbnail_png_to_writer_with_encoder_options_and_buffer_from_input(
        &mut input,
        options,
        png_options,
        writer_buffer_bytes,
        writer,
    )
}

#[doc(hidden)]
pub fn thumbnail_png_from_reader_to_writer_with_encoder_options_and_buffer<R, W>(
    reader: R,
    options: &ThumbnailOptions,
    png_options: &PngOptions,
    writer_buffer_bytes: usize,
    writer: W,
) -> Result<ThumbnailInfo>
where
    R: Read + Seek,
    W: Write + 'static,
{
    let mut input = buffered_input(reader, options)?;
    thumbnail_png_to_writer_with_encoder_options_and_buffer_from_input(
        &mut input,
        options,
        png_options,
        writer_buffer_bytes,
        writer,
    )
}

fn thumbnail_png_to_writer_with_encoder_options_and_buffer_from_input<
    R: BufRead + Seek,
    W: Write + 'static,
>(
    input: &mut SeekableInput<R>,
    options: &ThumbnailOptions,
    png_options: &PngOptions,
    writer_buffer_bytes: usize,
    writer: W,
) -> Result<ThumbnailInfo> {
    if options.output != OutputFormat::Png {
        return Err(Error::InvalidPngOptions("PNG settings require PNG output"));
    }
    let plan =
        thumbnail_png_encoded_to_writer(input, options, *png_options, writer_buffer_bytes, writer)?;
    Ok(ThumbnailInfo {
        width: plan.output.width,
        height: plan.output.height,
        format: OutputFormat::Png,
    })
}

/// Writes an encoded JPEG thumbnail directly to `writer` with default settings.
pub fn thumbnail_jpeg_to_writer<W: Write>(
    input: &[u8],
    options: &ThumbnailOptions,
    writer: W,
) -> Result<ThumbnailInfo> {
    thumbnail_jpeg_to_writer_with_options(input, options, &JpegOptions::default(), writer)
}

/// Writes a JPEG thumbnail from a seekable reader with default settings.
pub fn thumbnail_jpeg_from_reader_to_writer<R, W>(
    reader: R,
    options: &ThumbnailOptions,
    writer: W,
) -> Result<ThumbnailInfo>
where
    R: Read + Seek,
    W: Write,
{
    thumbnail_jpeg_from_reader_to_writer_with_options(
        reader,
        options,
        &JpegOptions::default(),
        writer,
    )
}

/// Writes an encoded JPEG thumbnail directly to `writer`.
///
/// The encoded byte limit is enforced while bytes are forwarded, but the
/// completed encoded image is not retained in the working-memory estimate.
pub fn thumbnail_jpeg_to_writer_with_options<W: Write>(
    input: &[u8],
    options: &ThumbnailOptions,
    jpeg_options: &JpegOptions,
    writer: W,
) -> Result<ThumbnailInfo> {
    thumbnail_jpeg_to_writer_with_options_and_buffer(input, options, jpeg_options, 0, writer)
}

/// Writes a JPEG thumbnail from a seekable reader directly to `writer`.
pub fn thumbnail_jpeg_from_reader_to_writer_with_options<R, W>(
    reader: R,
    options: &ThumbnailOptions,
    jpeg_options: &JpegOptions,
    writer: W,
) -> Result<ThumbnailInfo>
where
    R: Read + Seek,
    W: Write,
{
    thumbnail_jpeg_from_reader_to_writer_with_options_and_buffer(
        reader,
        options,
        jpeg_options,
        0,
        writer,
    )
}

/// Writes JPEG output through an adapter that retains a bounded chunk buffer.
#[doc(hidden)]
pub fn thumbnail_jpeg_to_writer_with_options_and_buffer<W: Write>(
    input: &[u8],
    options: &ThumbnailOptions,
    jpeg_options: &JpegOptions,
    writer_buffer_bytes: usize,
    writer: W,
) -> Result<ThumbnailInfo> {
    let mut input = SeekableInput::new(Cursor::new(input), options)?;
    thumbnail_jpeg_to_writer_with_options_and_buffer_from_input(
        &mut input,
        options,
        jpeg_options,
        writer_buffer_bytes,
        writer,
    )
}

#[doc(hidden)]
pub fn thumbnail_jpeg_from_reader_to_writer_with_options_and_buffer<R, W>(
    reader: R,
    options: &ThumbnailOptions,
    jpeg_options: &JpegOptions,
    writer_buffer_bytes: usize,
    writer: W,
) -> Result<ThumbnailInfo>
where
    R: Read + Seek,
    W: Write,
{
    let mut input = buffered_input(reader, options)?;
    thumbnail_jpeg_to_writer_with_options_and_buffer_from_input(
        &mut input,
        options,
        jpeg_options,
        writer_buffer_bytes,
        writer,
    )
}

fn thumbnail_jpeg_to_writer_with_options_and_buffer_from_input<R: BufRead + Seek, W: Write>(
    input: &mut SeekableInput<R>,
    options: &ThumbnailOptions,
    jpeg_options: &JpegOptions,
    writer_buffer_bytes: usize,
    writer: W,
) -> Result<ThumbnailInfo> {
    if options.output != OutputFormat::Jpeg {
        return Err(Error::InvalidJpegOptions(
            "JPEG settings require JPEG output",
        ));
    }
    let plan = thumbnail_jpeg_encoded_to_writer(
        input,
        options,
        *jpeg_options,
        writer_buffer_bytes,
        writer,
    )?;
    Ok(ThumbnailInfo {
        width: plan.output.width,
        height: plan.output.height,
        format: OutputFormat::Jpeg,
    })
}

fn thumbnail_jpeg_encoded_to_writer<R: BufRead + Seek, W: Write>(
    input: &mut SeekableInput<R>,
    options: &ThumbnailOptions,
    jpeg_options: JpegOptions,
    writer_buffer_bytes: usize,
    writer: W,
) -> Result<ProcessingPlan> {
    let inspection = inspect_png(input, options)?;
    if inspection.interlaced {
        let (downsampler, plan) = thumbnail_png_adam7_downsampler_with_storage(
            input,
            options,
            EncodedOutputStorage::Writer(writer_buffer_bytes),
        )?;
        let sink = JpegWriterRowSink::new(
            plan.output,
            plan.encoded_output_limit_bytes,
            jpeg_options,
            writer,
        )?;
        downsampler.finish_into(sink)?;
        return Ok(plan);
    }

    let mut downsampler = None;
    let mut writer = Some(writer);
    let decoded = decode_png_rows_with_storage(
        input,
        options,
        EncodedOutputStorage::Writer(writer_buffer_bytes),
        |row| {
            if downsampler.is_none() {
                let target = writer.take().ok_or_else(|| {
                    Error::DecodeFailure("failed to acquire the JPEG output writer".to_owned())
                })?;
                let sink = JpegWriterRowSink::new(
                    row.plan.output,
                    row.plan.encoded_output_limit_bytes,
                    jpeg_options,
                    target,
                )?;
                downsampler = Some(AreaDownsampler::with_sink_and_fit(
                    row.plan.source,
                    row.plan.output,
                    options.fit,
                    sink,
                )?);
            }
            downsampler
                .as_mut()
                .ok_or_else(|| {
                    Error::DecodeFailure("failed to initialize the JPEG writer sink".to_owned())
                })?
                .push_row(row.y, row.pixels)?;
            Ok(())
        },
    )?;
    downsampler.ok_or(Error::TruncatedInput)?.finish()?;
    Ok(decoded.plan)
}

fn thumbnail_png_encoded_to_writer<R: BufRead + Seek, W: Write + 'static>(
    input: &mut SeekableInput<R>,
    options: &ThumbnailOptions,
    png_options: PngOptions,
    writer_buffer_bytes: usize,
    writer: W,
) -> Result<ProcessingPlan> {
    let inspection = inspect_png(input, options)?;
    let color = resolve_encoder_color(png_options.color, inspection.auto_color);
    if inspection.interlaced {
        let (downsampler, plan) = thumbnail_png_adam7_downsampler_with_storage(
            input,
            options,
            EncodedOutputStorage::Writer(writer_buffer_bytes),
        )?;
        let sink = PngWriterRowSink::with_writer(
            plan.output,
            plan.encoded_output_limit_bytes,
            color,
            png_options,
            writer,
        )?;
        downsampler.finish_into(sink)?;
        return Ok(plan);
    }

    let mut downsampler = None;
    let mut writer = Some(writer);
    let decoded = decode_png_rows_with_storage(
        input,
        options,
        EncodedOutputStorage::Writer(writer_buffer_bytes),
        |row| {
            if downsampler.is_none() {
                let target = writer.take().ok_or_else(|| {
                    Error::DecodeFailure("failed to acquire the PNG output writer".to_owned())
                })?;
                let sink = PngWriterRowSink::with_writer(
                    row.plan.output,
                    row.plan.encoded_output_limit_bytes,
                    color,
                    png_options,
                    target,
                )?;
                downsampler = Some(AreaDownsampler::with_sink_and_fit(
                    row.plan.source,
                    row.plan.output,
                    options.fit,
                    sink,
                )?);
            }
            downsampler
                .as_mut()
                .ok_or_else(|| {
                    Error::DecodeFailure("failed to initialize the PNG writer sink".to_owned())
                })?
                .push_row(row.y, row.pixels)?;
            Ok(())
        },
    )?;
    downsampler.ok_or(Error::TruncatedInput)?.finish()?;
    Ok(decoded.plan)
}

fn thumbnail_jpeg_encoded<R: BufRead + Seek>(
    input: &mut SeekableInput<R>,
    options: &ThumbnailOptions,
    jpeg_options: JpegOptions,
) -> Result<(Vec<u8>, ProcessingPlan)> {
    let inspection = inspect_png(input, options)?;
    if inspection.interlaced {
        let (downsampler, plan) = thumbnail_png_adam7_downsampler(input, options)?;
        let sink = JpegRowSink::new(plan.output, plan.encoded_output_limit_bytes, jpeg_options)?;
        return Ok((downsampler.finish_into(sink)?, plan));
    }

    let mut downsampler: Option<AreaDownsampler<JpegRowSink>> = None;
    let decoded =
        decode_png_rows_with_storage(input, options, EncodedOutputStorage::Buffered, |row| {
            if downsampler.is_none() {
                let sink = JpegRowSink::new(
                    row.plan.output,
                    row.plan.encoded_output_limit_bytes,
                    jpeg_options,
                )?;
                downsampler = Some(AreaDownsampler::with_sink_and_fit(
                    row.plan.source,
                    row.plan.output,
                    options.fit,
                    sink,
                )?);
            }
            let active = downsampler.as_mut().ok_or_else(|| {
                Error::DecodeFailure("failed to initialize the JPEG row sink".to_owned())
            })?;
            active.push_row(row.y, row.pixels)?;
            Ok(())
        })?;
    let bytes = downsampler.ok_or(Error::TruncatedInput)?.finish()?;
    Ok((bytes, decoded.plan))
}

fn thumbnail_png_encoded<R: BufRead + Seek>(
    input: &mut SeekableInput<R>,
    options: &ThumbnailOptions,
    png_options: PngOptions,
) -> Result<(Vec<u8>, ProcessingPlan)> {
    let inspection = inspect_png(input, options)?;
    let color = resolve_encoder_color(png_options.color, inspection.auto_color);
    if inspection.interlaced {
        let (downsampler, plan) = thumbnail_png_adam7_downsampler(input, options)?;
        let sink = PngRowSink::new(
            plan.output,
            plan.encoded_output_limit_bytes,
            color,
            png_options,
        )?;
        return Ok((downsampler.finish_into(sink)?, plan));
    }

    let mut downsampler: Option<AreaDownsampler<PngRowSink>> = None;
    let decoded =
        decode_png_rows_with_storage(input, options, EncodedOutputStorage::Buffered, |row| {
            if downsampler.is_none() {
                let sink = PngRowSink::new(
                    row.plan.output,
                    row.plan.encoded_output_limit_bytes,
                    color,
                    png_options,
                )?;
                downsampler = Some(AreaDownsampler::with_sink_and_fit(
                    row.plan.source,
                    row.plan.output,
                    options.fit,
                    sink,
                )?);
            }
            let active = downsampler.as_mut().ok_or_else(|| {
                Error::DecodeFailure("failed to initialize the PNG row sink".to_owned())
            })?;
            active.push_row(row.y, row.pixels)?;
            Ok(())
        })?;
    let bytes = downsampler.ok_or(Error::TruncatedInput)?.finish()?;
    Ok((bytes, decoded.plan))
}

fn thumbnail_png_rgba_planned<R: BufRead + Seek>(
    input: &mut SeekableInput<R>,
    options: &ThumbnailOptions,
) -> Result<(RgbaImage, ProcessingPlan)> {
    if png_is_interlaced(input, options)? {
        return thumbnail_png_adam7_rgba_planned(input, options);
    }

    let mut downsampler = None;
    let decoded =
        decode_png_rows_with_storage(input, options, EncodedOutputStorage::Buffered, |row| {
            if downsampler.is_none() {
                downsampler = Some(AreaDownsampler::new_with_fit(
                    row.plan.source,
                    row.plan.output,
                    options.fit,
                )?);
            }
            let active = downsampler.as_mut().ok_or_else(|| {
                Error::DecodeFailure("failed to initialize the area downsampler".to_owned())
            })?;
            active.push_row(row.y, row.pixels)?;
            Ok(())
        })?;

    let image = downsampler
        .ok_or(Error::TruncatedInput)?
        .finish()
        .map_err(Error::from)?;
    Ok((image, decoded.plan))
}

fn png_is_interlaced<R: BufRead + Seek>(
    input: &mut SeekableInput<R>,
    options: &ThumbnailOptions,
) -> Result<bool> {
    inspect_png_header(input, options).map(|inspection| inspection.input.interlaced)
}

#[derive(Clone, Copy)]
struct PngInspection {
    interlaced: bool,
    auto_color: EncoderColor,
}

#[derive(Clone, Copy)]
struct PngHeaderInspection {
    input: PngInputInfo,
    source_bytes_per_pixel: u8,
}

fn inspect_png_header<R: BufRead + Seek>(
    input: &mut SeekableInput<R>,
    options: &ThumbnailOptions,
) -> Result<PngHeaderInspection> {
    let metadata = input.chunk_metadata()?;
    reject_animation(metadata.animated)?;
    let encoded_bytes = input.encoded_bytes;
    let decoder_limit = options.limits.max_working_memory_bytes.max(64 * 1024);
    let mut decoder = png::Decoder::new_with_limits(
        input.rewind()?,
        png::Limits {
            bytes: decoder_limit,
        },
    );
    decoder.set_transformations(png::Transformations::IDENTITY);
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);
    let header = decoder
        .read_header_info()
        .map_err(|error| map_decode_error(error, decoder_limit))?;
    let dimensions = Dimensions::new(header.width, header.height)?;
    validate_source_color_depth(header.color_type, header.bit_depth)?;
    validate_direct_trns_length(metadata.raw_trns_length, header.color_type)?;
    let source_bytes_per_pixel = planning_bytes_per_pixel(header.color_type, header.bit_depth)?;
    Ok(PngHeaderInspection {
        input: PngInputInfo {
            dimensions,
            encoded_bytes,
            color_type: png_input_color_type(header.color_type),
            bit_depth: bit_depth_value(header.bit_depth),
            interlaced: header.interlaced,
        },
        source_bytes_per_pixel,
    })
}

fn validate_header_matches_inspection(
    header: &png::Info<'_>,
    inspection: PngHeaderInspection,
) -> Result<()> {
    if header.width != inspection.input.dimensions.width
        || header.height != inspection.input.dimensions.height
        || header.interlaced != inspection.input.interlaced
        || png_input_color_type(header.color_type) != inspection.input.color_type
        || bit_depth_value(header.bit_depth) != inspection.input.bit_depth
    {
        return Err(Error::DecodeFailure(
            "PNG header changed between inspection and decoding".to_owned(),
        ));
    }
    Ok(())
}

fn reject_animation(animated: bool) -> Result<()> {
    if animated {
        return Err(Error::Unsupported {
            feature: UnsupportedFeature::Animation,
            detail: "APNG is not supported",
        });
    }
    Ok(())
}

const fn png_input_color_type(color: ColorType) -> PngInputColorType {
    match color {
        ColorType::Grayscale => PngInputColorType::Grayscale,
        ColorType::Rgb => PngInputColorType::Rgb,
        ColorType::Indexed => PngInputColorType::Indexed,
        ColorType::GrayscaleAlpha => PngInputColorType::GrayscaleAlpha,
        ColorType::Rgba => PngInputColorType::Rgba,
    }
}

const fn bit_depth_value(depth: BitDepth) -> u8 {
    match depth {
        BitDepth::One => 1,
        BitDepth::Two => 2,
        BitDepth::Four => 4,
        BitDepth::Eight => 8,
        BitDepth::Sixteen => 16,
    }
}

fn inspect_png<R: BufRead + Seek>(
    input: &mut SeekableInput<R>,
    options: &ThumbnailOptions,
) -> Result<PngInspection> {
    let inspection = inspect_png_header(input, options)?;
    let decoder_limit = options.limits.max_working_memory_bytes;
    let mut decoder = png::Decoder::new_with_limits(
        input.rewind()?,
        png::Limits {
            bytes: decoder_limit,
        },
    );
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);
    let reader = decoder
        .read_info()
        .map_err(|error| map_decode_error(error, decoder_limit))?;
    let header = reader.info();
    validate_source_color_depth(header.color_type, header.bit_depth)?;
    validate_header_matches_inspection(header, inspection)?;
    reject_animation(reader.info().is_animated())?;
    Ok(PngInspection {
        interlaced: inspection.input.interlaced,
        auto_color: auto_encoder_color(header),
    })
}

const fn resolve_encoder_color(requested: PngColorMode, automatic: EncoderColor) -> EncoderColor {
    match requested {
        PngColorMode::Auto => automatic,
        PngColorMode::Rgba8 => EncoderColor::Rgba8,
        PngColorMode::Rgb8 => EncoderColor::Rgb8,
        PngColorMode::GrayscaleAlpha8 => EncoderColor::GrayscaleAlpha8,
        PngColorMode::Grayscale8 => EncoderColor::Grayscale8,
    }
}

fn auto_encoder_color(info: &png::Info<'_>) -> EncoderColor {
    let transparency = info.trns.as_deref();
    match info.color_type {
        ColorType::Grayscale => {
            if transparency.is_some() {
                EncoderColor::GrayscaleAlpha8
            } else {
                EncoderColor::Grayscale8
            }
        }
        ColorType::GrayscaleAlpha => EncoderColor::GrayscaleAlpha8,
        ColorType::Rgb => {
            if transparency.is_some() {
                EncoderColor::Rgba8
            } else {
                EncoderColor::Rgb8
            }
        }
        ColorType::Rgba => EncoderColor::Rgba8,
        ColorType::Indexed => {
            let grayscale = info.palette.as_deref().is_some_and(|palette| {
                !palette.is_empty()
                    && palette.len() % 3 == 0
                    && palette
                        .chunks_exact(3)
                        .all(|color| color[0] == color[1] && color[1] == color[2])
            });
            let has_transparency =
                transparency.is_some_and(|alpha| alpha.iter().any(|value| *value != 255));
            match (grayscale, has_transparency) {
                (true, false) => EncoderColor::Grayscale8,
                (true, true) => EncoderColor::GrayscaleAlpha8,
                (false, false) => EncoderColor::Rgb8,
                (false, true) => EncoderColor::Rgba8,
            }
        }
    }
}

fn thumbnail_png_adam7_rgba_planned<R: BufRead + Seek>(
    input: &mut SeekableInput<R>,
    options: &ThumbnailOptions,
) -> Result<(RgbaImage, ProcessingPlan)> {
    let (downsampler, plan) = thumbnail_png_adam7_downsampler(input, options)?;
    Ok((downsampler.finish()?, plan))
}

fn thumbnail_png_adam7_downsampler<R: BufRead + Seek>(
    input: &mut SeekableInput<R>,
    options: &ThumbnailOptions,
) -> Result<(SparseAreaDownsampler, ProcessingPlan)> {
    thumbnail_png_adam7_downsampler_with_storage(input, options, EncodedOutputStorage::Buffered)
}

fn thumbnail_png_adam7_downsampler_with_storage<R: BufRead + Seek>(
    input: &mut SeekableInput<R>,
    options: &ThumbnailOptions,
    storage: EncodedOutputStorage,
) -> Result<(SparseAreaDownsampler, ProcessingPlan)> {
    let inspection = inspect_png_header(input, options)?;
    let decoder_limit = options.limits.max_working_memory_bytes;
    let mut decoder = png::Decoder::new_with_limits(
        input.rewind()?,
        png::Limits {
            bytes: decoder_limit,
        },
    );
    decoder.set_transformations(png::Transformations::IDENTITY);
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);

    let header = decoder
        .read_header_info()
        .map_err(|error| map_decode_error(error, decoder_limit))?;
    let dimensions = inspection.input.dimensions;
    let source_color = header.color_type;
    validate_source_color_depth(source_color, header.bit_depth)?;
    validate_header_matches_inspection(header, inspection)?;
    if !header.interlaced {
        return Err(Error::DecodeFailure(
            "Adam7 path received a non-interlaced PNG".to_owned(),
        ));
    }
    let input_info = InputInfo {
        dimensions,
        encoded_bytes: inspection.input.encoded_bytes,
        source_bytes_per_pixel: inspection.source_bytes_per_pixel,
    };
    let plan = match storage {
        EncodedOutputStorage::Buffered => plan_thumbnail_sparse(input_info, options),
        EncodedOutputStorage::Writer(buffer_bytes) => {
            plan_thumbnail_sparse_to_writer_with_buffer(input_info, options, buffer_bytes)
        }
    }?;
    decoder.set_limits(png::Limits {
        bytes: plan
            .memory
            .decoder_rows_bytes
            .checked_add(plan.memory.decoder_staging_bytes)
            .ok_or(streamthumb_core::Error::IntegerOverflow {
                operation: "Adam7 PNG decoder allowance",
            })?,
    });

    let mut reader = decoder
        .read_info()
        .map_err(|error| map_decode_error(error, decoder_limit))?;
    if reader.info().is_animated() {
        return Err(Error::Unsupported {
            feature: UnsupportedFeature::Animation,
            detail: "APNG is not supported",
        });
    }
    let (output_color, output_depth) = reader.output_color_type();
    validate_source_color_depth(output_color, output_depth)?;
    let source_format = SourceFormat::from_info(reader.info())?;
    if !reader.info().interlaced {
        return Err(Error::DecodeFailure(
            "PNG interlace mode changed after header validation".to_owned(),
        ));
    }

    let mut downsampler =
        SparseAreaDownsampler::new_with_fit(plan.source, plan.output, options.fit)?;
    for pass in ADAM7_PASSES {
        let samples = pass_sample_count(dimensions.width, pass.x_offset, pass.x_stride);
        let lines = pass_sample_count(dimensions.height, pass.y_offset, pass.y_stride);
        if samples == 0 || lines == 0 {
            continue;
        }
        for line in 0..lines {
            let row = reader
                .next_interlaced_row()
                .map_err(|error| map_decode_error(error, decoder_limit))?
                .ok_or(Error::TruncatedInput)?;
            if !matches!(row.interlace(), png::InterlaceInfo::Adam7(_)) {
                return Err(Error::DecodeFailure(
                    "PNG decoder returned a non-Adam7 row for interlaced input".to_owned(),
                ));
            }
            let sample_count =
                usize::try_from(samples).map_err(|_| streamthumb_core::Error::IntegerOverflow {
                    operation: "Adam7 pass sample count conversion",
                })?;
            let expected_bytes = source_format.row_bytes(sample_count)?;
            if row.data().len() != expected_bytes {
                return Err(Error::DecodeFailure(format!(
                    "Adam7 pass row has {} bytes; expected {expected_bytes}",
                    row.data().len()
                )));
            }
            let y = pass
                .y_offset
                .checked_add(line.checked_mul(pass.y_stride).ok_or(
                    streamthumb_core::Error::IntegerOverflow {
                        operation: "Adam7 source y coordinate",
                    },
                )?)
                .ok_or(streamthumb_core::Error::IntegerOverflow {
                    operation: "Adam7 source y coordinate",
                })?;
            for sample in 0..samples {
                let x = pass
                    .x_offset
                    .checked_add(sample.checked_mul(pass.x_stride).ok_or(
                        streamthumb_core::Error::IntegerOverflow {
                            operation: "Adam7 source x coordinate",
                        },
                    )?)
                    .ok_or(streamthumb_core::Error::IntegerOverflow {
                        operation: "Adam7 source x coordinate",
                    })?;
                downsampler.push_pixel(
                    x,
                    y,
                    source_format.pixel(
                        row.data(),
                        usize::try_from(sample).map_err(|_| {
                            streamthumb_core::Error::IntegerOverflow {
                                operation: "Adam7 sample index conversion",
                            }
                        })?,
                    )?,
                )?;
            }
        }
    }

    if reader
        .next_interlaced_row()
        .map_err(|error| map_decode_error(error, decoder_limit))?
        .is_some()
    {
        return Err(Error::DecodeFailure(
            "PNG decoder returned more Adam7 rows than expected".to_owned(),
        ));
    }
    reader
        .finish()
        .map_err(|error| map_decode_error(error, decoder_limit))?;
    Ok((downsampler, plan))
}

#[derive(Clone, Copy)]
struct Adam7Pass {
    x_stride: u32,
    x_offset: u32,
    y_stride: u32,
    y_offset: u32,
}

const ADAM7_PASSES: [Adam7Pass; 7] = [
    Adam7Pass {
        x_stride: 8,
        x_offset: 0,
        y_stride: 8,
        y_offset: 0,
    },
    Adam7Pass {
        x_stride: 8,
        x_offset: 4,
        y_stride: 8,
        y_offset: 0,
    },
    Adam7Pass {
        x_stride: 4,
        x_offset: 0,
        y_stride: 8,
        y_offset: 4,
    },
    Adam7Pass {
        x_stride: 4,
        x_offset: 2,
        y_stride: 4,
        y_offset: 0,
    },
    Adam7Pass {
        x_stride: 2,
        x_offset: 0,
        y_stride: 4,
        y_offset: 2,
    },
    Adam7Pass {
        x_stride: 2,
        x_offset: 1,
        y_stride: 2,
        y_offset: 0,
    },
    Adam7Pass {
        x_stride: 1,
        x_offset: 0,
        y_stride: 2,
        y_offset: 1,
    },
];

fn pass_sample_count(length: u32, offset: u32, stride: u32) -> u32 {
    length.saturating_sub(offset).div_ceil(stride)
}

fn validate_encoded_length(encoded_bytes: u64, options: &ThumbnailOptions) -> Result<()> {
    if encoded_bytes > options.limits.max_input_bytes {
        return Err(streamthumb_core::Error::LimitExceeded {
            kind: streamthumb_core::LimitKind::InputBytes,
            actual: encoded_bytes,
            limit: options.limits.max_input_bytes,
        }
        .into());
    }
    Ok(())
}

fn validate_direct_trns_length(length: Option<usize>, color: ColorType) -> Result<()> {
    let expected = match color {
        ColorType::Grayscale => 2_usize,
        ColorType::Rgb => 6_usize,
        ColorType::GrayscaleAlpha | ColorType::Rgba | ColorType::Indexed => return Ok(()),
    };
    if let Some(length) = length {
        if length != expected {
            return Err(Error::DecodeFailure(format!(
                "direct-color tRNS has {length} bytes; expected {expected}"
            )));
        }
    }
    Ok(())
}

fn validate_source_color_depth(color: ColorType, depth: BitDepth) -> Result<()> {
    let valid_depth = match color {
        ColorType::Grayscale => matches!(
            depth,
            BitDepth::One | BitDepth::Two | BitDepth::Four | BitDepth::Eight | BitDepth::Sixteen
        ),
        ColorType::Indexed => matches!(
            depth,
            BitDepth::One | BitDepth::Two | BitDepth::Four | BitDepth::Eight
        ),
        ColorType::GrayscaleAlpha | ColorType::Rgb | ColorType::Rgba => {
            matches!(depth, BitDepth::Eight | BitDepth::Sixteen)
        }
    };
    if !valid_depth {
        return Err(Error::Unsupported {
            feature: UnsupportedFeature::BitDepth,
            detail: "grayscale supports 1, 2, 4, 8, or 16 bits; alpha and truecolor formats support 8 or 16 bits; palette indices support 1, 2, 4, or 8 bits",
        });
    }
    Ok(())
}

fn reject_interlacing(interlaced: bool) -> Result<()> {
    if interlaced {
        return Err(Error::Unsupported {
            feature: UnsupportedFeature::Interlacing,
            detail: "row callbacks require non-interlaced input; use thumbnail_png for Adam7",
        });
    }
    Ok(())
}

fn planning_bytes_per_pixel(color: ColorType, depth: BitDepth) -> Result<u8> {
    if color == ColorType::Indexed
        || (color == ColorType::Grayscale
            && matches!(depth, BitDepth::One | BitDepth::Two | BitDepth::Four))
    {
        Ok(1)
    } else {
        direct_bytes_per_pixel(color, depth)
    }
}

fn direct_channel_count(color: ColorType) -> Result<u8> {
    match color {
        ColorType::Grayscale => Ok(1),
        ColorType::GrayscaleAlpha => Ok(2),
        ColorType::Rgb => Ok(3),
        ColorType::Rgba => Ok(4),
        ColorType::Indexed => Err(Error::Unsupported {
            feature: UnsupportedFeature::ColorType,
            detail: "palette pixels do not have a direct channel count",
        }),
    }
}

fn direct_bytes_per_pixel(color: ColorType, depth: BitDepth) -> Result<u8> {
    let sample_bytes = match depth {
        BitDepth::Eight => 1,
        BitDepth::Sixteen => 2,
        BitDepth::One | BitDepth::Two | BitDepth::Four => {
            return Err(Error::Unsupported {
                feature: UnsupportedFeature::BitDepth,
                detail: "sub-byte direct samples are not supported",
            });
        }
    };
    direct_channel_count(color)?
        .checked_mul(sample_bytes)
        .ok_or_else(|| {
            streamthumb_core::Error::IntegerOverflow {
                operation: "direct source bytes per pixel",
            }
            .into()
        })
}

enum SourceFormat {
    Grayscale {
        bits: usize,
        transparent: Option<u16>,
    },
    Direct {
        color: ColorType,
        depth: BitDepth,
        bytes: usize,
        transparent_rgb: Option<[u16; 3]>,
    },
    Indexed {
        colors: Vec<[u8; 4]>,
        bits: usize,
    },
}

impl SourceFormat {
    fn from_info(info: &png::Info<'_>) -> Result<Self> {
        if info.color_type == ColorType::Grayscale {
            let bits = grayscale_depth_bits(info.bit_depth);
            let transparent = match info.trns.as_deref() {
                None => None,
                Some(bytes) if bits < 16 && bytes.len() == 1 => {
                    let value = u16::from(bytes[0]);
                    let maximum = grayscale_sample_maximum(bits)?;
                    if u32::from(value) > maximum {
                        return Err(Error::DecodeFailure(format!(
                            "grayscale tRNS sample {value} exceeds the {bits}-bit range"
                        )));
                    }
                    Some(value)
                }
                Some(bytes) if bits == 16 && bytes.len() == 2 => {
                    Some(u16::from_be_bytes([bytes[0], bytes[1]]))
                }
                Some(bytes) => {
                    return Err(Error::DecodeFailure(format!(
                        "normalized grayscale tRNS has {} bytes; expected {}",
                        bytes.len(),
                        if bits == 16 { 2 } else { 1 }
                    )));
                }
            };
            return Ok(Self::Grayscale { bits, transparent });
        }
        if info.color_type != ColorType::Indexed {
            return Ok(Self::Direct {
                color: info.color_type,
                depth: info.bit_depth,
                bytes: usize::from(direct_bytes_per_pixel(info.color_type, info.bit_depth)?),
                transparent_rgb: parse_rgb_transparency(info)?,
            });
        }

        let palette = info.palette.as_deref().ok_or_else(|| {
            Error::DecodeFailure("indexed PNG is missing its PLTE chunk".to_owned())
        })?;
        if palette.is_empty() || palette.len() % 3 != 0 {
            return Err(Error::DecodeFailure(
                "indexed PNG has an invalid PLTE length".to_owned(),
            ));
        }
        let entries = palette.len() / 3;
        let bits = bit_depth_bits(info.bit_depth)?;
        let capacity = 1_usize
            .checked_shl(u32::try_from(bits).map_err(|_| {
                streamthumb_core::Error::IntegerOverflow {
                    operation: "palette bit depth conversion",
                }
            })?)
            .ok_or(streamthumb_core::Error::IntegerOverflow {
                operation: "palette capacity",
            })?;
        if entries > capacity {
            return Err(Error::DecodeFailure(format!(
                "indexed PNG has {entries} palette entries but its bit depth permits {capacity}"
            )));
        }
        let transparency = match info.trns.as_deref() {
            Some(transparency) => transparency,
            None => &[],
        };
        if transparency.len() > entries {
            return Err(Error::DecodeFailure(format!(
                "indexed PNG has {} transparency entries for {entries} palette entries",
                transparency.len()
            )));
        }
        let allocation_bytes =
            entries
                .checked_mul(4)
                .ok_or(streamthumb_core::Error::IntegerOverflow {
                    operation: "palette lookup allocation",
                })?;
        let mut colors = Vec::new();
        colors
            .try_reserve_exact(entries)
            .map_err(|_| Error::AllocationFailed {
                bytes: allocation_bytes,
            })?;
        for (index, rgb) in palette.chunks_exact(3).enumerate() {
            let alpha = match transparency.get(index) {
                Some(alpha) => *alpha,
                None => u8::MAX,
            };
            colors.push([rgb[0], rgb[1], rgb[2], alpha]);
        }
        Ok(Self::Indexed { colors, bits })
    }

    fn row_bytes(&self, samples: usize) -> Result<usize> {
        match self {
            Self::Grayscale { bits, .. } => grayscale_row_bytes(samples, *bits),
            Self::Direct { bytes, .. } => samples.checked_mul(*bytes).ok_or_else(|| {
                streamthumb_core::Error::IntegerOverflow {
                    operation: "decoded PNG row length",
                }
                .into()
            }),
            Self::Indexed { bits, .. } => samples
                .checked_mul(*bits)
                .and_then(|value| value.checked_add(7))
                .map(|value| value / 8)
                .ok_or_else(|| {
                    streamthumb_core::Error::IntegerOverflow {
                        operation: "packed palette row length",
                    }
                    .into()
                }),
        }
    }

    fn pixel(&self, source: &[u8], index: usize) -> Result<[u8; 4]> {
        match self {
            Self::Grayscale { bits, transparent } => {
                normalize_grayscale_pixel(source, index, *bits, *transparent)
            }
            Self::Direct {
                color,
                depth,
                bytes,
                transparent_rgb,
            } => {
                let start =
                    index
                        .checked_mul(*bytes)
                        .ok_or(streamthumb_core::Error::IntegerOverflow {
                            operation: "direct sample offset",
                        })?;
                let end =
                    start
                        .checked_add(*bytes)
                        .ok_or(streamthumb_core::Error::IntegerOverflow {
                            operation: "direct sample end",
                        })?;
                let sample = source.get(start..end).ok_or_else(|| {
                    Error::DecodeFailure("decoded PNG row ended within a sample".to_owned())
                })?;
                normalize_direct_pixel(sample, *color, *depth, *transparent_rgb)
            }
            Self::Indexed { colors, bits } => {
                let bit_offset =
                    index
                        .checked_mul(*bits)
                        .ok_or(streamthumb_core::Error::IntegerOverflow {
                            operation: "palette sample bit offset",
                        })?;
                let byte = *source.get(bit_offset / 8).ok_or_else(|| {
                    Error::DecodeFailure("packed palette row ended within an index".to_owned())
                })?;
                let shift = 8 - *bits - bit_offset % 8;
                let mask = (1_u16 << *bits) - 1;
                let palette_index = usize::from((u16::from(byte >> shift)) & mask);
                colors.get(palette_index).copied().ok_or_else(|| {
                    Error::DecodeFailure(format!(
                        "palette index {palette_index} exceeds {} entries",
                        colors.len()
                    ))
                })
            }
        }
    }
}

fn parse_rgb_transparency(info: &png::Info<'_>) -> Result<Option<[u16; 3]>> {
    if info.color_type != ColorType::Rgb {
        return Ok(None);
    }
    let Some(bytes) = info.trns.as_deref() else {
        return Ok(None);
    };
    match info.bit_depth {
        BitDepth::Eight if bytes.len() == 3 => Ok(Some([
            u16::from(bytes[0]),
            u16::from(bytes[1]),
            u16::from(bytes[2]),
        ])),
        BitDepth::Sixteen if bytes.len() == 6 => Ok(Some([
            u16::from_be_bytes([bytes[0], bytes[1]]),
            u16::from_be_bytes([bytes[2], bytes[3]]),
            u16::from_be_bytes([bytes[4], bytes[5]]),
        ])),
        BitDepth::Eight | BitDepth::Sixteen => Err(Error::DecodeFailure(format!(
            "normalized RGB tRNS has {} bytes; expected {}",
            bytes.len(),
            if info.bit_depth == BitDepth::Sixteen {
                6
            } else {
                3
            }
        ))),
        BitDepth::One | BitDepth::Two | BitDepth::Four => Err(Error::Unsupported {
            feature: UnsupportedFeature::BitDepth,
            detail: "sub-byte RGB samples are not supported",
        }),
    }
}

fn grayscale_depth_bits(depth: BitDepth) -> usize {
    match depth {
        BitDepth::One => 1,
        BitDepth::Two => 2,
        BitDepth::Four => 4,
        BitDepth::Eight => 8,
        BitDepth::Sixteen => 16,
    }
}

fn grayscale_sample_maximum(bits: usize) -> Result<u32> {
    1_u32
        .checked_shl(
            u32::try_from(bits).map_err(|_| streamthumb_core::Error::IntegerOverflow {
                operation: "grayscale bit depth conversion",
            })?,
        )
        .and_then(|value| value.checked_sub(1))
        .ok_or_else(|| {
            streamthumb_core::Error::IntegerOverflow {
                operation: "grayscale sample maximum",
            }
            .into()
        })
}

fn grayscale_row_bytes(samples: usize, bits: usize) -> Result<usize> {
    samples
        .checked_mul(bits)
        .and_then(|value| value.checked_add(7))
        .map(|value| value / 8)
        .ok_or_else(|| {
            streamthumb_core::Error::IntegerOverflow {
                operation: "packed grayscale row length",
            }
            .into()
        })
}

fn normalize_grayscale_pixel(
    source: &[u8],
    index: usize,
    bits: usize,
    transparent: Option<u16>,
) -> Result<[u8; 4]> {
    let raw = match bits {
        1 | 2 | 4 => {
            let bit_offset =
                index
                    .checked_mul(bits)
                    .ok_or(streamthumb_core::Error::IntegerOverflow {
                        operation: "grayscale sample bit offset",
                    })?;
            let byte = *source.get(bit_offset / 8).ok_or_else(|| {
                Error::DecodeFailure("packed grayscale row ended within a sample".to_owned())
            })?;
            let shift = 8 - bits - bit_offset % 8;
            let mask = (1_u16 << bits) - 1;
            u16::from(byte >> shift) & mask
        }
        8 => u16::from(*source.get(index).ok_or_else(|| {
            Error::DecodeFailure("grayscale row ended within a sample".to_owned())
        })?),
        16 => {
            let start = index
                .checked_mul(2)
                .ok_or(streamthumb_core::Error::IntegerOverflow {
                    operation: "16-bit grayscale sample offset",
                })?;
            let end = start
                .checked_add(2)
                .ok_or(streamthumb_core::Error::IntegerOverflow {
                    operation: "16-bit grayscale sample end",
                })?;
            let bytes = source.get(start..end).ok_or_else(|| {
                Error::DecodeFailure("16-bit grayscale row ended within a sample".to_owned())
            })?;
            u16::from_be_bytes([bytes[0], bytes[1]])
        }
        _ => {
            return Err(Error::Unsupported {
                feature: UnsupportedFeature::BitDepth,
                detail: "unsupported grayscale bit depth",
            });
        }
    };
    let maximum = grayscale_sample_maximum(bits)?;
    let scaled = (u32::from(raw) * 255 + maximum / 2) / maximum;
    let gray = u8::try_from(scaled).map_err(|_| streamthumb_core::Error::IntegerOverflow {
        operation: "grayscale sample normalization",
    })?;
    let alpha = if transparent == Some(raw) { 0 } else { u8::MAX };
    Ok([gray, gray, gray, alpha])
}

fn bit_depth_bits(depth: BitDepth) -> Result<usize> {
    match depth {
        BitDepth::One => Ok(1),
        BitDepth::Two => Ok(2),
        BitDepth::Four => Ok(4),
        BitDepth::Eight => Ok(8),
        BitDepth::Sixteen => Err(Error::Unsupported {
            feature: UnsupportedFeature::BitDepth,
            detail: "16-bit palette indices are not supported",
        }),
    }
}

fn normalize_row(source: &[u8], format: &SourceFormat, destination: &mut [u8]) -> Result<()> {
    let width =
        destination
            .len()
            .checked_div(4)
            .ok_or(streamthumb_core::Error::IntegerOverflow {
                operation: "normalized PNG row width",
            })?;
    let expected_source_len = format.row_bytes(width)?;
    if source.len() != expected_source_len {
        return Err(Error::DecodeFailure(format!(
            "decoded row has {} bytes; expected {expected_source_len}",
            source.len()
        )));
    }
    for (index, rgba) in destination.chunks_exact_mut(4).enumerate() {
        rgba.copy_from_slice(&format.pixel(source, index)?);
    }
    Ok(())
}

fn normalize_direct_pixel(
    source: &[u8],
    color: ColorType,
    depth: BitDepth,
    transparent_rgb: Option<[u16; 3]>,
) -> Result<[u8; 4]> {
    let channels = usize::from(direct_channel_count(color)?);
    let sample_bytes = match depth {
        BitDepth::Eight => 1,
        BitDepth::Sixteen => 2,
        BitDepth::One | BitDepth::Two | BitDepth::Four => {
            return Err(Error::Unsupported {
                feature: UnsupportedFeature::BitDepth,
                detail: "sub-byte direct samples are not supported",
            });
        }
    };
    let expected =
        channels
            .checked_mul(sample_bytes)
            .ok_or(streamthumb_core::Error::IntegerOverflow {
                operation: "direct sample length",
            })?;
    if source.len() != expected {
        return Err(Error::DecodeFailure(format!(
            "decoded PNG sample has {} bytes; expected {expected}",
            source.len()
        )));
    }

    let raw_channel = |index: usize| -> Result<u16> {
        let start =
            index
                .checked_mul(sample_bytes)
                .ok_or(streamthumb_core::Error::IntegerOverflow {
                    operation: "direct channel offset",
                })?;
        match depth {
            BitDepth::Eight => source.get(start).copied().map(u16::from).ok_or_else(|| {
                Error::DecodeFailure("decoded PNG sample ended within a channel".to_owned())
            }),
            BitDepth::Sixteen => {
                let end = start
                    .checked_add(2)
                    .ok_or(streamthumb_core::Error::IntegerOverflow {
                        operation: "16-bit channel end",
                    })?;
                let bytes = source.get(start..end).ok_or_else(|| {
                    Error::DecodeFailure("decoded PNG sample ended within a channel".to_owned())
                })?;
                Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
            }
            BitDepth::One | BitDepth::Two | BitDepth::Four => Err(Error::Unsupported {
                feature: UnsupportedFeature::BitDepth,
                detail: "sub-byte direct samples are not supported",
            }),
        }
    };
    let normalize = |value: u16| -> Result<u8> {
        let scaled = match depth {
            BitDepth::Eight => u32::from(value),
            BitDepth::Sixteen => (u32::from(value) * 255 + 32_767) / 65_535,
            BitDepth::One | BitDepth::Two | BitDepth::Four => {
                return Err(Error::Unsupported {
                    feature: UnsupportedFeature::BitDepth,
                    detail: "sub-byte direct samples are not supported",
                });
            }
        };
        u8::try_from(scaled).map_err(|_| {
            streamthumb_core::Error::IntegerOverflow {
                operation: "direct sample normalization",
            }
            .into()
        })
    };

    match color {
        ColorType::Grayscale => {
            let gray = normalize(raw_channel(0)?)?;
            Ok([gray, gray, gray, u8::MAX])
        }
        ColorType::GrayscaleAlpha => {
            let gray = normalize(raw_channel(0)?)?;
            Ok([gray, gray, gray, normalize(raw_channel(1)?)?])
        }
        ColorType::Rgb => {
            let raw = [raw_channel(0)?, raw_channel(1)?, raw_channel(2)?];
            let alpha = if transparent_rgb == Some(raw) {
                0
            } else {
                u8::MAX
            };
            Ok([
                normalize(raw[0])?,
                normalize(raw[1])?,
                normalize(raw[2])?,
                alpha,
            ])
        }
        ColorType::Rgba => Ok([
            normalize(raw_channel(0)?)?,
            normalize(raw_channel(1)?)?,
            normalize(raw_channel(2)?)?,
            normalize(raw_channel(3)?)?,
        ]),
        ColorType::Indexed => Err(Error::DecodeFailure(
            "palette sample reached the direct normalizer".to_owned(),
        )),
    }
}

fn map_decode_error(error: png::DecodingError, decoder_limit: usize) -> Error {
    match error {
        png::DecodingError::IoError(io_error)
            if io_error.kind() == std::io::ErrorKind::UnexpectedEof =>
        {
            Error::TruncatedInput
        }
        png::DecodingError::IoError(io_error) => Error::InputIo(io_error),
        png::DecodingError::LimitsExceeded => Error::DecoderMemoryLimitExceeded {
            limit: decoder_limit,
        },
        other => Error::DecodeFailure(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{JpegSubsampling, PngCompression, PngFilter};
    use flate2::{Compression, write::ZlibEncoder};
    use png::Filter;
    use std::error::Error as _;
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};
    use streamthumb_core::{Error as CoreError, Fit, LimitKind};

    #[derive(Clone, Default)]
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    #[derive(Default)]
    struct ReaderStats {
        reads: usize,
        max_read: usize,
        seeks: usize,
    }

    struct ChunkedReader {
        inner: Cursor<Vec<u8>>,
        chunk_bytes: usize,
        stats: Arc<Mutex<ReaderStats>>,
    }

    impl ChunkedReader {
        fn new(bytes: Vec<u8>, chunk_bytes: usize) -> (Self, Arc<Mutex<ReaderStats>>) {
            let stats = Arc::new(Mutex::new(ReaderStats::default()));
            (
                Self {
                    inner: Cursor::new(bytes),
                    chunk_bytes,
                    stats: Arc::clone(&stats),
                },
                stats,
            )
        }
    }

    impl Read for ChunkedReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let limit = buffer.len().min(self.chunk_bytes);
            let read = self.inner.read(&mut buffer[..limit])?;
            let mut stats = self.stats.lock().unwrap();
            stats.reads += usize::from(read != 0);
            stats.max_read = stats.max_read.max(read);
            Ok(read)
        }
    }

    impl Seek for ChunkedReader {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.stats.lock().unwrap().seeks += 1;
            self.inner.seek(position)
        }
    }

    struct ReadFailure {
        position: u64,
        length: u64,
    }

    impl Read for ReadFailure {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "injected read failure",
            ))
        }
    }

    impl Seek for ReadFailure {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.position = match position {
                SeekFrom::Start(position) => position,
                SeekFrom::End(offset) => self.length.checked_add_signed(offset).unwrap(),
                SeekFrom::Current(offset) => self.position.checked_add_signed(offset).unwrap(),
            };
            Ok(self.position)
        }
    }

    impl SharedWriter {
        fn bytes(&self) -> Vec<u8> {
            self.0.lock().unwrap().clone()
        }
    }

    impl Write for SharedWriter {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("injected writer failure"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn encode_png(
        width: u32,
        height: u32,
        color: ColorType,
        depth: BitDepth,
        filter: Filter,
        pixels: &[u8],
    ) -> Vec<u8> {
        let mut encoded = Vec::new();
        let mut encoder = png::Encoder::new(&mut encoded, width, height);
        encoder.set_color(color);
        encoder.set_depth(depth);
        encoder.set_filter(filter);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(pixels).unwrap();
        writer.finish().unwrap();
        encoded
    }

    fn decode_raw_png(encoded: &[u8]) -> (ColorType, Vec<u8>) {
        let mut reader = png::Decoder::new(Cursor::new(encoded)).read_info().unwrap();
        let color = reader.output_color_type().0;
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        pixels.truncate(info.buffer_size());
        (color, pixels)
    }

    fn decode_raw_jpeg(encoded: &[u8]) -> (u16, u16, Vec<u8>) {
        let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(encoded));
        let pixels = decoder.decode().unwrap();
        let info = decoder.info().unwrap();
        assert_eq!(info.pixel_format, jpeg_decoder::PixelFormat::RGB24);
        (info.width, info.height, pixels)
    }

    fn encoded_bytes(output: ThumbnailOutput) -> Vec<u8> {
        match output {
            ThumbnailOutput::Encoded { bytes, .. } => bytes,
            ThumbnailOutput::Rgba { .. } => panic!("expected encoded PNG output"),
        }
    }

    fn test_depth_bits(depth: BitDepth) -> usize {
        match depth {
            BitDepth::One => 1,
            BitDepth::Two => 2,
            BitDepth::Four => 4,
            BitDepth::Eight => 8,
            BitDepth::Sixteen => panic!("test palette helper does not support 16-bit indices"),
        }
    }

    fn pack_palette_row(indices: &[u8], depth: BitDepth) -> Vec<u8> {
        let bits = test_depth_bits(depth);
        let mut packed = vec![0; (indices.len() * bits).div_ceil(8)];
        let mask = u8::try_from((1_u16 << bits) - 1).unwrap();
        for (index, value) in indices.iter().copied().enumerate() {
            let bit_offset = index * bits;
            let shift = 8 - bits - bit_offset % 8;
            packed[bit_offset / 8] |= (value & mask) << shift;
        }
        packed
    }

    fn pack_grayscale_row(samples: &[u16], depth: BitDepth) -> Vec<u8> {
        match depth {
            BitDepth::One | BitDepth::Two | BitDepth::Four => {
                let samples = samples
                    .iter()
                    .map(|sample| u8::try_from(*sample).unwrap())
                    .collect::<Vec<_>>();
                pack_palette_row(&samples, depth)
            }
            BitDepth::Eight => samples
                .iter()
                .map(|sample| u8::try_from(*sample).unwrap())
                .collect(),
            BitDepth::Sixteen => samples
                .iter()
                .flat_map(|sample| sample.to_be_bytes())
                .collect(),
        }
    }

    fn encode_grayscale_png(
        width: u32,
        height: u32,
        depth: BitDepth,
        samples: &[u16],
        transparency: Option<u16>,
    ) -> Vec<u8> {
        let width_usize = usize::try_from(width).unwrap();
        let mut packed = Vec::new();
        for row in samples.chunks_exact(width_usize) {
            packed.extend_from_slice(&pack_grayscale_row(row, depth));
        }
        assert_eq!(
            samples.len(),
            width_usize * usize::try_from(height).unwrap()
        );

        let mut encoded = Vec::new();
        let mut encoder = png::Encoder::new(&mut encoded, width, height);
        encoder.set_color(ColorType::Grayscale);
        encoder.set_depth(depth);
        if let Some(transparency) = transparency {
            encoder.set_trns(transparency.to_be_bytes().to_vec());
        }
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&packed).unwrap();
        writer.finish().unwrap();
        encoded
    }

    fn encode_adam7_grayscale_png(
        width: u32,
        height: u32,
        depth: BitDepth,
        samples: &[u16],
        transparency: Option<u16>,
    ) -> Vec<u8> {
        let mut filtered = Vec::new();
        for pass in ADAM7_PASSES {
            let pass_samples = pass_sample_count(width, pass.x_offset, pass.x_stride);
            let lines = pass_sample_count(height, pass.y_offset, pass.y_stride);
            if pass_samples == 0 || lines == 0 {
                continue;
            }
            for line in 0..lines {
                filtered.push(0);
                let y = pass.y_offset + line * pass.y_stride;
                let mut row = Vec::new();
                for sample in 0..pass_samples {
                    let x = pass.x_offset + sample * pass.x_stride;
                    let offset =
                        usize::try_from(u64::from(y) * u64::from(width) + u64::from(x)).unwrap();
                    row.push(samples[offset]);
                }
                filtered.extend_from_slice(&pack_grayscale_row(&row, depth));
            }
        }

        let mut compressor = ZlibEncoder::new(Vec::new(), Compression::default());
        compressor.write_all(&filtered).unwrap();
        let compressed = compressor.finish().unwrap();
        let mut encoded = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.push(match depth {
            BitDepth::One => 1,
            BitDepth::Two => 2,
            BitDepth::Four => 4,
            BitDepth::Eight => 8,
            BitDepth::Sixteen => 16,
        });
        ihdr.push(0);
        ihdr.extend_from_slice(&[0, 0, 1]);
        append_chunk(&mut encoded, *b"IHDR", &ihdr);
        if let Some(transparency) = transparency {
            append_chunk(&mut encoded, *b"tRNS", &transparency.to_be_bytes());
        }
        append_chunk(&mut encoded, *b"IDAT", &compressed);
        append_chunk(&mut encoded, *b"IEND", &[]);
        encoded
    }

    fn encode_palette_png(
        width: u32,
        height: u32,
        depth: BitDepth,
        indices: &[u8],
        palette: &[u8],
        transparency: &[u8],
    ) -> Vec<u8> {
        let width_usize = usize::try_from(width).unwrap();
        let mut packed = Vec::new();
        for row in indices.chunks_exact(width_usize) {
            packed.extend_from_slice(&pack_palette_row(row, depth));
        }
        assert_eq!(
            indices.len(),
            width_usize * usize::try_from(height).unwrap()
        );

        let mut encoded = Vec::new();
        let mut encoder = png::Encoder::new(&mut encoded, width, height);
        encoder.set_color(ColorType::Indexed);
        encoder.set_depth(depth);
        encoder.set_palette(palette.to_vec());
        if !transparency.is_empty() {
            encoder.set_trns(transparency.to_vec());
        }
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&packed).unwrap();
        writer.finish().unwrap();
        encoded
    }

    fn encode_adam7_png(width: u32, height: u32, color: ColorType, pixels: &[u8]) -> Vec<u8> {
        encode_adam7_direct_png(width, height, color, BitDepth::Eight, pixels)
    }

    fn encode_adam7_direct_png(
        width: u32,
        height: u32,
        color: ColorType,
        depth: BitDepth,
        pixels: &[u8],
    ) -> Vec<u8> {
        encode_adam7_direct_png_with_trns(width, height, color, depth, pixels, &[])
    }

    fn encode_adam7_direct_png_with_trns(
        width: u32,
        height: u32,
        color: ColorType,
        depth: BitDepth,
        pixels: &[u8],
        transparency: &[u8],
    ) -> Vec<u8> {
        let channels = match color {
            ColorType::Grayscale => 1_usize,
            ColorType::GrayscaleAlpha => 2_usize,
            ColorType::Rgb => 3_usize,
            ColorType::Rgba => 4_usize,
            _ => panic!("test helper does not support this color type"),
        };
        let sample_bytes = channels
            * match depth {
                BitDepth::Eight => 1,
                BitDepth::Sixteen => 2,
                _ => panic!("test direct helper supports only 8- and 16-bit samples"),
            };
        let mut filtered = Vec::new();
        for pass in ADAM7_PASSES {
            let samples = pass_sample_count(width, pass.x_offset, pass.x_stride);
            let lines = pass_sample_count(height, pass.y_offset, pass.y_stride);
            if samples == 0 || lines == 0 {
                continue;
            }
            for line in 0..lines {
                filtered.push(0);
                let y = pass.y_offset + line * pass.y_stride;
                for sample in 0..samples {
                    let x = pass.x_offset + sample * pass.x_stride;
                    let offset = usize::try_from(
                        (u64::from(y) * u64::from(width) + u64::from(x))
                            * u64::try_from(sample_bytes).unwrap(),
                    )
                    .unwrap();
                    filtered.extend_from_slice(&pixels[offset..offset + sample_bytes]);
                }
            }
        }

        let mut compressor = ZlibEncoder::new(Vec::new(), Compression::default());
        compressor.write_all(&filtered).unwrap();
        let compressed = compressor.finish().unwrap();
        let mut encoded = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.push(match depth {
            BitDepth::Eight => 8,
            BitDepth::Sixteen => 16,
            _ => unreachable!(),
        });
        ihdr.push(match color {
            ColorType::Grayscale => 0,
            ColorType::Rgb => 2,
            ColorType::GrayscaleAlpha => 4,
            ColorType::Rgba => 6,
            _ => unreachable!(),
        });
        ihdr.extend_from_slice(&[0, 0, 1]);
        append_chunk(&mut encoded, *b"IHDR", &ihdr);
        if !transparency.is_empty() {
            append_chunk(&mut encoded, *b"tRNS", transparency);
        }
        append_chunk(&mut encoded, *b"IDAT", &compressed);
        append_chunk(&mut encoded, *b"IEND", &[]);
        encoded
    }

    fn encode_adam7_palette_png(
        width: u32,
        height: u32,
        depth: BitDepth,
        indices: &[u8],
        palette: &[u8],
        transparency: &[u8],
    ) -> Vec<u8> {
        let mut filtered = Vec::new();
        for pass in ADAM7_PASSES {
            let samples = pass_sample_count(width, pass.x_offset, pass.x_stride);
            let lines = pass_sample_count(height, pass.y_offset, pass.y_stride);
            if samples == 0 || lines == 0 {
                continue;
            }
            for line in 0..lines {
                filtered.push(0);
                let y = pass.y_offset + line * pass.y_stride;
                let mut pass_indices = Vec::new();
                for sample in 0..samples {
                    let x = pass.x_offset + sample * pass.x_stride;
                    let offset =
                        usize::try_from(u64::from(y) * u64::from(width) + u64::from(x)).unwrap();
                    pass_indices.push(indices[offset]);
                }
                filtered.extend_from_slice(&pack_palette_row(&pass_indices, depth));
            }
        }

        let mut compressor = ZlibEncoder::new(Vec::new(), Compression::default());
        compressor.write_all(&filtered).unwrap();
        let compressed = compressor.finish().unwrap();
        let mut encoded = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.push(u8::try_from(test_depth_bits(depth)).unwrap());
        ihdr.push(3);
        ihdr.extend_from_slice(&[0, 0, 1]);
        append_chunk(&mut encoded, *b"IHDR", &ihdr);
        append_chunk(&mut encoded, *b"PLTE", palette);
        if !transparency.is_empty() {
            append_chunk(&mut encoded, *b"tRNS", transparency);
        }
        append_chunk(&mut encoded, *b"IDAT", &compressed);
        append_chunk(&mut encoded, *b"IEND", &[]);
        encoded
    }

    fn append_chunk(png: &mut Vec<u8>, chunk_type: [u8; 4], data: &[u8]) {
        png.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
        png.extend_from_slice(&chunk_type);
        png.extend_from_slice(data);
        let start = png.len() - data.len() - 4;
        png.extend_from_slice(&crc32(&png[start..]).to_be_bytes());
    }

    fn default_options() -> ThumbnailOptions {
        ThumbnailOptions::default()
    }

    #[test]
    fn streams_rgb_rows_in_order_and_normalizes_alpha() {
        let pixels = [
            1, 2, 3, 4, 5, 6, // row 0
            7, 8, 9, 10, 11, 12, // row 1
            13, 14, 15, 16, 17, 18, // row 2
        ];
        let encoded = encode_png(
            2,
            3,
            ColorType::Rgb,
            BitDepth::Eight,
            Filter::Paeth,
            &pixels,
        );
        let mut rows = Vec::new();

        let info = decode_png_rows(&encoded, &default_options(), |row| {
            rows.push((row.y, row.pixels.to_vec()));
            Ok(())
        })
        .unwrap();

        assert_eq!(info.dimensions, Dimensions::new(2, 3).unwrap());
        assert_eq!(info.rows_decoded, 3);
        assert_eq!(rows[0], (0, vec![1, 2, 3, 255, 4, 5, 6, 255]));
        assert_eq!(rows[1], (1, vec![7, 8, 9, 255, 10, 11, 12, 255]));
        assert_eq!(rows[2], (2, vec![13, 14, 15, 255, 16, 17, 18, 255]));
    }

    #[test]
    fn applies_eight_bit_rgb_transparency() {
        let mut encoded = Vec::new();
        let mut encoder = png::Encoder::new(&mut encoded, 2, 1);
        encoder.set_color(ColorType::Rgb);
        encoder.set_depth(BitDepth::Eight);
        encoder.set_trns(vec![0, 10, 0, 20, 0, 30]);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[10, 20, 30, 10, 20, 31]).unwrap();
        writer.finish().unwrap();
        let mut rgba = Vec::new();

        decode_png_rows(&encoded, &default_options(), |row| {
            rgba.extend_from_slice(row.pixels);
            Ok(())
        })
        .unwrap();

        assert_eq!(rgba, [10, 20, 30, 0, 10, 20, 31, 255]);
    }

    #[test]
    fn compares_sixteen_bit_rgb_transparency_before_normalization() {
        let transparent = [0x01, 0x00, 0x02, 0x00, 0x03, 0x00];
        let pixels = [
            0x01, 0x00, 0x02, 0x00, 0x03, 0x00, // transparent
            0x01, 0x01, 0x02, 0x00, 0x03, 0x00, // same RGBA8 color, distinct source red
        ];
        let mut encoded = Vec::new();
        let mut encoder = png::Encoder::new(&mut encoded, 2, 1);
        encoder.set_color(ColorType::Rgb);
        encoder.set_depth(BitDepth::Sixteen);
        encoder.set_trns(transparent.to_vec());
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&pixels).unwrap();
        writer.finish().unwrap();
        let mut rgba = Vec::new();

        decode_png_rows(&encoded, &default_options(), |row| {
            rgba.extend_from_slice(row.pixels);
            Ok(())
        })
        .unwrap();

        assert_eq!(rgba, [1, 2, 3, 0, 1, 2, 3, 255]);
    }

    #[test]
    fn rejects_invalid_rgb_transparency_length_before_rows() {
        for transparency in [vec![0; 5], vec![0; 7]] {
            let mut encoded = Vec::new();
            let mut encoder = png::Encoder::new(&mut encoded, 1, 1);
            encoder.set_color(ColorType::Rgb);
            encoder.set_depth(BitDepth::Eight);
            encoder.set_trns(transparency);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[1, 2, 3]).unwrap();
            writer.finish().unwrap();
            let mut callbacks = 0;

            assert!(
                decode_png_rows(&encoded, &default_options(), |_| {
                    callbacks += 1;
                    Ok(())
                })
                .is_err()
            );
            assert_eq!(callbacks, 0);
        }
    }

    #[test]
    fn streams_grayscale_rows_and_preserves_grayscale_alpha() {
        let grayscale = encode_png(
            3,
            1,
            ColorType::Grayscale,
            BitDepth::Eight,
            Filter::Sub,
            &[7, 80, 201],
        );
        let mut grayscale_rgba = Vec::new();
        decode_png_rows(&grayscale, &default_options(), |row| {
            grayscale_rgba.extend_from_slice(row.pixels);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            grayscale_rgba,
            [7, 7, 7, 255, 80, 80, 80, 255, 201, 201, 201, 255]
        );

        let grayscale_alpha = encode_png(
            2,
            1,
            ColorType::GrayscaleAlpha,
            BitDepth::Eight,
            Filter::Paeth,
            &[31, 0, 190, 117],
        );
        let mut grayscale_alpha_rgba = Vec::new();
        decode_png_rows(&grayscale_alpha, &default_options(), |row| {
            grayscale_alpha_rgba.extend_from_slice(row.pixels);
            Ok(())
        })
        .unwrap();
        assert_eq!(grayscale_alpha_rgba, [31, 31, 31, 0, 190, 190, 190, 117]);
    }

    #[test]
    fn applies_separate_grayscale_transparency() {
        let mut encoded = Vec::new();
        let mut encoder = png::Encoder::new(&mut encoded, 2, 1);
        encoder.set_color(ColorType::Grayscale);
        encoder.set_depth(BitDepth::Eight);
        encoder.set_trns(vec![0, 10]);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[10, 11]).unwrap();
        writer.finish().unwrap();
        let mut rgba = Vec::new();

        decode_png_rows(&encoded, &default_options(), |row| {
            rgba.extend_from_slice(row.pixels);
            Ok(())
        })
        .unwrap();

        assert_eq!(rgba, [10, 10, 10, 0, 11, 11, 11, 255]);
    }

    #[test]
    fn expands_packed_grayscale_depths_with_exact_scaling() {
        let cases = [
            (
                BitDepth::One,
                vec![0, 1],
                1,
                vec![0, 0, 0, 255, 255, 255, 255, 0],
            ),
            (
                BitDepth::Two,
                vec![0, 1, 2, 3],
                2,
                vec![
                    0, 0, 0, 255, 85, 85, 85, 255, 170, 170, 170, 0, 255, 255, 255, 255,
                ],
            ),
            (
                BitDepth::Four,
                vec![0, 7, 8, 15],
                7,
                vec![
                    0, 0, 0, 255, 119, 119, 119, 0, 136, 136, 136, 255, 255, 255, 255, 255,
                ],
            ),
        ];

        for (depth, samples, transparent, expected) in cases {
            let encoded = encode_grayscale_png(
                u32::try_from(samples.len()).unwrap(),
                1,
                depth,
                &samples,
                Some(transparent),
            );
            let mut actual = Vec::new();
            let info = decode_png_rows(&encoded, &default_options(), |row| {
                actual.extend_from_slice(row.pixels);
                Ok(())
            })
            .unwrap();

            assert_eq!(actual, expected, "packed grayscale mismatch at {depth:?}");
            assert_eq!(info.plan.memory.decoder_rows_bytes, (samples.len() + 1) * 3);
        }
    }

    #[test]
    fn applies_sixteen_bit_grayscale_transparency_before_scaling() {
        let encoded =
            encode_grayscale_png(3, 1, BitDepth::Sixteen, &[0, 32_768, 65_535], Some(32_768));
        let mut rgba = Vec::new();
        decode_png_rows(&encoded, &default_options(), |row| {
            rgba.extend_from_slice(row.pixels);
            Ok(())
        })
        .unwrap();
        assert_eq!(rgba, [0, 0, 0, 255, 128, 128, 128, 0, 255, 255, 255, 255]);
    }

    #[test]
    fn rejects_invalid_grayscale_transparency_before_rows() {
        let mut malformed_length = Vec::new();
        let mut encoder = png::Encoder::new(&mut malformed_length, 1, 1);
        encoder.set_color(ColorType::Grayscale);
        encoder.set_depth(BitDepth::Eight);
        encoder.set_trns(vec![7]);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[7]).unwrap();
        writer.finish().unwrap();
        let out_of_range = encode_grayscale_png(1, 1, BitDepth::Two, &[0], Some(4));

        for encoded in [malformed_length, out_of_range] {
            let mut callbacks = 0;
            assert!(
                decode_png_rows(&encoded, &default_options(), |_| {
                    callbacks += 1;
                    Ok(())
                })
                .is_err()
            );
            assert_eq!(callbacks, 0);
        }
    }

    #[test]
    fn expands_palette_rows_with_transparency() {
        let palette = [
            255, 0, 0, // red
            0, 255, 0, // green
            0, 0, 255, // blue
            240, 240, 240, // light gray
        ];
        let encoded =
            encode_palette_png(4, 1, BitDepth::Two, &[0, 1, 2, 3], &palette, &[0, 64, 128]);
        let mut rgba = Vec::new();

        decode_png_rows(&encoded, &default_options(), |row| {
            rgba.extend_from_slice(row.pixels);
            Ok(())
        })
        .unwrap();

        assert_eq!(
            rgba,
            [
                255, 0, 0, 0, 0, 255, 0, 64, 0, 0, 255, 128, 240, 240, 240, 255,
            ]
        );
    }

    #[test]
    fn rejects_palette_indices_outside_plte_for_sequential_and_adam7() {
        let palette = [12, 34, 56];
        let sequential = encode_palette_png(1, 1, BitDepth::Two, &[3], &palette, &[]);
        let adam7 = encode_adam7_palette_png(1, 1, BitDepth::Two, &[3], &palette, &[]);
        let mut callbacks = 0;

        assert!(
            decode_png_rows(&sequential, &default_options(), |_| {
                callbacks += 1;
                Ok(())
            })
            .is_err()
        );
        assert_eq!(callbacks, 0);
        assert!(thumbnail_png_rgba(&adam7, &default_options()).is_err());
    }

    #[test]
    fn rejects_invalid_palette_and_transparency_lengths_before_rows() {
        let too_many_colors =
            encode_palette_png(1, 1, BitDepth::One, &[0], &[1, 2, 3, 4, 5, 6, 7, 8, 9], &[]);
        let too_much_transparency =
            encode_palette_png(1, 1, BitDepth::One, &[0], &[1, 2, 3], &[0, 128]);

        for encoded in [too_many_colors, too_much_transparency] {
            let mut callbacks = 0;
            assert!(
                decode_png_rows(&encoded, &default_options(), |_| {
                    callbacks += 1;
                    Ok(())
                })
                .is_err()
            );
            assert_eq!(callbacks, 0);
        }
    }

    #[test]
    fn adam7_rgb_and_rgba_match_non_interlaced_thumbnails() {
        for color in [ColorType::Rgb, ColorType::Rgba] {
            let sample_bytes = if color == ColorType::Rgb { 3 } else { 4 };
            let width = 11_u32;
            let height = 9_u32;
            let mut pixels = Vec::new();
            for y in 0..height {
                for x in 0..width {
                    pixels.extend_from_slice(&[
                        u8::try_from((x * 31 + y * 7) % 256).unwrap(),
                        u8::try_from((x * 11 + y * 43) % 256).unwrap(),
                        u8::try_from((x * 53 + y * 3) % 256).unwrap(),
                    ]);
                    if sample_bytes == 4 {
                        pixels.push(u8::try_from((x * 29 + y * 47) % 256).unwrap());
                    }
                }
            }
            let adam7 = encode_adam7_png(width, height, color, &pixels);
            let sequential = encode_png(
                width,
                height,
                color,
                BitDepth::Eight,
                Filter::Paeth,
                &pixels,
            );
            let mut options = default_options();
            options.max_width = 5;
            options.max_height = 4;

            assert_eq!(
                thumbnail_png_rgba(&adam7, &options).unwrap(),
                thumbnail_png_rgba(&sequential, &options).unwrap()
            );
        }
    }

    #[test]
    fn adam7_rgb_transparency_matches_non_interlaced_for_both_depths() {
        for depth in [BitDepth::Eight, BitDepth::Sixteen] {
            let width = 11_u32;
            let height = 9_u32;
            let modulus = if depth == BitDepth::Eight {
                256
            } else {
                65_536
            };
            let mut pixels = Vec::new();
            for y in 0..height {
                for x in 0..width {
                    for channel in 0..3_u32 {
                        let value =
                            u16::try_from((17 + x * 7_919 + y * 4_099 + channel * 41) % modulus)
                                .unwrap();
                        if depth == BitDepth::Eight {
                            pixels.push(u8::try_from(value).unwrap());
                        } else {
                            pixels.extend_from_slice(&value.to_be_bytes());
                        }
                    }
                }
            }
            let transparency = [17_u16, 58, 99]
                .into_iter()
                .flat_map(u16::to_be_bytes)
                .collect::<Vec<_>>();
            let mut sequential = Vec::new();
            let mut encoder = png::Encoder::new(&mut sequential, width, height);
            encoder.set_color(ColorType::Rgb);
            encoder.set_depth(depth);
            encoder.set_trns(transparency.clone());
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&pixels).unwrap();
            writer.finish().unwrap();
            let adam7 = encode_adam7_direct_png_with_trns(
                width,
                height,
                ColorType::Rgb,
                depth,
                &pixels,
                &transparency,
            );
            let mut options = default_options();
            options.max_width = 5;
            options.max_height = 4;

            assert_eq!(
                thumbnail_png_rgba(&adam7, &options).unwrap(),
                thumbnail_png_rgba(&sequential, &options).unwrap(),
                "RGB tRNS Adam7 mismatch at {depth:?}"
            );
        }
    }

    #[test]
    fn adam7_grayscale_formats_match_non_interlaced_thumbnails() {
        for color in [ColorType::Grayscale, ColorType::GrayscaleAlpha] {
            let sample_bytes = if color == ColorType::Grayscale { 1 } else { 2 };
            let width = 13_u32;
            let height = 10_u32;
            let mut pixels = Vec::new();
            for y in 0..height {
                for x in 0..width {
                    pixels.push(u8::try_from((x * 37 + y * 19) % 256).unwrap());
                    if sample_bytes == 2 {
                        pixels.push(u8::try_from((x * 13 + y * 41) % 256).unwrap());
                    }
                }
            }
            let adam7 = encode_adam7_png(width, height, color, &pixels);
            let sequential = encode_png(
                width,
                height,
                color,
                BitDepth::Eight,
                Filter::Paeth,
                &pixels,
            );
            let mut options = default_options();
            options.max_width = 6;
            options.max_height = 5;

            assert_eq!(
                thumbnail_png_rgba(&adam7, &options).unwrap(),
                thumbnail_png_rgba(&sequential, &options).unwrap()
            );
        }
    }

    #[test]
    fn adam7_grayscale_depths_and_transparency_match_non_interlaced() {
        for depth in [
            BitDepth::One,
            BitDepth::Two,
            BitDepth::Four,
            BitDepth::Eight,
            BitDepth::Sixteen,
        ] {
            let bits = grayscale_depth_bits(depth);
            let maximum = u16::try_from(grayscale_sample_maximum(bits).unwrap()).unwrap();
            let transparent = maximum / 2;
            let width = 13_u32;
            let height = 10_u32;
            let samples = (0..width * height)
                .map(|index| {
                    u16::try_from((u32::from(maximum) + 1 + index * 17) % (u32::from(maximum) + 1))
                        .unwrap()
                })
                .collect::<Vec<_>>();
            let sequential =
                encode_grayscale_png(width, height, depth, &samples, Some(transparent));
            let adam7 =
                encode_adam7_grayscale_png(width, height, depth, &samples, Some(transparent));
            let mut options = default_options();
            options.max_width = 6;
            options.max_height = 5;

            assert_eq!(
                thumbnail_png_rgba(&adam7, &options).unwrap(),
                thumbnail_png_rgba(&sequential, &options).unwrap(),
                "grayscale Adam7 mismatch at {depth:?}"
            );
        }
    }

    #[test]
    fn adam7_sixteen_bit_formats_match_non_interlaced_thumbnails() {
        for (color, channels) in [
            (ColorType::Grayscale, 1_u32),
            (ColorType::GrayscaleAlpha, 2_u32),
            (ColorType::Rgb, 3_u32),
            (ColorType::Rgba, 4_u32),
        ] {
            let width = 11_u32;
            let height = 9_u32;
            let mut pixels = Vec::new();
            for y in 0..height {
                for x in 0..width {
                    for channel in 0..channels {
                        let value =
                            u16::try_from((x * 7_919 + y * 4_099 + channel * 16_381) % 65_536)
                                .unwrap();
                        pixels.extend_from_slice(&value.to_be_bytes());
                    }
                }
            }
            let sequential = encode_png(
                width,
                height,
                color,
                BitDepth::Sixteen,
                Filter::Paeth,
                &pixels,
            );
            let adam7 = encode_adam7_direct_png(width, height, color, BitDepth::Sixteen, &pixels);
            let mut options = default_options();
            options.max_width = 5;
            options.max_height = 4;

            assert_eq!(
                thumbnail_png_rgba(&adam7, &options).unwrap(),
                thumbnail_png_rgba(&sequential, &options).unwrap(),
                "16-bit Adam7 mismatch for {color:?}"
            );
        }
    }

    #[test]
    fn adam7_palette_depths_match_non_interlaced_thumbnails() {
        for depth in [
            BitDepth::One,
            BitDepth::Two,
            BitDepth::Four,
            BitDepth::Eight,
        ] {
            let capacity = 1_usize << test_depth_bits(depth);
            let entries = capacity.min(17);
            let mut palette = Vec::new();
            let mut transparency = Vec::new();
            for index in 0..entries {
                let value = u8::try_from(index).unwrap();
                palette.extend_from_slice(&[
                    value.wrapping_mul(31),
                    value.wrapping_mul(67),
                    value.wrapping_mul(113),
                ]);
                if index + 1 < entries {
                    transparency.push(value.wrapping_mul(47));
                }
            }
            let width = 13_u32;
            let height = 10_u32;
            let indices = (0..width * height)
                .map(|index| u8::try_from(index as usize % entries).unwrap())
                .collect::<Vec<_>>();
            let sequential =
                encode_palette_png(width, height, depth, &indices, &palette, &transparency);
            let adam7 =
                encode_adam7_palette_png(width, height, depth, &indices, &palette, &transparency);
            let mut options = default_options();
            options.max_width = 6;
            options.max_height = 5;

            assert_eq!(
                thumbnail_png_rgba(&adam7, &options).unwrap(),
                thumbnail_png_rgba(&sequential, &options).unwrap(),
                "palette mismatch at {depth:?}"
            );
        }
    }

    #[test]
    fn adam7_handles_small_dimensions_and_empty_passes() {
        for height in 1..=9_u32 {
            for width in 1..=9_u32 {
                let pixels = (0..width * height)
                    .flat_map(|index| {
                        let value = u8::try_from(index % 251).unwrap();
                        [value, value.wrapping_add(1), value.wrapping_add(2), 255]
                    })
                    .collect::<Vec<_>>();
                let adam7 = encode_adam7_png(width, height, ColorType::Rgba, &pixels);
                let sequential = encode_png(
                    width,
                    height,
                    ColorType::Rgba,
                    BitDepth::Eight,
                    Filter::NoFilter,
                    &pixels,
                );
                let mut options = default_options();
                options.max_width = 3;
                options.max_height = 3;
                assert_eq!(
                    thumbnail_png_rgba(&adam7, &options).unwrap(),
                    thumbnail_png_rgba(&sequential, &options).unwrap(),
                    "Adam7 mismatch for {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn adam7_enforces_sparse_memory_plan_before_decoding() {
        let width = 64;
        let height = 64;
        let encoded = encode_adam7_png(
            width,
            height,
            ColorType::Rgba,
            &vec![128; width as usize * height as usize * 4],
        );
        let mut options = default_options();
        options.max_width = width;
        options.max_height = height;
        options.output = OutputFormat::Rgba;
        let encoded_bytes = u64::try_from(encoded.len()).unwrap();
        let required = plan_thumbnail_sparse(
            InputInfo {
                dimensions: Dimensions::new(width, height).unwrap(),
                encoded_bytes,
                source_bytes_per_pixel: 4,
            },
            &options,
        )
        .unwrap()
        .memory
        .total_bytes;
        options.limits.max_working_memory_bytes = required - 1;

        assert!(matches!(
            thumbnail_png_rgba(&encoded, &options).unwrap_err(),
            Error::Core(CoreError::LimitExceeded {
                kind: LimitKind::WorkingMemory,
                ..
            })
        ));
    }

    #[test]
    fn adam7_truncation_fails_without_panicking() {
        for (color, depth, sample_bytes) in [
            (ColorType::GrayscaleAlpha, BitDepth::Eight, 2_usize),
            (ColorType::Rgba, BitDepth::Eight, 4_usize),
            (ColorType::Rgba, BitDepth::Sixteen, 8_usize),
        ] {
            let pixels = vec![42; 17 * 13 * sample_bytes];
            let mut encoded = encode_adam7_direct_png(17, 13, color, depth, &pixels);
            encoded.truncate(encoded.len() - 20);
            assert!(thumbnail_png_rgba(&encoded, &default_options()).is_err());
        }
    }

    #[test]
    fn preserves_rgba_rows_for_every_png_filter() {
        let pixels = [
            1, 2, 3, 4, 5, 6, 7, 8, // row 0
            9, 10, 11, 12, 13, 14, 15, 16, // row 1
        ];
        for filter in [
            Filter::NoFilter,
            Filter::Sub,
            Filter::Up,
            Filter::Avg,
            Filter::Paeth,
        ] {
            let encoded = encode_png(2, 2, ColorType::Rgba, BitDepth::Eight, filter, &pixels);
            let mut decoded = Vec::new();
            decode_png_rows(&encoded, &default_options(), |row| {
                decoded.extend_from_slice(row.pixels);
                Ok(())
            })
            .unwrap();
            assert_eq!(decoded, pixels, "filter {filter:?}");
        }
    }

    #[test]
    fn fuses_rgb_decode_with_arbitrary_ratio_area_downsampling() {
        let encoded = encode_png(
            3,
            1,
            ColorType::Rgb,
            BitDepth::Eight,
            Filter::Sub,
            &[0, 0, 0, 60, 60, 60, 120, 120, 120],
        );
        let options = ThumbnailOptions {
            max_width: 2,
            max_height: 1,
            ..default_options()
        };

        let thumbnail = thumbnail_png_rgba(&encoded, &options).unwrap();

        assert_eq!(thumbnail.dimensions, Dimensions::new(2, 1).unwrap());
        assert_eq!(thumbnail.pixels, [20, 20, 20, 255, 100, 100, 100, 255]);
    }

    #[test]
    fn centered_cover_is_shared_by_ordered_adam7_and_all_outputs() {
        let width = 4;
        let height = 2;
        let row = [10, 0, 0, 255, 20, 0, 0, 255, 30, 0, 0, 255, 40, 0, 0, 255];
        let pixels = [row, row].concat();
        let ordered = encode_png(
            width,
            height,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &pixels,
        );
        let adam7 = encode_adam7_png(width, height, ColorType::Rgba, &pixels);
        let expected = [20, 0, 0, 255, 30, 0, 0, 255].repeat(2);

        for input in [&ordered, &adam7] {
            let rgba_options = ThumbnailOptions {
                max_width: 2,
                max_height: 2,
                fit: Fit::Cover,
                output: OutputFormat::Rgba,
                ..default_options()
            };
            let rgba = thumbnail_png_rgba(input, &rgba_options).unwrap();
            assert_eq!(rgba.dimensions, Dimensions::new(2, 2).unwrap());
            assert_eq!(rgba.pixels, expected);

            let png_options = ThumbnailOptions {
                output: OutputFormat::Png,
                ..rgba_options
            };
            let png = encoded_bytes(thumbnail_png(input, &png_options).unwrap());
            assert_eq!(decode_raw_png(&png), (ColorType::Rgba, expected.clone()));

            let jpeg_options = ThumbnailOptions {
                output: OutputFormat::Jpeg,
                ..rgba_options
            };
            let jpeg = thumbnail_png_with_jpeg_options(
                input,
                &jpeg_options,
                &JpegOptions {
                    quality: 100,
                    subsampling: JpegSubsampling::S444,
                    ..JpegOptions::default()
                },
            )
            .unwrap();
            let (jpeg_width, jpeg_height, _) = decode_raw_jpeg(&encoded_bytes(jpeg));
            assert_eq!((jpeg_width, jpeg_height), (2, 2));
        }
    }

    #[test]
    fn fused_path_uses_premultiplied_alpha() {
        let encoded = encode_png(
            2,
            1,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[255, 0, 0, 0, 0, 0, 255, 255],
        );
        let options = ThumbnailOptions {
            max_width: 1,
            max_height: 1,
            ..default_options()
        };

        let thumbnail = thumbnail_png_rgba(&encoded, &options).unwrap();

        assert_eq!(thumbnail.pixels, [0, 0, 255, 128]);
    }

    #[test]
    fn public_output_api_returns_raw_rgba_when_requested() {
        let encoded = encode_png(
            1,
            1,
            ColorType::Rgb,
            BitDepth::Eight,
            Filter::NoFilter,
            &[10, 20, 30],
        );
        let options = ThumbnailOptions {
            output: OutputFormat::Rgba,
            ..default_options()
        };

        let output = thumbnail_png(&encoded, &options).unwrap();

        assert_eq!(
            output,
            ThumbnailOutput::Rgba {
                pixels: vec![10, 20, 30, 255],
                width: 1,
                height: 1,
            }
        );
        assert_eq!(output.info().format, OutputFormat::Rgba);
    }

    #[test]
    fn encoded_output_round_trips_as_rgba_png() {
        let encoded = encode_png(
            2,
            1,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[255, 0, 0, 0, 0, 0, 255, 255],
        );
        let options = ThumbnailOptions {
            max_width: 1,
            max_height: 1,
            output: OutputFormat::Png,
            ..default_options()
        };

        let output = thumbnail_png(&encoded, &options).unwrap();
        let ThumbnailOutput::Encoded {
            bytes,
            width,
            height,
            mime_type,
            ..
        } = output
        else {
            panic!("expected encoded PNG output");
        };
        assert_eq!((width, height, mime_type), (1, 1, "image/png"));

        let mut reader = png::Decoder::new(Cursor::new(bytes)).read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut pixels).unwrap();
        assert_eq!(&pixels[..info.buffer_size()], &[0, 0, 255, 128]);
    }

    #[test]
    fn direct_writers_match_buffered_png_and_jpeg_for_ordered_and_adam7_input() {
        let width = 17;
        let height = 19;
        let pixels = (0..width * height)
            .flat_map(|index| {
                let value = u8::try_from(index % 251).unwrap();
                [value, value.wrapping_mul(3), value.wrapping_mul(5), 255]
            })
            .collect::<Vec<_>>();
        let ordered = encode_png(
            width,
            height,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::Paeth,
            &pixels,
        );
        let adam7 = encode_adam7_png(width, height, ColorType::Rgba, &pixels);

        for input in [&ordered, &adam7] {
            let png_options = ThumbnailOptions {
                max_width: 11,
                max_height: 13,
                output: OutputFormat::Png,
                ..default_options()
            };
            let expected_png = encoded_bytes(thumbnail_png(input, &png_options).unwrap());
            let png_writer = SharedWriter::default();
            let png_info =
                thumbnail_png_to_writer(input, &png_options, png_writer.clone()).unwrap();
            assert_eq!(png_writer.bytes(), expected_png);
            assert_eq!(png_info.format, OutputFormat::Png);
            assert_eq!((png_info.width, png_info.height), (11, 12));

            let jpeg_options = ThumbnailOptions {
                output: OutputFormat::Jpeg,
                ..png_options
            };
            let expected_jpeg = encoded_bytes(
                thumbnail_png_with_jpeg_options(input, &jpeg_options, &JpegOptions::default())
                    .unwrap(),
            );
            let jpeg_writer = SharedWriter::default();
            let jpeg_info =
                thumbnail_jpeg_to_writer(input, &jpeg_options, jpeg_writer.clone()).unwrap();
            assert_eq!(jpeg_writer.bytes(), expected_jpeg);
            assert_eq!(jpeg_info.format, OutputFormat::Jpeg);
            assert_eq!((jpeg_info.width, jpeg_info.height), (11, 12));
        }
    }

    #[test]
    fn direct_writer_failures_are_returned_as_encode_errors() {
        let input = encode_png(
            1,
            1,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[1, 2, 3, 255],
        );
        let options = ThumbnailOptions {
            output: OutputFormat::Png,
            ..default_options()
        };

        assert!(matches!(
            thumbnail_png_to_writer(&input, &options, FailingWriter),
            Err(Error::EncodeFailure(message)) if message.contains("injected writer failure")
        ));
    }

    #[test]
    fn direct_writer_uses_the_smaller_non_retaining_memory_plan() {
        let width = 16;
        let height = 16;
        let input = encode_png(
            width,
            height,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[91; 16 * 16 * 4],
        );
        let mut options = ThumbnailOptions {
            max_width: width,
            max_height: height,
            output: OutputFormat::Png,
            ..default_options()
        };
        let writer_plan = plan_thumbnail_to_writer_with_buffer(
            InputInfo {
                dimensions: Dimensions::new(width, height).unwrap(),
                encoded_bytes: u64::try_from(input.len()).unwrap(),
                source_bytes_per_pixel: 4,
            },
            &options,
            0,
        )
        .unwrap();
        options.limits.max_working_memory_bytes = writer_plan.memory.total_bytes;

        assert!(matches!(
            thumbnail_png(&input, &options),
            Err(Error::Core(CoreError::LimitExceeded {
                kind: LimitKind::WorkingMemory,
                ..
            }))
        ));
        let output = SharedWriter::default();
        thumbnail_png_to_writer(&input, &options, output.clone()).unwrap();
        assert!(!output.bytes().is_empty());
    }

    #[test]
    fn streaming_jpeg_decodes_for_ordered_and_adam7_inputs() {
        let width = 17;
        let height = 33;
        let pixels = (0..width * height)
            .flat_map(|index| {
                let value = u8::try_from(index % 251).unwrap();
                [value, value.wrapping_mul(3), value.wrapping_mul(7), 255]
            })
            .collect::<Vec<_>>();
        let ordered = encode_png(
            width,
            height,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::Paeth,
            &pixels,
        );
        let adam7 = encode_adam7_png(width, height, ColorType::Rgba, &pixels);
        let options = ThumbnailOptions {
            max_width: width,
            max_height: height,
            output: OutputFormat::Jpeg,
            ..default_options()
        };
        let jpeg_options = JpegOptions {
            quality: 100,
            subsampling: JpegSubsampling::S444,
            ..JpegOptions::default()
        };

        for input in [&ordered, &adam7] {
            let output = thumbnail_png_with_jpeg_options(input, &options, &jpeg_options).unwrap();
            assert_eq!(output.info().format, OutputFormat::Jpeg);
            assert!(matches!(
                &output,
                ThumbnailOutput::Encoded {
                    mime_type: "image/jpeg",
                    ..
                }
            ));
            let (actual_width, actual_height, actual) = decode_raw_jpeg(&encoded_bytes(output));
            assert_eq!((actual_width, actual_height), (17, 33));
            for (actual, expected) in actual.chunks_exact(3).zip(pixels.chunks_exact(4)) {
                for channel in 0..3 {
                    assert!(
                        actual[channel].abs_diff(expected[channel]) <= 4,
                        "channel {channel}: actual {}, expected {}",
                        actual[channel],
                        expected[channel]
                    );
                }
            }
        }
    }

    #[test]
    fn jpeg_settings_require_jpeg_output() {
        let input = encode_png(
            1,
            1,
            ColorType::Rgb,
            BitDepth::Eight,
            Filter::NoFilter,
            &[1, 2, 3],
        );
        assert!(matches!(
            thumbnail_png_with_jpeg_options(&input, &default_options(), &JpegOptions::default()),
            Err(Error::InvalidJpegOptions(_))
        ));
    }

    #[test]
    fn explicit_png_color_modes_write_expected_color_types_and_pixels() {
        let input = encode_png(
            2,
            1,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[255, 0, 0, 64, 0, 255, 0, 255],
        );
        let thumbnail_options = ThumbnailOptions {
            max_width: 2,
            max_height: 1,
            output: OutputFormat::Png,
            ..default_options()
        };
        let cases = [
            (
                PngColorMode::Rgba8,
                ColorType::Rgba,
                vec![255, 0, 0, 64, 0, 255, 0, 255],
            ),
            (
                PngColorMode::Rgb8,
                ColorType::Rgb,
                vec![255, 0, 0, 0, 255, 0],
            ),
            (
                PngColorMode::GrayscaleAlpha8,
                ColorType::GrayscaleAlpha,
                vec![77, 64, 149, 255],
            ),
            (
                PngColorMode::Grayscale8,
                ColorType::Grayscale,
                vec![77, 149],
            ),
        ];

        for (color, expected_type, expected_pixels) in cases {
            let options = PngOptions {
                color,
                ..PngOptions::default()
            };
            let output =
                thumbnail_png_with_encoder_options(&input, &thumbnail_options, &options).unwrap();
            let (actual_type, actual_pixels) = decode_raw_png(&encoded_bytes(output));
            assert_eq!(actual_type, expected_type, "color mode {color:?}");
            assert_eq!(actual_pixels, expected_pixels, "color mode {color:?}");
        }
    }

    #[test]
    fn default_png_options_are_deterministic_and_match_the_convenience_api() {
        let input = encode_png(
            3,
            1,
            ColorType::Rgb,
            BitDepth::Eight,
            Filter::Sub,
            &[1, 2, 3, 4, 5, 6, 7, 8, 9],
        );
        let options = default_options();
        let first = encoded_bytes(thumbnail_png(&input, &options).unwrap());
        let second = encoded_bytes(
            thumbnail_png_with_encoder_options(&input, &options, &PngOptions::default()).unwrap(),
        );

        assert_eq!(first, second);
        assert_eq!(decode_raw_png(&first).0, ColorType::Rgba);
    }

    #[test]
    fn auto_png_color_uses_only_safe_input_metadata() {
        let grayscale = encode_grayscale_png(2, 1, BitDepth::Eight, &[10, 20], None);
        let grayscale_alpha = encode_grayscale_png(2, 1, BitDepth::Eight, &[10, 20], Some(10));
        let rgb = encode_png(
            1,
            1,
            ColorType::Rgb,
            BitDepth::Eight,
            Filter::NoFilter,
            &[1, 2, 3],
        );
        let rgba = encode_png(
            1,
            1,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[1, 2, 3, 4],
        );
        let gray_palette =
            encode_palette_png(2, 1, BitDepth::One, &[0, 1], &[0, 0, 0, 9, 9, 9], &[]);
        let color_palette =
            encode_palette_png(2, 1, BitDepth::One, &[0, 1], &[1, 2, 3, 4, 5, 6], &[255, 7]);
        let cases = [
            (&grayscale, ColorType::Grayscale),
            (&grayscale_alpha, ColorType::GrayscaleAlpha),
            (&rgb, ColorType::Rgb),
            (&rgba, ColorType::Rgba),
            (&gray_palette, ColorType::Grayscale),
            (&color_palette, ColorType::Rgba),
        ];
        let options = PngOptions {
            color: PngColorMode::Auto,
            ..PngOptions::default()
        };

        for (input, expected_type) in cases {
            let output =
                thumbnail_png_with_encoder_options(input, &default_options(), &options).unwrap();
            assert_eq!(decode_raw_png(&encoded_bytes(output)).0, expected_type);
        }
    }

    #[test]
    fn every_compression_and_filter_setting_writes_a_valid_png() {
        let input = encode_png(
            3,
            2,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[17; 3 * 2 * 4],
        );
        let compressions = [
            PngCompression::NoCompression,
            PngCompression::Fastest,
            PngCompression::Fast,
            PngCompression::Balanced,
            PngCompression::High,
        ];
        let filters = [
            PngFilter::Default,
            PngFilter::None,
            PngFilter::Sub,
            PngFilter::Up,
            PngFilter::Average,
            PngFilter::Paeth,
            PngFilter::Adaptive,
            PngFilter::MinEntropy,
        ];

        for compression in compressions {
            for filter in filters {
                let options = PngOptions {
                    compression,
                    filter,
                    ..PngOptions::default()
                };
                let output =
                    thumbnail_png_with_encoder_options(&input, &default_options(), &options)
                        .unwrap();
                let (color, pixels) = decode_raw_png(&encoded_bytes(output));
                assert_eq!(color, ColorType::Rgba);
                assert_eq!(pixels, [17; 3 * 2 * 4]);
            }
        }
    }

    #[test]
    fn adam7_input_uses_the_selected_png_encoder_options() {
        let input = encode_adam7_png(
            2,
            2,
            ColorType::Rgba,
            &[
                255, 0, 0, 64, 0, 255, 0, 128, 0, 0, 255, 192, 255, 255, 255, 255,
            ],
        );
        let options = PngOptions {
            color: PngColorMode::GrayscaleAlpha8,
            compression: PngCompression::High,
            filter: PngFilter::MinEntropy,
        };
        let output =
            thumbnail_png_with_encoder_options(&input, &default_options(), &options).unwrap();
        let (color, pixels) = decode_raw_png(&encoded_bytes(output));

        assert_eq!(color, ColorType::GrayscaleAlpha);
        assert_eq!(pixels, [77, 64, 149, 128, 29, 192, 255, 255]);
    }

    #[test]
    fn png_settings_are_rejected_for_raw_rgba_output() {
        let input = encode_png(
            1,
            1,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[0, 0, 0, 255],
        );
        let thumbnail_options = ThumbnailOptions {
            output: OutputFormat::Rgba,
            ..default_options()
        };
        let png_options = PngOptions {
            color: PngColorMode::Rgb8,
            ..PngOptions::default()
        };

        assert!(matches!(
            thumbnail_png_with_encoder_options(&input, &thumbnail_options, &png_options),
            Err(Error::InvalidPngOptions(_))
        ));
    }

    #[test]
    fn streaming_png_matches_rgba_output_for_ordered_and_adam7_inputs() {
        let width = 9;
        let height = 7;
        let pixels = (0..width * height)
            .flat_map(|index| {
                let value = u8::try_from(index).unwrap();
                [
                    value.wrapping_mul(3),
                    value.wrapping_mul(5),
                    value.wrapping_mul(7),
                    value.wrapping_mul(11),
                ]
            })
            .collect::<Vec<_>>();
        let ordered = encode_png(
            width,
            height,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::Paeth,
            &pixels,
        );
        let adam7 = encode_adam7_png(width, height, ColorType::Rgba, &pixels);

        for input in [&ordered, &adam7] {
            let rgba_options = ThumbnailOptions {
                max_width: 4,
                max_height: 3,
                output: OutputFormat::Rgba,
                ..default_options()
            };
            let expected = thumbnail_png_rgba(input, &rgba_options).unwrap();
            let png_options = ThumbnailOptions {
                output: OutputFormat::Png,
                ..rgba_options
            };
            let mut reader =
                SeekableInput::new(Cursor::new(input.as_slice()), &png_options).unwrap();
            let (encoded, plan) =
                thumbnail_png_encoded(&mut reader, &png_options, PngOptions::default()).unwrap();
            let actual = thumbnail_png_rgba(&encoded, &rgba_options).unwrap();

            assert_eq!(actual, expected);
            assert_eq!(plan.memory.output_rgba_bytes, 0);
            assert_eq!(
                plan.memory.output_row_bytes,
                usize::try_from(plan.output.width).unwrap() * 4
            );
        }
    }

    #[test]
    fn rejects_input_byte_limit_before_png_parsing() {
        let encoded = encode_png(
            1,
            1,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[0, 0, 0, 0],
        );
        let mut options = default_options();
        options.limits.max_input_bytes = u64::try_from(encoded.len() - 1).unwrap();

        let error = decode_png_rows(&encoded, &options, |_| Ok(())).unwrap_err();
        assert!(matches!(
            error,
            Error::Core(CoreError::LimitExceeded {
                kind: LimitKind::InputBytes,
                ..
            })
        ));
    }

    #[test]
    fn rejects_dimensions_before_decoding_rows() {
        let encoded = encode_png(
            2,
            1,
            ColorType::Rgb,
            BitDepth::Eight,
            Filter::NoFilter,
            &[0; 6],
        );
        let mut options = default_options();
        options.limits.max_width = 1;
        let mut callbacks = 0;

        let error = decode_png_rows(&encoded, &options, |_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();

        assert_eq!(callbacks, 0);
        assert!(matches!(
            error,
            Error::Core(CoreError::LimitExceeded {
                kind: LimitKind::InputWidth,
                ..
            })
        ));
    }

    #[test]
    fn rejects_pixel_bomb_from_ihdr_before_decoding_rows() {
        let mut encoded = encode_png(
            1,
            1,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[0, 0, 0, 0],
        );
        replace_ihdr_dimensions(&mut encoded, 50_000, 50_000);
        let mut options = default_options();
        options.limits.max_width = 60_000;
        options.limits.max_height = 60_000;
        options.limits.max_pixels = 500_000_000;
        let mut callbacks = 0;

        let error = decode_png_rows(&encoded, &options, |_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();

        assert_eq!(callbacks, 0);
        assert!(matches!(
            error,
            Error::Core(CoreError::LimitExceeded {
                kind: LimitKind::InputPixels,
                ..
            })
        ));
    }

    #[test]
    fn rejects_oversized_truncated_ancillary_chunk_without_decoding_rows() {
        let mut encoded = encode_png(
            1,
            1,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[0, 0, 0, 0],
        );
        let mut chunk_header = Vec::new();
        chunk_header.extend_from_slice(&0x7fff_ffff_u32.to_be_bytes());
        chunk_header.extend_from_slice(b"eXIf");
        encoded.splice(33..33, chunk_header);
        let mut callbacks = 0;

        let error = decode_png_rows(&encoded, &default_options(), |_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();

        assert_eq!(callbacks, 0);
        assert!(matches!(
            error,
            Error::TruncatedInput
                | Error::DecodeFailure(_)
                | Error::DecoderMemoryLimitExceeded { .. }
        ));
    }

    #[test]
    fn highly_compressible_source_plan_stays_below_full_frame_size() {
        let width = 512;
        let height = 512;
        let pixels = vec![0_u8; width * height * 4];
        let encoded = encode_png(
            width as u32,
            height as u32,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &pixels,
        );
        let options = ThumbnailOptions {
            max_width: 16,
            max_height: 16,
            output: OutputFormat::Rgba,
            ..default_options()
        };
        let mut rows = 0;

        let info = decode_png_rows(&encoded, &options, |_| {
            rows += 1;
            Ok(())
        })
        .unwrap();

        assert_eq!(rows, height);
        assert!(encoded.len() < pixels.len() / 10);
        assert!(info.plan.memory.total_bytes < pixels.len());
    }

    #[test]
    fn rejects_memory_budget_before_decoding_rows() {
        let encoded = encode_png(
            2,
            2,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[0; 16],
        );
        let mut options = default_options();
        options.limits.max_working_memory_bytes = 1;
        let mut callbacks = 0;

        let error = decode_png_rows(&encoded, &options, |_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();

        assert_eq!(callbacks, 0);
        assert!(matches!(
            error,
            Error::Core(CoreError::LimitExceeded {
                kind: LimitKind::WorkingMemory,
                ..
            })
        ));
    }

    #[test]
    fn preflight_matches_buffered_execution_for_ordered_and_adam7_input() {
        let width = 9;
        let height = 7;
        let pixels = (0..width * height * 4)
            .map(|index| u8::try_from((index * 17) % 251).unwrap())
            .collect::<Vec<_>>();
        let ordered = encode_png(
            width,
            height,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::Paeth,
            &pixels,
        );
        let adam7 = encode_adam7_png(width, height, ColorType::Rgba, &pixels);
        let options = ThumbnailOptions {
            max_width: 5,
            max_height: 4,
            output: OutputFormat::Rgba,
            ..default_options()
        };

        for (encoded, interlaced) in [(&ordered, false), (&adam7, true)] {
            let preflight = preflight_thumbnail_png(encoded, &options).unwrap();
            let mut input = SeekableInput::new(Cursor::new(encoded), &options).unwrap();
            let (_, executed) = thumbnail_png_rgba_planned(&mut input, &options).unwrap();

            assert_eq!(preflight.processing, executed);
            assert_eq!(
                preflight.input.dimensions,
                Dimensions::new(width, height).unwrap()
            );
            assert_eq!(preflight.input.encoded_bytes, encoded.len() as u64);
            assert_eq!(preflight.input.color_type, PngInputColorType::Rgba);
            assert_eq!(preflight.input.bit_depth, 8);
            assert_eq!(preflight.input.interlaced, interlaced);
            assert!(preflight.within_memory_limit);
            if interlaced {
                assert!(preflight.processing.memory.sparse_accumulator_bytes > 0);
                assert_eq!(preflight.processing.memory.horizontal_accumulator_bytes, 0);
                assert_eq!(preflight.processing.memory.vertical_accumulator_bytes, 0);
            }
        }
    }

    #[test]
    fn writer_preflight_matches_execution_and_includes_the_adapter_buffer() {
        let encoded = encode_png(
            9,
            7,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::Paeth,
            &[31; 9 * 7 * 4],
        );
        let options = ThumbnailOptions {
            max_width: 5,
            max_height: 4,
            output: OutputFormat::Png,
            ..default_options()
        };
        let buffer_bytes = 64 * 1024;
        let preflight =
            preflight_thumbnail_png_to_writer_with_buffer(&encoded, &options, buffer_bytes)
                .unwrap();
        let mut input = SeekableInput::new(Cursor::new(encoded), &options).unwrap();
        let executed = thumbnail_png_encoded_to_writer(
            &mut input,
            &options,
            PngOptions::default(),
            buffer_bytes,
            SharedWriter::default(),
        )
        .unwrap();

        assert_eq!(preflight.processing, executed);
        assert_eq!(
            preflight.processing.memory.encoded_output_bytes,
            buffer_bytes
        );
    }

    #[test]
    fn preflight_reports_a_memory_rejection_without_weakening_execution() {
        let encoded = encode_png(
            2,
            2,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[0; 16],
        );
        let mut options = ThumbnailOptions {
            output: OutputFormat::Rgba,
            ..default_options()
        };
        options.limits.max_working_memory_bytes = 1;

        let preflight = preflight_thumbnail_png(&encoded, &options).unwrap();

        assert!(!preflight.within_memory_limit);
        assert_eq!(preflight.configured_max_working_memory_bytes, 1);
        assert!(preflight.processing.memory.total_bytes > 1);
        assert!(matches!(
            thumbnail_png_rgba(&encoded, &options),
            Err(Error::Core(streamthumb_core::Error::LimitExceeded {
                kind: LimitKind::WorkingMemory,
                ..
            }))
        ));
    }

    #[test]
    fn writer_preflight_rejects_raw_rgba_delivery() {
        let encoded = encode_png(
            1,
            1,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[0; 4],
        );
        let options = ThumbnailOptions {
            output: OutputFormat::Rgba,
            ..default_options()
        };

        assert!(matches!(
            preflight_thumbnail_png_to_writer_with_buffer(&encoded, &options, 64 * 1024),
            Err(Error::InvalidOutputDelivery(_))
        ));
    }

    #[test]
    fn normalizes_sixteen_bit_direct_color_with_rounding() {
        let cases = [
            (
                ColorType::Grayscale,
                vec![0x80, 0x00],
                vec![128, 128, 128, 255],
            ),
            (
                ColorType::GrayscaleAlpha,
                vec![0x12, 0x34, 0xab, 0xcd],
                vec![18, 18, 18, 171],
            ),
            (
                ColorType::Rgb,
                vec![0, 0, 0x80, 0, 0xff, 0xff],
                vec![0, 128, 255, 255],
            ),
            (
                ColorType::Rgba,
                vec![0x01, 0x01, 0x7f, 0xff, 0x80, 0, 0xff, 0xff],
                vec![1, 127, 128, 255],
            ),
        ];

        for (color, source, expected) in cases {
            let encoded = encode_png(1, 1, color, BitDepth::Sixteen, Filter::Paeth, &source);
            let mut actual = Vec::new();
            let info = decode_png_rows(&encoded, &default_options(), |row| {
                actual.extend_from_slice(row.pixels);
                Ok(())
            })
            .unwrap();
            assert_eq!(actual, expected, "16-bit mismatch for {color:?}");
            if color == ColorType::Rgba {
                assert_eq!(info.plan.memory.decoder_rows_bytes, 27);
            }
        }
    }

    #[test]
    fn rejects_interlacing_from_the_header() {
        let mut encoded = encode_png(
            1,
            1,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[0, 0, 0, 0],
        );
        encoded[28] = 1;
        replace_ihdr_crc(&mut encoded);

        assert!(matches!(
            decode_png_rows(&encoded, &default_options(), |_| Ok(())).unwrap_err(),
            Error::Unsupported {
                feature: UnsupportedFeature::Interlacing,
                ..
            }
        ));
    }

    #[test]
    fn rejects_apng_before_decoding_rows() {
        let mut encoded = encode_png(
            1,
            1,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[0, 0, 0, 0],
        );
        insert_chunk_after_ihdr(
            &mut encoded,
            *b"fcTL",
            &[
                0, 0, 0, 0, // sequence number
                0, 0, 0, 1, // width
                0, 0, 0, 1, // height
                0, 0, 0, 0, // x offset
                0, 0, 0, 0, // y offset
                0, 1, // delay numerator
                0, 100, // delay denominator
                0,   // dispose operation
                0,   // blend operation
            ],
        );
        insert_chunk_after_ihdr(&mut encoded, *b"acTL", &[0, 0, 0, 1, 0, 0, 0, 0]);
        let mut callbacks = 0;

        let error = decode_png_rows(&encoded, &default_options(), |_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();

        assert_eq!(callbacks, 0);
        assert!(matches!(
            error,
            Error::Unsupported {
                feature: UnsupportedFeature::Animation,
                ..
            }
        ));
        assert!(matches!(
            preflight_thumbnail_png(&encoded, &default_options()),
            Err(Error::Unsupported {
                feature: UnsupportedFeature::Animation,
                ..
            })
        ));
    }

    #[test]
    fn rejects_malformed_input_without_calling_the_consumer() {
        let mut callbacks = 0;
        let error = decode_png_rows(b"not a PNG", &default_options(), |_| {
            callbacks += 1;
            Ok(())
        })
        .unwrap_err();

        assert_eq!(callbacks, 0);
        assert!(matches!(error, Error::DecodeFailure(_)));
    }

    #[test]
    fn rejects_truncated_input_without_panicking() {
        let mut encoded = encode_png(
            8,
            8,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[42; 8 * 8 * 4],
        );
        encoded.truncate(encoded.len() / 2);

        let error = decode_png_rows(&encoded, &default_options(), |_| Ok(())).unwrap_err();
        assert!(matches!(error, Error::TruncatedInput), "{error:?}");
        assert!(matches!(
            preflight_thumbnail_png(&encoded, &default_options()),
            Err(Error::TruncatedInput)
        ));
    }

    #[test]
    fn propagates_consumer_failures_and_stops_decoding() {
        let encoded = encode_png(
            1,
            3,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[0; 12],
        );
        let mut callbacks = 0;

        let error = decode_png_rows(&encoded, &default_options(), |_| {
            callbacks += 1;
            Err(Error::RowConsumer("intentional test failure".to_owned()))
        })
        .unwrap_err();

        assert_eq!(callbacks, 1);
        assert!(matches!(error, Error::RowConsumer(_)));
    }

    #[test]
    fn seekable_reader_apis_match_slice_apis_for_ordered_and_adam7_input() {
        let width = 9;
        let height = 7;
        let pixels = (0..width * height * 4)
            .map(|index| u8::try_from((index * 37) % 251).unwrap())
            .collect::<Vec<_>>();
        let ordered = encode_png(
            width,
            height,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::Paeth,
            &pixels,
        );
        let adam7 = encode_adam7_png(width, height, ColorType::Rgba, &pixels);
        let rgba_options = ThumbnailOptions {
            max_width: 5,
            max_height: 4,
            output: OutputFormat::Rgba,
            ..default_options()
        };
        let png_options = ThumbnailOptions {
            output: OutputFormat::Png,
            ..rgba_options
        };
        let jpeg_options = ThumbnailOptions {
            output: OutputFormat::Jpeg,
            ..rgba_options
        };

        for encoded in [ordered, adam7] {
            assert_eq!(
                thumbnail_png_rgba_from_reader(Cursor::new(encoded.as_slice()), &rgba_options)
                    .unwrap(),
                thumbnail_png_rgba(&encoded, &rgba_options).unwrap()
            );
            let mut prefixed = vec![91; 13];
            prefixed.extend_from_slice(&encoded);
            let mut prefixed_reader = Cursor::new(prefixed);
            prefixed_reader.set_position(13);
            assert_eq!(
                thumbnail_png_rgba_from_reader(prefixed_reader, &rgba_options).unwrap(),
                thumbnail_png_rgba(&encoded, &rgba_options).unwrap()
            );

            let ThumbnailOutput::Encoded {
                bytes: expected, ..
            } = thumbnail_png(&encoded, &png_options).unwrap()
            else {
                panic!("PNG options must produce encoded output");
            };
            let output = SharedWriter::default();
            thumbnail_png_from_reader_to_writer(
                Cursor::new(encoded.as_slice()),
                &png_options,
                output.clone(),
            )
            .unwrap();
            assert_eq!(*output.0.lock().unwrap(), expected);

            let expected = SharedWriter::default();
            thumbnail_jpeg_to_writer(&encoded, &jpeg_options, expected.clone()).unwrap();
            let actual = SharedWriter::default();
            thumbnail_jpeg_from_reader_to_writer(
                Cursor::new(encoded),
                &jpeg_options,
                actual.clone(),
            )
            .unwrap();
            assert_eq!(*actual.0.lock().unwrap(), *expected.0.lock().unwrap());
        }
    }

    #[test]
    fn seekable_reader_decodes_through_small_bounded_chunks() {
        let encoded = encode_png(
            64,
            64,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::Paeth,
            &(0..64 * 64 * 4)
                .map(|index| u8::try_from((index * 29) % 251).unwrap())
                .collect::<Vec<_>>(),
        );
        let (reader, stats) = ChunkedReader::new(encoded, 7);

        let image = thumbnail_png_rgba_from_reader(reader, &default_options()).unwrap();

        assert_eq!(image.dimensions.width, 64);
        let stats = stats.lock().unwrap();
        assert!(stats.reads > 10);
        assert!(stats.seeks >= 4);
        assert!(stats.max_read <= 7);
    }

    #[test]
    fn seekable_reader_rejects_input_limit_before_reading() {
        let encoded = encode_png(
            2,
            2,
            ColorType::Rgba,
            BitDepth::Eight,
            Filter::NoFilter,
            &[0; 16],
        );
        let mut options = default_options();
        options.limits.max_input_bytes = u64::try_from(encoded.len() - 1).unwrap();
        let (reader, stats) = ChunkedReader::new(encoded, 3);

        let error = thumbnail_png_rgba_from_reader(reader, &options).unwrap_err();

        assert!(matches!(
            error,
            Error::Core(CoreError::LimitExceeded {
                kind: LimitKind::InputBytes,
                ..
            })
        ));
        let stats = stats.lock().unwrap();
        assert_eq!(stats.reads, 0);
        assert_eq!(stats.seeks, 3);
    }

    #[test]
    fn seekable_reader_preserves_input_io_errors() {
        let error = thumbnail_png_rgba_from_reader(
            ReadFailure {
                position: 0,
                length: 128,
            },
            &default_options(),
        )
        .unwrap_err();

        assert!(matches!(error, Error::InputIo(_)));
        assert_eq!(
            error
                .source()
                .and_then(|source| source.downcast_ref::<std::io::Error>())
                .map(std::io::Error::kind),
            Some(std::io::ErrorKind::PermissionDenied)
        );
    }

    fn replace_ihdr_crc(png: &mut [u8]) {
        let crc = crc32(&png[12..29]).to_be_bytes();
        png[29..33].copy_from_slice(&crc);
    }

    fn replace_ihdr_dimensions(png: &mut [u8], width: u32, height: u32) {
        png[16..20].copy_from_slice(&width.to_be_bytes());
        png[20..24].copy_from_slice(&height.to_be_bytes());
        replace_ihdr_crc(png);
    }

    fn insert_chunk_after_ihdr(png: &mut Vec<u8>, chunk_type: [u8; 4], data: &[u8]) {
        let mut chunk = Vec::with_capacity(12 + data.len());
        chunk.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
        chunk.extend_from_slice(&chunk_type);
        chunk.extend_from_slice(data);
        chunk.extend_from_slice(&crc32(&chunk[4..]).to_be_bytes());
        png.splice(33..33, chunk);
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
}
