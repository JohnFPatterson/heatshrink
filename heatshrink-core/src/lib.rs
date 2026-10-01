//! Safe port of the heatshrink LZSS streaming codec.
//!
//! Behavior matches `heatshrink_encoder.c` and `heatshrink_decoder.c` for the
//! dynamic configuration (the default in `heatshrink_config.h`) and for the
//! static configuration (`HEATSHRINK_DYNAMIC_ALLOC` 0) via
//! [`Encoder::from_static_config`] and [`Decoder::from_static_config`].

#![forbid(unsafe_code)]

mod config;
mod decoder;
mod encoder;
mod error;

pub use config::{
    LITERAL_MARKER, MAX_WINDOW_BITS, MIN_LOOKAHEAD_BITS, MIN_WINDOW_BITS, STATIC_INPUT_BUFFER_SIZE,
    STATIC_LOOKAHEAD_BITS, STATIC_WINDOW_BITS, USE_INDEX,
};
pub use decoder::{Decoder, DecoderState, SinkFull};
pub use encoder::{Encoder, EncoderState, PollBytes};
pub use error::Error;

/// White-box view used by the FFI crate. Field layout of the C structs is
/// recreated there; this module exposes the scalar state the C objects hold.
#[doc(hidden)]
pub mod internals {
    pub use crate::decoder::DecoderState;
    pub use crate::encoder::EncoderState;
}
