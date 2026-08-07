/// PNG color representation written by the streaming encoder.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PngColorMode {
    /// Selects a lossless color representation from input metadata.
    Auto,
    /// Writes red, green, blue, and alpha channels.
    #[default]
    Rgba8,
    /// Writes red, green, and blue channels and discards alpha.
    Rgb8,
    /// Uses `(77R + 150G + 29B + 128) >> 8` and preserves alpha.
    GrayscaleAlpha8,
    /// Uses `(77R + 150G + 29B + 128) >> 8` and discards alpha.
    Grayscale8,
}

/// PNG compression speed and size tradeoff.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PngCompression {
    NoCompression,
    Fastest,
    Fast,
    #[default]
    Balanced,
    High,
}

/// PNG scanline filter strategy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PngFilter {
    /// Uses the filter selected by the compression preset.
    #[default]
    Default,
    None,
    Sub,
    Up,
    Average,
    Paeth,
    Adaptive,
    MinEntropy,
}

/// Settings used only when the requested output representation is PNG.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PngOptions {
    pub color: PngColorMode,
    pub compression: PngCompression,
    pub filter: PngFilter,
}

impl Default for PngOptions {
    fn default() -> Self {
        Self {
            color: PngColorMode::Rgba8,
            compression: PngCompression::Balanced,
            filter: PngFilter::Default,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_preserve_the_original_encoder_behavior() {
        assert_eq!(
            PngOptions::default(),
            PngOptions {
                color: PngColorMode::Rgba8,
                compression: PngCompression::Balanced,
                filter: PngFilter::Default,
            }
        );
    }
}
