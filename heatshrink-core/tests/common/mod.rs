//! Shared streaming helper matching `compress_and_expand_and_check`.

use heatshrink_core::{Decoder, Encoder};

pub fn fill_pseudorandom(size: usize, seed: u32) -> Vec<u8> {
    let mut rn: u64 = 9_223_372_036_854_775_783;
    let mut out = vec![0u8; size];
    for byte in &mut out {
        rn = rn
            .wrapping_mul(u64::from(seed))
            .wrapping_add(u64::from(seed));
        *byte = (rn % 26) as u8 + b'a';
    }
    out
}

#[allow(dead_code)]
pub fn compress_and_expand(input: &[u8], window: u8, lookahead: u8, input_buffer: u16) {
    let enc = Encoder::try_new(window, lookahead).expect("encoder alloc");
    let dec = Decoder::try_new(input_buffer, window, lookahead).expect("decoder alloc");
    compress_pair(enc, dec, input);
}

pub fn compress_pair(mut enc: Encoder, mut dec: Decoder, input: &[u8]) {
    let mut comp = vec![0u8; input.len() + input.len() / 2 + 4];
    let mut plain = vec![0u8; input.len() + input.len() / 2 + 4];
    let mut sunk = 0usize;
    let mut polled = 0usize;
    while sunk < input.len() {
        let n = enc.sink(&input[sunk..]).expect("encoder sink");
        sunk += n;
        if sunk == input.len() {
            assert!(!enc.finish(), "encoder finish should be MORE");
        }
        loop {
            let space = comp.len() - polled;
            let got = enc
                .poll(&mut comp[polled..polled + space])
                .expect("encoder poll");
            polled += got.n;
            if !got.more {
                break;
            }
            assert!(polled < comp.len(), "compression expanded too far");
        }
        if sunk == input.len() {
            assert!(enc.finish(), "encoder finish should be DONE");
        }
    }
    let compressed = polled;
    sunk = 0;
    polled = 0;
    while sunk < compressed {
        let n = dec.sink(&comp[sunk..compressed]).unwrap_or(0);
        sunk += n;
        if sunk == compressed {
            assert!(!dec.finish(), "decoder finish should be MORE");
        }
        loop {
            let space = plain.len() - polled;
            let got = dec
                .poll(&mut plain[polled..polled + space])
                .expect("decoder poll");
            assert!(got.n > 0, "decoder poll produced no bytes");
            polled += got.n;
            if !got.more {
                break;
            }
        }
        if sunk == compressed {
            assert!(dec.finish(), "decoder finish should be DONE");
        }
        assert!(
            polled <= input.len(),
            "decompressed data is larger than input"
        );
    }
    assert_eq!(polled, input.len(), "decompressed length");
    assert_eq!(&plain[..polled], input);
}

#[allow(dead_code)]
pub fn encode_finished(window: u8, lookahead: u8, input: &[u8]) -> Vec<u8> {
    let mut enc = Encoder::try_new(window, lookahead).expect("alloc");
    let mut copied = enc.sink(input).expect("sink");
    assert_eq!(copied, input.len());
    let mut out = vec![0u8; 1024];
    let first = enc.poll(&mut out).expect("poll");
    assert!(!first.more && first.n == 0);
    assert!(!enc.finish());
    let second = enc.poll(&mut out).expect("poll");
    assert!(!second.more);
    copied = second.n;
    assert!(enc.finish());
    out.truncate(copied);
    out
}
