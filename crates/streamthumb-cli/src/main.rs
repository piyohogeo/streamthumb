use std::{env, error::Error, fmt, fs, path::PathBuf, process::ExitCode};

use streamthumb_core::{OutputFormat, ThumbnailOptions};
use streamthumb_png::{
    PngColorMode, PngCompression, PngFilter, PngOptions, ThumbnailOutput,
    thumbnail_png_with_encoder_options,
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
    let output = thumbnail_png_with_encoder_options(&input, &config.options, &config.png_options)?;
    let ThumbnailOutput::Encoded { bytes, .. } = output else {
        return Err(CliError::Message(
            "internal error: CLI requested a non-PNG output".to_owned(),
        ));
    };
    fs::write(&config.output, bytes)?;
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
struct Config {
    input: PathBuf,
    output: PathBuf,
    options: ThumbnailOptions,
    png_options: PngOptions,
}

impl Config {
    fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self, CliError> {
        let mut arguments = arguments.into_iter();
        let input = arguments.next().ok_or_else(usage)?;
        let output = arguments.next().ok_or_else(usage)?;
        let mut options = ThumbnailOptions {
            output: OutputFormat::Png,
            ..ThumbnailOptions::default()
        };
        let mut png_options = PngOptions::default();

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--max-width" => {
                    options.max_width = parse_dimension(arguments.next(), "--max-width")?;
                }
                "--max-height" => {
                    options.max_height = parse_dimension(arguments.next(), "--max-height")?;
                }
                "--allow-upscale" => options.allow_upscale = true,
                "--png-color" => {
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
                _ => {
                    return Err(CliError::Message(format!(
                        "unknown argument {argument:?}\n{}",
                        usage_text()
                    )));
                }
            }
        }

        Ok(Self {
            input: input.into(),
            output: output.into(),
            options,
            png_options,
        })
    }
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
    "usage: streamthumb <input.png> <output.png> [--max-width N] [--max-height N] [--allow-upscale] [--png-color MODE] [--png-compression LEVEL] [--png-filter FILTER]"
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
                "--allow-upscale",
            ]
            .map(str::to_owned),
        )
        .unwrap();

        assert_eq!(config.input, PathBuf::from("input.png"));
        assert_eq!(config.output, PathBuf::from("output.png"));
        assert_eq!(config.options.max_width, 320);
        assert_eq!(config.options.max_height, 200);
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
    fn rejects_missing_invalid_and_unknown_arguments() {
        assert!(Config::parse(Vec::<String>::new()).is_err());
        assert!(Config::parse(["in", "out", "--max-width", "0"].map(str::to_owned)).is_err());
        assert!(Config::parse(["in", "out", "--unknown"].map(str::to_owned)).is_err());
        assert!(
            Config::parse(["in", "out", "--png-color", "indexed8"].map(str::to_owned)).is_err()
        );
        assert!(Config::parse(["in", "out", "--png-filter"].map(str::to_owned)).is_err());
    }
}
