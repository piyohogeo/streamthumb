use crate::Limits;

/// The strategy used to place the image within the requested output box.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Fit {
    #[default]
    Contain,
}

/// The resampling filter used to create the thumbnail.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Filter {
    #[default]
    Area,
}

/// The representation returned by the thumbnail operation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OutputFormat {
    /// A complete encoded PNG image.
    #[default]
    Png,
    /// A complete baseline sequential JPEG image.
    Jpeg,
    /// Tightly packed straight-alpha RGBA8 pixels.
    Rgba,
}

/// Options shared by native and WebAssembly thumbnail APIs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThumbnailOptions {
    pub max_width: u32,
    pub max_height: u32,
    pub fit: Fit,
    pub allow_upscale: bool,
    pub filter: Filter,
    pub limits: Limits,
    pub output: OutputFormat,
}

impl Default for ThumbnailOptions {
    fn default() -> Self {
        Self {
            max_width: 512,
            max_height: 512,
            fit: Fit::Contain,
            allow_upscale: false,
            filter: Filter::Area,
            limits: Limits::default(),
            output: OutputFormat::Png,
        }
    }
}
