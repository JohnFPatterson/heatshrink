use heatshrink_core::{Decoder, Encoder};

fn encode_all(window: u8, lookahead: u8, input: &[u8]) -> Vec<u8> {
    let mut enc = Encoder::try_new(window, lookahead).expect("params");
    let mut sunk = 0;
    let mut out = Vec::new();
    let mut tmp = [0u8; 64];
    while sunk < input.len() {
        let n = enc.sink(&input[sunk..]).expect("sink");
        sunk += n;
        if sunk == input.len() {
            assert!(!enc.finish());
        }
        loop {
            let polled = enc.poll(&mut tmp).expect("poll");
            out.extend_from_slice(&tmp[..polled.n]);
            if !polled.more {
                break;
            }
        }
        if sunk == input.len() {
            assert!(enc.finish());
        }
    }
    out
}

fn decode_all(ibs: u16, window: u8, lookahead: u8, input: &[u8]) -> Vec<u8> {
    let mut dec = Decoder::try_new(ibs, window, lookahead).expect("params");
    let mut sunk = 0;
    let mut out = Vec::new();
    let mut tmp = [0u8; 64];
    while sunk < input.len() {
        if let Ok(n) = dec.sink(&input[sunk..]) {
            sunk += n;
        }
        if sunk == input.len() {
            assert!(!dec.finish());
        }
        loop {
            let polled = dec.poll(&mut tmp).expect("poll");
            out.extend_from_slice(&tmp[..polled.n]);
            if !polled.more {
                break;
            }
        }
        if sunk == input.len() {
            assert!(dec.finish());
        }
    }
    out
}

#[test]
fn literals_match_c_vector() {
    let got = encode_all(8, 7, &[0, 1, 2, 3, 4]);
    assert_eq!(got, vec![0x80, 0x40, 0x60, 0x50, 0x38, 0x20]);
}

#[test]
fn aaaaa_match_c_vector() {
    let got = encode_all(8, 7, b"aaaaa");
    assert_eq!(got, vec![0xb0, 0x80, 0x01, 0x80]);
}

#[test]
fn repeat_match_c_vector() {
    let got = encode_all(8, 3, b"abcdabcd");
    assert_eq!(got, vec![0xb0, 0xd8, 0xac, 0x76, 0x40, 0x1b]);
}

#[test]
fn repeat_tail_match_c_vector() {
    let got = encode_all(8, 3, b"abcdabcde");
    assert_eq!(got, vec![0xb0, 0xd8, 0xac, 0x76, 0x40, 0x1b, 0xb2, 0x80]);
}

#[test]
fn decode_foo() {
    let got = decode_all(256, 7, 3, &[0xb3, 0x5b, 0xed, 0xe0]);
    assert_eq!(got, b"foo");
}

#[test]
fn decode_foofoo() {
    let got = decode_all(256, 7, 6, &[0xb3, 0x5b, 0xed, 0xe0, 0x41, 0x00]);
    assert_eq!(got, b"foofoo");
}
