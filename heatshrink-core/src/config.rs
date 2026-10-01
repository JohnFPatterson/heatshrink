//! Constants from `heatshrink_common.h` and the static block of `heatshrink_config.h`.

/// Minimum window size in bits (inclusive).
pub const MIN_WINDOW_BITS: u8 = 4;
/// Maximum window size in bits (inclusive).
pub const MAX_WINDOW_BITS: u8 = 15;
/// Minimum lookahead size in bits (inclusive).
pub const MIN_LOOKAHEAD_BITS: u8 = 3;

pub const LITERAL_MARKER: u8 = 0x01;
pub const BACKREF_MARKER: u8 = 0x00;

/// `HEATSHRINK_STATIC_INPUT_BUFFER_SIZE` in the static build.
pub const STATIC_INPUT_BUFFER_SIZE: u16 = 32;
/// `HEATSHRINK_STATIC_WINDOW_BITS` in the static build.
pub const STATIC_WINDOW_BITS: u8 = 8;
/// `HEATSHRINK_STATIC_LOOKAHEAD_BITS` in the static build.
pub const STATIC_LOOKAHEAD_BITS: u8 = 4;

/// Shipped `heatshrink_config.h` sets `HEATSHRINK_USE_INDEX` to 1.
/// The non-index scan is not built by either in-scope configuration.
pub const USE_INDEX: bool = true;

pub(crate) fn params_ok(window_sz2: u8, lookahead_sz2: u8) -> bool {
    (MIN_WINDOW_BITS..=MAX_WINDOW_BITS).contains(&window_sz2)
        && lookahead_sz2 >= MIN_LOOKAHEAD_BITS
        && lookahead_sz2 < window_sz2
}

pub(crate) fn decoder_params_ok(input_buffer_size: u16, window_sz2: u8, lookahead_sz2: u8) -> bool {
    input_buffer_size != 0 && params_ok(window_sz2, lookahead_sz2)
}
