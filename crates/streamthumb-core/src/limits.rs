/// Hard resource limits applied before thumbnail processing starts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    pub max_input_bytes: u64,
    pub max_width: u32,
    pub max_height: u32,
    pub max_pixels: u64,
    pub max_output_width: u32,
    pub max_output_height: u32,
    pub max_output_pixels: u64,
    pub max_working_memory_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_input_bytes: 64 * 1024 * 1024,
            max_width: 100_000,
            max_height: 100_000,
            max_pixels: 500_000_000,
            max_output_width: 8_192,
            max_output_height: 8_192,
            max_output_pixels: 16_777_216,
            max_working_memory_bytes: 32 * 1024 * 1024,
        }
    }
}
