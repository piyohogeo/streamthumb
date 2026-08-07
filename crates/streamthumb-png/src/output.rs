use streamthumb_core::{OutputFormat, RgbaImage, ThumbnailInfo};

/// A thumbnail returned as raw pixels or an encoded image.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ThumbnailOutput {
    Encoded {
        bytes: Vec<u8>,
        width: u32,
        height: u32,
        mime_type: &'static str,
        format: OutputFormat,
    },
    Rgba {
        pixels: Vec<u8>,
        width: u32,
        height: u32,
    },
}

impl ThumbnailOutput {
    /// Returns metadata shared by both output representations.
    pub const fn info(&self) -> ThumbnailInfo {
        match self {
            Self::Encoded {
                width,
                height,
                format,
                ..
            } => ThumbnailInfo {
                width: *width,
                height: *height,
                format: *format,
            },
            Self::Rgba { width, height, .. } => ThumbnailInfo {
                width: *width,
                height: *height,
                format: OutputFormat::Rgba,
            },
        }
    }
}

impl From<RgbaImage> for ThumbnailOutput {
    fn from(image: RgbaImage) -> Self {
        Self::Rgba {
            pixels: image.pixels,
            width: image.dimensions.width,
            height: image.dimensions.height,
        }
    }
}
