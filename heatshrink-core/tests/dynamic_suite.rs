//! Case-for-case port of `test_heatshrink_dynamic.c` (libtheft suite excluded).

#[path = "common/mod.rs"]
mod common;

use common::{compress_and_expand, encode_finished, fill_pseudorandom};
use heatshrink_core::{Decoder, Encoder, Error};

#[test]
fn encoder_alloc_should_reject_invalid_arguments() {
    assert!(Encoder::try_new(heatshrink_core::MIN_WINDOW_BITS - 1, 8).is_err());
    assert!(Encoder::try_new(heatshrink_core::MAX_WINDOW_BITS + 1, 8).is_err());
    assert!(Encoder::try_new(8, heatshrink_core::MIN_LOOKAHEAD_BITS - 1).is_err());
    assert!(Encoder::try_new(8, 9).is_err());
}

#[test]
fn encoder_sink_should_accept_input_when_it_will_fit() {
    let mut hse = Encoder::try_new(8, 7).unwrap();
    let input = vec![b'*'; 256];
    assert_eq!(hse.sink(&input).unwrap(), 256);
}

#[test]
fn encoder_sink_should_accept_partial_input_when_some_will_fit() {
    let mut hse = Encoder::try_new(8, 7).unwrap();
    let input = vec![b'*'; 512];
    assert_eq!(hse.sink(&input).unwrap(), 256);
}

#[test]
fn encoder_poll_should_indicate_when_no_input_is_provided() {
    let mut hse = Encoder::try_new(8, 7).unwrap();
    let mut output = [0u8; 512];
    let polled = hse.poll(&mut output).unwrap();
    assert!(!polled.more && polled.n == 0);
}

#[test]
fn encoder_should_emit_data_without_repetitions_as_literal_sequence() {
    let got = encode_finished(8, 7, &[0, 1, 2, 3, 4]);
    assert_eq!(got, [0x80, 0x40, 0x60, 0x50, 0x38, 0x20]);
}

#[test]
fn encoder_should_emit_series_of_same_byte_as_literal_then_backref() {
    let got = encode_finished(8, 7, b"aaaaa");
    assert_eq!(got, [0xb0, 0x80, 0x01, 0x80]);
}

#[test]
fn encoder_poll_should_detect_repeated_substring() {
    let got = encode_finished(8, 3, b"abcdabcd");
    assert_eq!(got, [0xb0, 0xd8, 0xac, 0x76, 0x40, 0x1b]);
}

#[test]
fn encoder_poll_should_detect_repeated_substring_and_preserve_trailing_literal() {
    let got = encode_finished(8, 3, b"abcdabcde");
    assert_eq!(got, [0xb0, 0xd8, 0xac, 0x76, 0x40, 0x1b, 0xb2, 0x80]);
}

#[test]
fn decoder_alloc_should_reject_excessively_small_window() {
    assert!(matches!(
        Decoder::try_new(256, heatshrink_core::MIN_WINDOW_BITS - 1, 4),
        Err(Error::InvalidParams)
    ));
}

#[test]
fn decoder_alloc_should_reject_zero_byte_input_buffer() {
    assert!(Decoder::try_new(
        0,
        heatshrink_core::MIN_WINDOW_BITS,
        heatshrink_core::MIN_WINDOW_BITS - 1
    )
    .is_err());
}

#[test]
fn decoder_alloc_should_reject_lookahead_equal_to_window_size() {
    assert!(Decoder::try_new(
        0,
        heatshrink_core::MIN_WINDOW_BITS,
        heatshrink_core::MIN_WINDOW_BITS
    )
    .is_err());
}

#[test]
fn decoder_alloc_should_reject_lookahead_greater_than_window_size() {
    assert!(Decoder::try_new(
        0,
        heatshrink_core::MIN_WINDOW_BITS,
        heatshrink_core::MIN_WINDOW_BITS + 1
    )
    .is_err());
}

#[test]
fn decoder_sink_should_reject_excessively_large_input() {
    let mut hsd = Decoder::try_new(
        1,
        heatshrink_core::MIN_WINDOW_BITS,
        heatshrink_core::MIN_WINDOW_BITS - 1,
    )
    .unwrap();
    let input = [0u8, 1, 2, 3, 4, 5];
    assert_eq!(hsd.sink(&input).unwrap(), 1);
    assert!(hsd.sink(&input[1..]).is_err());
}

#[test]
fn decoder_sink_should_sink_data_when_preconditions_hold() {
    let mut hsd = Decoder::try_new(
        256,
        heatshrink_core::MIN_WINDOW_BITS,
        heatshrink_core::MIN_WINDOW_BITS - 1,
    )
    .unwrap();
    let input = [0u8, 1, 2, 3, 4, 5];
    assert_eq!(hsd.sink(&input).unwrap(), 6);
    assert_eq!(hsd.input_size(), 6);
    assert_eq!(hsd.input_index(), 0);
}

#[test]
fn decoder_poll_should_return_empty_if_empty() {
    let mut hsd = Decoder::try_new(
        256,
        heatshrink_core::MIN_WINDOW_BITS,
        heatshrink_core::MIN_WINDOW_BITS - 1,
    )
    .unwrap();
    let mut output = [0u8; 256];
    let polled = hsd.poll(&mut output).unwrap();
    assert!(!polled.more);
}

#[test]
fn decoder_poll_should_expand_short_literal() {
    let mut hsd = Decoder::try_new(256, 7, 3).unwrap();
    let input = [0xb3, 0x5b, 0xed, 0xe0];
    hsd.sink(&input).unwrap();
    let mut output = [0u8; 4];
    let polled = hsd.poll(&mut output).unwrap();
    assert!(!polled.more);
    assert_eq!(polled.n, 3);
    assert_eq!(&output[..3], b"foo");
}

#[test]
fn decoder_poll_should_expand_short_literal_and_backref() {
    let mut hsd = Decoder::try_new(256, 7, 6).unwrap();
    let input = [0xb3, 0x5b, 0xed, 0xe0, 0x41, 0x00];
    hsd.sink(&input).unwrap();
    let mut output = [0u8; 6];
    let polled = hsd.poll(&mut output).unwrap();
    assert_eq!(polled.n, 6);
    assert_eq!(&output, b"foofoo");
}

#[test]
fn decoder_poll_should_expand_short_self_overlapping_backref() {
    let mut hsd = Decoder::try_new(256, 8, 7).unwrap();
    let input = [0xb0, 0x80, 0x01, 0x80];
    hsd.sink(&input).unwrap();
    let mut output = [0u8; 6];
    let polled = hsd.poll(&mut output).unwrap();
    assert_eq!(polled.n, 5);
    assert_eq!(&output[..5], b"aaaaa");
}

#[test]
fn decoder_poll_should_suspend_if_out_of_space_in_output_buffer_during_literal_expansion() {
    let mut hsd = Decoder::try_new(256, 7, 6).unwrap();
    let input = [0xb3, 0x5b, 0xed, 0xe0, 0x40, 0x80];
    hsd.sink(&input).unwrap();
    let mut output = [0u8; 1];
    let polled = hsd.poll(&mut output).unwrap();
    assert!(polled.more);
    assert_eq!(polled.n, 1);
    assert_eq!(output[0], b'f');
}

#[test]
fn decoder_poll_should_suspend_if_out_of_space_in_output_buffer_during_backref_expansion() {
    let mut hsd = Decoder::try_new(256, 7, 6).unwrap();
    let input = [0xb3, 0x5b, 0xed, 0xe0, 0x40, 0x80];
    hsd.sink(&input).unwrap();
    let mut output = [0u8; 4];
    let polled = hsd.poll(&mut output).unwrap();
    assert!(polled.more);
    assert_eq!(polled.n, 4);
    assert_eq!(&output, b"foof");
}

#[test]
fn decoder_poll_should_expand_short_literal_and_backref_when_fed_input_byte_by_byte() {
    let mut hsd = Decoder::try_new(256, 7, 6).unwrap();
    let input = [0xb3, 0x5b, 0xed, 0xe0, 0x41, 0x00];
    for byte in input {
        assert_eq!(hsd.sink(&[byte]).unwrap(), 1);
    }
    let _ = hsd.finish();
    let mut output = [0u8; 7];
    let polled = hsd.poll(&mut output).unwrap();
    assert_eq!(polled.n, 6);
    assert!(!polled.more);
    assert_eq!(&output[..6], b"foofoo");
}

#[test]
fn decoder_finish_should_note_when_done() {
    let mut hsd = Decoder::try_new(256, 7, 6).unwrap();
    let input = [0xb3, 0x5b, 0xed, 0xe0, 0x41, 0x00];
    hsd.sink(&input).unwrap();
    let mut output = [0u8; 7];
    let polled = hsd.poll(&mut output).unwrap();
    assert!(!polled.more);
    assert_eq!(&output[..6], b"foofoo");
    assert!(hsd.finish());
}

#[test]
fn gen() {
    let _ = encode_finished(8, 7, b"aaaaa");
}

#[test]
fn decoder_should_not_get_stuck_with_finish_yielding_more_but_0_bytes_output_from_poll() {
    let mut input = vec![0xffu8; 512];
    let mut output = [0u8; 1024];
    let mut hsd = Decoder::try_new(256, 8, 4).unwrap();
    for byte in 0u16..256 {
        for i in 1..512 {
            input[i] = byte as u8;
            hsd.reset();
            // The C test asserts only HSDR_SINK_OK. Buffers shorter than `i`
            // accept a prefix and still return OK.
            hsd.sink(&input[..i]).expect("sink");
            let polled = hsd.poll(&mut output).unwrap();
            assert!(!polled.more);
            assert!(hsd.finish());
            input[i] = 0xff;
        }
    }
}

#[test]
fn data_without_duplication_should_match() {
    compress_and_expand(b"abcdefghijklmnopqrstuvwxyz", 8, 3, 256);
}

#[test]
fn data_with_simple_repetition_should_compress_and_decompress_properly() {
    compress_and_expand(b"abcabcdabcdeabcdefabcdefgabcdefgh", 8, 3, 256);
}

fn tiny_roundtrip(input: &[u8]) {
    let mut hse = Encoder::try_new(8, 3).unwrap();
    let mut hsd = Decoder::try_new(256, 8, 3).unwrap();
    for byte in input {
        hse.sink(&[*byte]).unwrap();
    }
    assert!(!hse.finish());
    let mut comp = [0u8; 60];
    let mut packed = 0usize;
    loop {
        let got = hse.poll(&mut comp[packed..packed + 1]).unwrap();
        packed += got.n;
        if hse.finish() {
            break;
        }
    }
    for i in 0..packed {
        hsd.sink(&comp[i..i + 1]).unwrap();
    }
    let mut plain = [0u8; 60];
    for i in 0..input.len() {
        hsd.poll(&mut plain[i..i + 1]).unwrap();
    }
    assert_eq!(&plain[..input.len()], input);
}

#[test]
fn data_without_duplication_should_match_with_absurdly_tiny_buffers() {
    tiny_roundtrip(b"abcdefghijklmnopqrstuvwxyz");
}

#[test]
fn data_with_simple_repetition_should_match_with_absurdly_tiny_buffers() {
    tiny_roundtrip(b"abcabcdabcdeabcdefabcdefgabcdefgh");
}

#[test]
fn small_input_buffer_should_not_impact_decoder_correctness() {
    let input: Vec<u8> = (0..5).map(|i| b'a' + (i % 26)).collect();
    compress_and_expand(&input, 8, 3, 5);
}

#[test]
fn regression_backreference_counters_should_not_roll_over() {
    let input = fill_pseudorandom(337, 3);
    compress_and_expand(&input, 8, 3, 64);
}

#[test]
fn regression_index_fail() {
    let input = fill_pseudorandom(507, 3);
    compress_and_expand(&input, 8, 3, 64);
}

#[test]
fn sixty_four_k() {
    let input = fill_pseudorandom(64 * 1024, 1);
    compress_and_expand(&input, 8, 3, 64);
}

#[test]
fn pseudorandom_single_byte_lookahead_fuzz() {
    for lookahead in 3..8u8 {
        for exp in 0..17 {
            let size = 1usize << exp;
            for ibs in [32u16, 64, 128, 256, 512, 1024, 2048, 4096, 8192] {
                for seed in 1..=10u32 {
                    let input = fill_pseudorandom(size, seed);
                    compress_and_expand(&input, 8, lookahead, ibs);
                }
            }
        }
    }
}

#[test]
fn pseudorandom_multi_byte_window_fuzz() {
    for lookahead in 6..9u8 {
        for exp in 0..17 {
            let size = 1usize << exp;
            for ibs in [32u16, 64, 128, 256, 512, 1024, 2048, 4096, 8192] {
                for seed in 1..=10u32 {
                    let input = fill_pseudorandom(size, seed);
                    compress_and_expand(&input, 11, lookahead, ibs);
                }
            }
        }
    }
}
