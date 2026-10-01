//! Case-for-case port of `test_heatshrink_static.c`.
//!
//! The C file builds with `HEATSHRINK_DYNAMIC_ALLOC` 0, so the encoder and
//! decoder are caller-owned objects with window 8, lookahead 4, and a 32-byte
//! decoder input buffer. Null-rejection and `alloc`/`free` do not exist in
//! that build.

#[path = "common/mod.rs"]
mod common;

use common::{compress_pair, fill_pseudorandom};
use heatshrink_core::{
    Decoder, Encoder, STATIC_INPUT_BUFFER_SIZE, STATIC_LOOKAHEAD_BITS, STATIC_WINDOW_BITS,
};

#[test]
fn static_constants_match_heatshrink_config() {
    assert_eq!(STATIC_INPUT_BUFFER_SIZE, 32);
    assert_eq!(STATIC_WINDOW_BITS, 8);
    assert_eq!(STATIC_LOOKAHEAD_BITS, 4);
    let enc = Encoder::from_static_config();
    let dec = Decoder::from_static_config();
    assert_eq!(enc.window_bits(), 8);
    assert_eq!(enc.lookahead_bits(), 4);
    assert_eq!(dec.window_bits(), 8);
    assert_eq!(dec.lookahead_bits(), 4);
    assert_eq!(dec.input_buffer_size(), 32);
}

#[test]
fn pseudorandom_data_should_match() {
    for exp in 0..16 {
        let size = 1usize << exp;
        for seed in 1..=100u32 {
            let input = fill_pseudorandom(size, seed);
            let mut enc = Encoder::from_static_config();
            let mut dec = Decoder::from_static_config();
            enc.reset();
            dec.reset();
            compress_pair(enc, dec, &input);
        }
    }
}
