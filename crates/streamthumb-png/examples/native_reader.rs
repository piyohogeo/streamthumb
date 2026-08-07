use std::env;
use std::error::Error;
use std::fs::File;
use std::path::Path;

use streamthumb_core::{Fit, OutputFormat, ThumbnailOptions};
use streamthumb_png::{thumbnail_jpeg_from_reader_to_writer, thumbnail_png_from_reader_to_writer};

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args_os().skip(1);
    let input_path = arguments
        .next()
        .ok_or("usage: native_reader <input.png> <output.png|output.jpg> [contain|cover]")?;
    let output_path = arguments
        .next()
        .ok_or("usage: native_reader <input.png> <output.png|output.jpg> [contain|cover]")?;
    let fit = match arguments.next().as_deref() {
        None => Fit::Contain,
        Some(value) if value == "contain" => Fit::Contain,
        Some(value) if value == "cover" => Fit::Cover,
        Some(_) => return Err("fit must be contain or cover".into()),
    };
    if arguments.next().is_some() {
        return Err("too many arguments".into());
    }

    let output_format = match Path::new(&output_path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => OutputFormat::Png,
        Some("jpg" | "jpeg") => OutputFormat::Jpeg,
        _ => return Err("output extension must be .png, .jpg, or .jpeg".into()),
    };
    let mut options = ThumbnailOptions {
        max_width: 512,
        max_height: 512,
        output: output_format,
        fit,
        ..ThumbnailOptions::default()
    };
    options.limits.max_input_bytes = 64 * 1024 * 1024;
    options.limits.max_working_memory_bytes = 128 * 1024 * 1024;

    let input = File::open(input_path)?;
    let output = File::create(output_path)?;
    let info = match output_format {
        OutputFormat::Png => thumbnail_png_from_reader_to_writer(input, &options, output)?,
        OutputFormat::Jpeg => thumbnail_jpeg_from_reader_to_writer(input, &options, output)?,
        OutputFormat::Rgba => unreachable!("the example accepts only encoded output extensions"),
    };
    println!(
        "created a {}x{} {} thumbnail",
        info.width,
        info.height,
        match info.format {
            OutputFormat::Png => "PNG",
            OutputFormat::Jpeg => "JPEG",
            OutputFormat::Rgba => "RGBA",
        }
    );
    Ok(())
}
