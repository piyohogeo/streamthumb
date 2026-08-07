use std::env;
use std::error::Error;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::Instant;

use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat};
use streamthumb_core::{Dimensions, Fit, OutputFormat, ThumbnailOptions, contain_dimensions};
use streamthumb_png::{ThumbnailOutput, thumbnail_png};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone, Copy)]
enum Pattern {
    Blank,
    Gradient,
    Noise,
}

impl Pattern {
    fn name(self) -> &'static str {
        match self {
            Self::Blank => "blank",
            Self::Gradient => "gradient",
            Self::Noise => "noise",
        }
    }
}

#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    width: u32,
    height: u32,
    pattern: Pattern,
    interlaced: bool,
}

const SMOKE_CASES: &[Case] = &[
    Case {
        name: "square-blank",
        width: 2_048,
        height: 2_048,
        pattern: Pattern::Blank,
        interlaced: false,
    },
    Case {
        name: "wide-gradient",
        width: 8_192,
        height: 64,
        pattern: Pattern::Gradient,
        interlaced: false,
    },
    Case {
        name: "tall-noise",
        width: 64,
        height: 8_192,
        pattern: Pattern::Noise,
        interlaced: false,
    },
];

const MEMORY_CASES: &[Case] = &[
    Case {
        name: "8k-square-blank",
        width: 8_192,
        height: 8_192,
        pattern: Pattern::Blank,
        interlaced: false,
    },
    Case {
        name: "16k-square-blank",
        width: 16_384,
        height: 16_384,
        pattern: Pattern::Blank,
        interlaced: false,
    },
    Case {
        name: "very-wide-gradient",
        width: 100_000,
        height: 32,
        pattern: Pattern::Gradient,
        interlaced: false,
    },
    Case {
        name: "very-tall-gradient",
        width: 32,
        height: 100_000,
        pattern: Pattern::Gradient,
        interlaced: false,
    },
];

const ADAM7_CASES: &[Case] = &[
    Case {
        name: "adam7-square-blank",
        width: 2_048,
        height: 2_048,
        pattern: Pattern::Blank,
        interlaced: true,
    },
    Case {
        name: "adam7-wide-gradient",
        width: 8_192,
        height: 64,
        pattern: Pattern::Gradient,
        interlaced: true,
    },
];

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    match args.as_slice() {
        [_, command, directory] if command == "generate-smoke" => {
            generate_corpus(Path::new(directory), SMOKE_CASES)
        }
        [_, command, directory] if command == "generate-memory" => {
            generate_corpus(Path::new(directory), MEMORY_CASES)
        }
        [_, command, directory] if command == "generate-adam7" => {
            generate_corpus(Path::new(directory), ADAM7_CASES)
        }
        [_, command, method, input, output, max_dimension] if command == "run" => {
            let max_dimension = max_dimension.parse::<u32>()?;
            run_method(method, Path::new(input), Path::new(output), max_dimension)
        }
        _ => {
            eprintln!(
                "usage:\n  streamthumb-benchmarks generate-smoke <directory>\n  \
                 streamthumb-benchmarks generate-memory <directory>\n  \
                 streamthumb-benchmarks generate-adam7 <directory>\n  \
                 streamthumb-benchmarks run <streamthumb-png|streamthumb-jpeg|streamthumb-cover-png|streamthumb-cover-jpeg|image-rs> <input> <output> <max-dimension>"
            );
            Err("invalid benchmark arguments".into())
        }
    }
}

fn generate_corpus(directory: &Path, cases: &[Case]) -> Result<()> {
    fs::create_dir_all(directory)?;
    let mut manifest = BufWriter::new(File::create(directory.join("manifest.tsv"))?);
    writeln!(manifest, "name\twidth\theight\tpattern\tinterlaced\tfile")?;

    for case in cases {
        let file_name = format!("{}-{}x{}.png", case.name, case.width, case.height);
        let path = directory.join(&file_name);
        if case.interlaced {
            write_adam7_png(&path, *case)?;
        } else {
            write_png(&path, *case)?;
        }
        writeln!(
            manifest,
            "{}\t{}\t{}\t{}\t{}\t{}",
            case.name,
            case.width,
            case.height,
            case.pattern.name(),
            case.interlaced,
            file_name
        )?;
    }
    Ok(())
}

fn write_adam7_png(path: &Path, case: Case) -> Result<()> {
    use flate2::{Compression, write::ZlibEncoder};

    const PASSES: [(u32, u32, u32, u32); 7] = [
        (8, 0, 8, 0),
        (8, 4, 8, 0),
        (4, 0, 8, 4),
        (4, 2, 4, 0),
        (2, 0, 4, 2),
        (2, 1, 2, 0),
        (1, 0, 2, 1),
    ];
    let mut compressor = ZlibEncoder::new(Vec::new(), Compression::default());
    for (x_stride, x_offset, y_stride, y_offset) in PASSES {
        let samples = case.width.saturating_sub(x_offset).div_ceil(x_stride);
        let lines = case.height.saturating_sub(y_offset).div_ceil(y_stride);
        if samples == 0 || lines == 0 {
            continue;
        }
        for line in 0..lines {
            compressor.write_all(&[0])?;
            let y = y_offset + line * y_stride;
            for sample in 0..samples {
                let x = x_offset + sample * x_stride;
                compressor.write_all(&adam7_pixel(case, x, y)?)?;
            }
        }
    }
    let compressed = compressor.finish()?;
    let mut output = BufWriter::new(File::create(path)?);
    output.write_all(b"\x89PNG\r\n\x1a\n")?;
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&case.width.to_be_bytes());
    ihdr.extend_from_slice(&case.height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 1]);
    write_chunk(&mut output, *b"IHDR", &ihdr)?;
    write_chunk(&mut output, *b"IDAT", &compressed)?;
    write_chunk(&mut output, *b"IEND", &[])?;
    output.flush()?;
    Ok(())
}

fn adam7_pixel(case: Case, x: u32, y: u32) -> Result<[u8; 4]> {
    match case.pattern {
        Pattern::Blank => Ok([32, 96, 160, 255]),
        Pattern::Gradient => {
            let red = scale_to_byte(x, case.width);
            let green = scale_to_byte(y, case.height);
            Ok([red, green, red.wrapping_add(green) / 2, 255])
        }
        Pattern::Noise => Err("Adam7 noise generation is not configured".into()),
    }
}

fn write_chunk(output: &mut impl Write, chunk_type: [u8; 4], data: &[u8]) -> Result<()> {
    output.write_all(&u32::try_from(data.len())?.to_be_bytes())?;
    output.write_all(&chunk_type)?;
    output.write_all(data)?;
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(&chunk_type);
    crc_input.extend_from_slice(data);
    output.write_all(&crc32(&crc_input).to_be_bytes())?;
    Ok(())
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

fn write_png(path: &Path, case: Case) -> Result<()> {
    let output = BufWriter::new(File::create(path)?);
    let mut encoder = png::Encoder::new(output, case.width, case.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?.into_stream_writer()?;
    let row_bytes = usize::try_from(case.width)?
        .checked_mul(4)
        .ok_or("row size overflow")?;
    let mut row = vec![0_u8; row_bytes];
    let mut state = 0x9e37_79b9_u32;

    for y in 0..case.height {
        fill_row(
            &mut row,
            case.width,
            y,
            case.height,
            case.pattern,
            &mut state,
        );
        writer.write_all(&row)?;
    }
    writer.finish()?;
    Ok(())
}

fn fill_row(row: &mut [u8], width: u32, y: u32, height: u32, pattern: Pattern, state: &mut u32) {
    for x in 0..width {
        let offset = x as usize * 4;
        let pixel = match pattern {
            Pattern::Blank => [32, 96, 160, 255],
            Pattern::Gradient => {
                let red = scale_to_byte(x, width);
                let green = scale_to_byte(y, height);
                [red, green, red.wrapping_add(green) / 2, 255]
            }
            Pattern::Noise => {
                *state ^= *state << 13;
                *state ^= *state >> 17;
                *state ^= *state << 5;
                let bytes = state.to_le_bytes();
                [bytes[0], bytes[1], bytes[2], 255]
            }
        };
        row[offset..offset + 4].copy_from_slice(&pixel);
    }
}

fn scale_to_byte(position: u32, length: u32) -> u8 {
    if length <= 1 {
        0
    } else {
        ((u64::from(position) * 255) / u64::from(length - 1)) as u8
    }
}

fn run_method(method: &str, input: &Path, output: &Path, max_dimension: u32) -> Result<()> {
    if max_dimension == 0 {
        return Err("max-dimension must be positive".into());
    }
    let input_bytes = fs::read(input)?;
    let started = Instant::now();
    let (source_width, source_height, output_width, output_height, output_bytes) = match method {
        "streamthumb" | "streamthumb-png" => run_streamthumb(
            &input_bytes,
            output,
            max_dimension,
            OutputFormat::Png,
            Fit::Contain,
        )?,
        "streamthumb-jpeg" => run_streamthumb(
            &input_bytes,
            output,
            max_dimension,
            OutputFormat::Jpeg,
            Fit::Contain,
        )?,
        "streamthumb-cover-png" => run_streamthumb(
            &input_bytes,
            output,
            max_dimension,
            OutputFormat::Png,
            Fit::Cover,
        )?,
        "streamthumb-cover-jpeg" => run_streamthumb(
            &input_bytes,
            output,
            max_dimension,
            OutputFormat::Jpeg,
            Fit::Cover,
        )?,
        "image-rs" => run_image_rs(&input_bytes, output, max_dimension)?,
        _ => return Err(format!("unsupported benchmark method: {method}").into()),
    };
    let elapsed = started.elapsed();
    println!(
        "{{\"method\":\"{}\",\"input\":\"{}\",\"encoded_input_bytes\":{},\"source_width\":{},\"source_height\":{},\"output_width\":{},\"output_height\":{},\"runtime_ms\":{:.3},\"output_bytes\":{}}}",
        method,
        json_escape(&input.display().to_string()),
        input_bytes.len(),
        source_width,
        source_height,
        output_width,
        output_height,
        elapsed.as_secs_f64() * 1_000.0,
        output_bytes
    );
    Ok(())
}

fn run_streamthumb(
    input: &[u8],
    output: &Path,
    max_dimension: u32,
    format: OutputFormat,
    fit: Fit,
) -> Result<(u32, u32, u32, u32, u64)> {
    let mut options = ThumbnailOptions {
        max_width: max_dimension,
        max_height: max_dimension,
        output: format,
        fit,
        ..ThumbnailOptions::default()
    };
    options.limits.max_input_bytes = u64::try_from(input.len())?.saturating_add(1);
    options.limits.max_working_memory_bytes = 512 * 1024 * 1024;
    let result = thumbnail_png(input, &options)?;
    match result {
        ThumbnailOutput::Encoded {
            bytes,
            width,
            height,
            ..
        } => {
            fs::write(output, &bytes)?;
            let (source_width, source_height) = png_dimensions(input)?;
            Ok((
                source_width,
                source_height,
                width,
                height,
                u64::try_from(bytes.len())?,
            ))
        }
        _ => Err("streamthumb returned an unexpected output format".into()),
    }
}

fn run_image_rs(
    input: &[u8],
    output: &Path,
    max_dimension: u32,
) -> Result<(u32, u32, u32, u32, u64)> {
    let decoded = image::load_from_memory_with_format(input, ImageFormat::Png)?.into_rgba8();
    let source_width = decoded.width();
    let source_height = decoded.height();
    let dimensions = contain_dimensions(
        Dimensions::new(source_width, source_height)?,
        Dimensions::new(max_dimension, max_dimension)?,
        false,
    )?;
    let resized = image::imageops::resize(
        &decoded,
        dimensions.width,
        dimensions.height,
        FilterType::Triangle,
    );
    DynamicImage::ImageRgba8(resized).save_with_format(output, ImageFormat::Png)?;
    let output_bytes = fs::metadata(output)?.len();
    Ok((
        source_width,
        source_height,
        dimensions.width,
        dimensions.height,
        output_bytes,
    ))
}

fn png_dimensions(input: &[u8]) -> Result<(u32, u32)> {
    let decoder = png::Decoder::new(std::io::Cursor::new(input));
    let reader = decoder.read_info()?;
    Ok((reader.info().width, reader.info().height))
}

fn json_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
