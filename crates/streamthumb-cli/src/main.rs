use std::{
    env,
    error::Error,
    fmt, fs,
    fs::{File, OpenOptions},
    io::BufWriter,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::atomic::{AtomicU32, Ordering},
};

use streamthumb_core::{Fit, OutputFormat, ThumbnailOptions};
use streamthumb_png::{
    JpegOptions, JpegSubsampling, PngColorMode, PngCompression, PngFilter, PngOptions,
    thumbnail_jpeg_to_writer_with_options, thumbnail_png_to_writer_with_encoder_options,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("streamthumb: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), CliError> {
    let config = Config::parse(env::args().skip(1))?;
    let input_metadata = fs::metadata(&config.input)?;
    if input_metadata.len() > config.options.limits.max_input_bytes {
        return Err(CliError::Message(format!(
            "input contains {} bytes, exceeding the configured {}-byte limit",
            input_metadata.len(),
            config.options.limits.max_input_bytes
        )));
    }

    let input = fs::read(&config.input)?;
    let mut staged = StagedOutput::create(&config.output)?;
    let output = BufWriter::new(staged.take_file()?);
    match config.options.output {
        OutputFormat::Png => {
            thumbnail_png_to_writer_with_encoder_options(
                &input,
                &config.options,
                &config.png_options,
                output,
            )?;
        }
        OutputFormat::Jpeg => {
            thumbnail_jpeg_to_writer_with_options(
                &input,
                &config.options,
                &config.jpeg_options,
                output,
            )?;
        }
        OutputFormat::Rgba => unreachable!("the CLI does not expose raw RGBA output"),
    }
    staged.commit(&config.output)?;
    Ok(())
}

static STAGING_SEQUENCE: AtomicU32 = AtomicU32::new(0);

struct StagedOutput {
    path: Option<PathBuf>,
    file: Option<File>,
}

impl StagedOutput {
    fn create(destination: &Path) -> Result<Self, CliError> {
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        let file_name = destination
            .file_name()
            .ok_or_else(|| CliError::Message("output path must name a file".to_owned()))?;
        for _ in 0..1_000 {
            let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(
                ".{}.streamthumb-{}-{sequence}.tmp",
                file_name.to_string_lossy(),
                std::process::id(),
            ));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        path: Some(path),
                        file: Some(file),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(CliError::Message(
            "could not allocate a unique staging output path".to_owned(),
        ))
    }

    fn take_file(&mut self) -> Result<File, CliError> {
        self.file.take().ok_or_else(|| {
            CliError::Message("internal error: staging output file was already taken".to_owned())
        })
    }

    fn commit(mut self, destination: &Path) -> Result<(), CliError> {
        let staging = self.path.take().ok_or_else(|| {
            CliError::Message("internal error: staging output path is missing".to_owned())
        })?;
        match fs::rename(&staging, destination) {
            Ok(()) => return Ok(()),
            Err(first_error) => {
                if !matches!(
                    fs::symlink_metadata(destination),
                    Ok(metadata) if metadata.file_type().is_file()
                ) {
                    self.path = Some(staging);
                    return Err(first_error.into());
                }
            }
        }

        let backup = unique_unused_path(destination, "backup")?;
        fs::rename(destination, &backup)?;
        if let Err(error) = fs::rename(&staging, destination) {
            let _ = fs::rename(&backup, destination);
            self.path = Some(staging);
            return Err(error.into());
        }
        if let Err(error) = fs::remove_file(&backup) {
            return Err(error.into());
        }
        Ok(())
    }
}

impl Drop for StagedOutput {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(path);
        }
    }
}

fn unique_unused_path(destination: &Path, role: &str) -> Result<PathBuf, CliError> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .ok_or_else(|| CliError::Message("output path must name a file".to_owned()))?;
    for _ in 0..1_000 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{}.streamthumb-{role}-{}-{sequence}.tmp",
            file_name.to_string_lossy(),
            std::process::id(),
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(CliError::Message(format!(
        "could not allocate a unique {role} path"
    )))
}

#[derive(Debug, Eq, PartialEq)]
struct Config {
    input: PathBuf,
    output: PathBuf,
    options: ThumbnailOptions,
    png_options: PngOptions,
    jpeg_options: JpegOptions,
}

impl Config {
    fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self, CliError> {
        let mut arguments = arguments.into_iter();
        let input = arguments.next().ok_or_else(usage)?;
        let output = PathBuf::from(arguments.next().ok_or_else(usage)?);
        let mut options = ThumbnailOptions {
            output: output_format_from_path(&output)?,
            ..ThumbnailOptions::default()
        };
        let mut png_options = PngOptions::default();
        let mut jpeg_options = JpegOptions::default();
        let mut png_options_configured = false;
        let mut jpeg_options_configured = false;

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--max-width" => {
                    options.max_width = parse_dimension(arguments.next(), "--max-width")?;
                }
                "--max-height" => {
                    options.max_height = parse_dimension(arguments.next(), "--max-height")?;
                }
                "--allow-upscale" => options.allow_upscale = true,
                "--fit" => {
                    let value = required_value(arguments.next(), "--fit")?;
                    options.fit = match value.as_str() {
                        "contain" => Fit::Contain,
                        "cover" => Fit::Cover,
                        _ => return Err(invalid_choice("--fit", &value)),
                    };
                }
                "--format" => {
                    let value = required_value(arguments.next(), "--format")?;
                    options.output = match value.as_str() {
                        "png" => OutputFormat::Png,
                        "jpeg" | "jpg" => OutputFormat::Jpeg,
                        _ => return Err(invalid_choice("--format", &value)),
                    };
                }
                "--png-color" => {
                    png_options_configured = true;
                    let value = required_value(arguments.next(), "--png-color")?;
                    png_options.color = match value.as_str() {
                        "auto" => PngColorMode::Auto,
                        "rgba8" => PngColorMode::Rgba8,
                        "rgb8" => PngColorMode::Rgb8,
                        "grayscale-alpha8" => PngColorMode::GrayscaleAlpha8,
                        "grayscale8" => PngColorMode::Grayscale8,
                        _ => return Err(invalid_choice("--png-color", &value)),
                    };
                }
                "--png-compression" => {
                    png_options_configured = true;
                    let value = required_value(arguments.next(), "--png-compression")?;
                    png_options.compression = match value.as_str() {
                        "none" => PngCompression::NoCompression,
                        "fastest" => PngCompression::Fastest,
                        "fast" => PngCompression::Fast,
                        "balanced" => PngCompression::Balanced,
                        "high" => PngCompression::High,
                        _ => return Err(invalid_choice("--png-compression", &value)),
                    };
                }
                "--png-filter" => {
                    png_options_configured = true;
                    let value = required_value(arguments.next(), "--png-filter")?;
                    png_options.filter = match value.as_str() {
                        "default" => PngFilter::Default,
                        "none" => PngFilter::None,
                        "sub" => PngFilter::Sub,
                        "up" => PngFilter::Up,
                        "average" => PngFilter::Average,
                        "paeth" => PngFilter::Paeth,
                        "adaptive" => PngFilter::Adaptive,
                        "min-entropy" => PngFilter::MinEntropy,
                        _ => return Err(invalid_choice("--png-filter", &value)),
                    };
                }
                "--jpeg-quality" => {
                    jpeg_options_configured = true;
                    let value = required_value(arguments.next(), "--jpeg-quality")?;
                    jpeg_options.quality = parse_jpeg_quality(&value)?;
                }
                "--jpeg-background" => {
                    jpeg_options_configured = true;
                    let value = required_value(arguments.next(), "--jpeg-background")?;
                    jpeg_options.background = parse_background(&value)?;
                }
                "--jpeg-subsampling" => {
                    jpeg_options_configured = true;
                    let value = required_value(arguments.next(), "--jpeg-subsampling")?;
                    jpeg_options.subsampling = match value.as_str() {
                        "420" => JpegSubsampling::S420,
                        "422" => JpegSubsampling::S422,
                        "444" => JpegSubsampling::S444,
                        _ => return Err(invalid_choice("--jpeg-subsampling", &value)),
                    };
                }
                _ => {
                    return Err(CliError::Message(format!(
                        "unknown argument {argument:?}\n{}",
                        usage_text()
                    )));
                }
            }
        }

        if png_options_configured && options.output != OutputFormat::Png {
            return Err(CliError::Message(
                "PNG options require PNG output".to_owned(),
            ));
        }
        if jpeg_options_configured && options.output != OutputFormat::Jpeg {
            return Err(CliError::Message(
                "JPEG options require JPEG output".to_owned(),
            ));
        }

        Ok(Self {
            input: input.into(),
            output,
            options,
            png_options,
            jpeg_options,
        })
    }
}

fn output_format_from_path(path: &std::path::Path) -> Result<OutputFormat, CliError> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => Ok(OutputFormat::Png),
        Some("jpg" | "jpeg") => Ok(OutputFormat::Jpeg),
        _ => Err(CliError::Message(
            "output extension must be .png, .jpg, or .jpeg; use --format to override a supported extension"
                .to_owned(),
        )),
    }
}

fn parse_jpeg_quality(value: &str) -> Result<u8, CliError> {
    value
        .parse::<u8>()
        .ok()
        .filter(|quality| (1..=100).contains(quality))
        .ok_or_else(|| CliError::Message("--jpeg-quality must be from 1 through 100".to_owned()))
}

fn parse_background(value: &str) -> Result<[u8; 3], CliError> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CliError::Message(
            "--jpeg-background must be an RGB color such as #ffffff".to_owned(),
        ));
    }
    Ok([
        u8::from_str_radix(&hex[0..2], 16).expect("validated hexadecimal red channel"),
        u8::from_str_radix(&hex[2..4], 16).expect("validated hexadecimal green channel"),
        u8::from_str_radix(&hex[4..6], 16).expect("validated hexadecimal blue channel"),
    ])
}

fn required_value(value: Option<String>, flag: &str) -> Result<String, CliError> {
    value.ok_or_else(|| CliError::Message(format!("{flag} requires a value")))
}

fn invalid_choice(flag: &str, value: &str) -> CliError {
    CliError::Message(format!("invalid {flag} value {value:?}\n{}", usage_text()))
}

fn parse_dimension(value: Option<String>, flag: &str) -> Result<u32, CliError> {
    let value = value.ok_or_else(|| CliError::Message(format!("{flag} requires a value")))?;
    let dimension = value
        .parse::<u32>()
        .map_err(|_| CliError::Message(format!("invalid {flag} value {value:?}")))?;
    if dimension == 0 {
        return Err(CliError::Message(format!(
            "{flag} must be greater than zero"
        )));
    }
    Ok(dimension)
}

fn usage() -> CliError {
    CliError::Message(usage_text().to_owned())
}

const fn usage_text() -> &'static str {
    "usage: streamthumb <input.png> <output.png|output.jpg> [--format png|jpeg] [--max-width N] [--max-height N] [--fit contain|cover] [--allow-upscale] [--png-color MODE] [--png-compression LEVEL] [--png-filter FILTER] [--jpeg-quality 1..100] [--jpeg-background #rrggbb] [--jpeg-subsampling 420|422|444]"
}

#[derive(Debug)]
enum CliError {
    Io(std::io::Error),
    Thumbnail(streamthumb_png::Error),
    Message(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Thumbnail(error) => error.fmt(formatter),
            Self::Message(message) => formatter.write_str(message),
        }
    }
}

impl Error for CliError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Thumbnail(error) => Some(error),
            Self::Message(_) => None,
        }
    }
}

impl From<std::io::Error> for CliError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<streamthumb_png::Error> for CliError {
    fn from(error: streamthumb_png::Error) -> Self {
        Self::Thumbnail(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn test_directory(name: &str) -> PathBuf {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "streamthumb-cli-{name}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn parses_dimensions_and_upscale_flag() {
        let config = Config::parse(
            [
                "input.png",
                "output.png",
                "--max-width",
                "320",
                "--max-height",
                "200",
                "--fit",
                "cover",
                "--allow-upscale",
            ]
            .map(str::to_owned),
        )
        .unwrap();

        assert_eq!(config.input, PathBuf::from("input.png"));
        assert_eq!(config.output, PathBuf::from("output.png"));
        assert_eq!(config.options.max_width, 320);
        assert_eq!(config.options.max_height, 200);
        assert_eq!(config.options.fit, Fit::Cover);
        assert!(config.options.allow_upscale);
        assert_eq!(config.options.output, OutputFormat::Png);
        assert_eq!(config.png_options, PngOptions::default());
    }

    #[test]
    fn parses_png_encoder_options() {
        let config = Config::parse(
            [
                "input.png",
                "output.png",
                "--png-color",
                "auto",
                "--png-compression",
                "high",
                "--png-filter",
                "min-entropy",
            ]
            .map(str::to_owned),
        )
        .unwrap();

        assert_eq!(config.png_options.color, PngColorMode::Auto);
        assert_eq!(config.png_options.compression, PngCompression::High);
        assert_eq!(config.png_options.filter, PngFilter::MinEntropy);
    }

    #[test]
    fn infers_jpeg_and_parses_encoder_options() {
        let config = Config::parse(
            [
                "input.png",
                "output.jpg",
                "--jpeg-quality",
                "92",
                "--jpeg-background",
                "#0a64dc",
                "--jpeg-subsampling",
                "444",
            ]
            .map(str::to_owned),
        )
        .unwrap();

        assert_eq!(config.options.output, OutputFormat::Jpeg);
        assert_eq!(
            config.jpeg_options,
            JpegOptions {
                quality: 92,
                background: [10, 100, 220],
                subsampling: JpegSubsampling::S444,
            }
        );
    }

    #[test]
    fn validates_format_specific_options() {
        assert!(
            Config::parse(["input.png", "output.jpg", "--png-color", "rgb8"].map(str::to_owned))
                .is_err()
        );
        assert!(
            Config::parse(["input.png", "output.png", "--jpeg-quality", "85"].map(str::to_owned))
                .is_err()
        );
        assert!(
            Config::parse(["input.png", "output.jpg", "--jpeg-quality", "0"].map(str::to_owned))
                .is_err()
        );
        assert!(
            Config::parse(
                ["input.png", "output.jpg", "--jpeg-background", "xyz"].map(str::to_owned)
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_missing_invalid_and_unknown_arguments() {
        assert!(Config::parse(Vec::<String>::new()).is_err());
        assert!(Config::parse(["in", "out", "--max-width", "0"].map(str::to_owned)).is_err());
        assert!(Config::parse(["in", "out", "--unknown"].map(str::to_owned)).is_err());
        assert!(
            Config::parse(["in", "out", "--png-color", "indexed8"].map(str::to_owned)).is_err()
        );
        assert!(Config::parse(["in", "out", "--png-filter"].map(str::to_owned)).is_err());
        assert!(Config::parse(["in.png", "out.png", "--fit", "crop"].map(str::to_owned)).is_err());
    }

    #[test]
    fn abandoned_staging_output_preserves_the_destination() {
        let directory = test_directory("abandon");
        let destination = directory.join("thumbnail.png");
        fs::write(&destination, b"original").unwrap();
        let staging_path;
        {
            let mut staged = StagedOutput::create(&destination).unwrap();
            staging_path = staged.path.clone().unwrap();
            staged.take_file().unwrap().write_all(b"partial").unwrap();
        }

        assert_eq!(fs::read(&destination).unwrap(), b"original");
        assert!(!staging_path.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn committed_staging_output_replaces_the_destination() {
        let directory = test_directory("commit");
        let destination = directory.join("thumbnail.png");
        fs::write(&destination, b"original").unwrap();
        let mut staged = StagedOutput::create(&destination).unwrap();
        staged.take_file().unwrap().write_all(b"complete").unwrap();
        staged.commit(&destination).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"complete");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}
