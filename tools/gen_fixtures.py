#!/usr/bin/env python3
"""Materialize heatshrink C-test inputs as HSF1 fixtures.

Covers the bytes used by test_heatshrink_static.c and
test_heatshrink_dynamic.c, including the deterministic pseudorandom fuzz
loops. Decoder input-buffer sweeps that reuse those same bytes live in the
ported Rust tests; one representative input_buffer is stored here so the
differential driver still sees window and lookahead from the fixture.
"""

import os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "tests", "inputs")
MASK = (1 << 64) - 1
PRIME = 9223372036854775783


def fill(size, seed):
    rn = PRIME
    out = bytearray(size)
    for i in range(size):
        rn = (rn * seed + seed) & MASK
        out[i] = (rn % 26) + ord("a")
    return bytes(out)


def write_fix(rel, alloc, window, lookahead, ibs, sink_chunk, poll_chunk, payload):
    path = os.path.join(OUT, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    header = (
        "HSF1\n"
        f"alloc={alloc}\n"
        f"window={window}\n"
        f"lookahead={lookahead}\n"
        f"input_buffer={ibs}\n"
        f"sink_chunk={sink_chunk}\n"
        f"poll_chunk={poll_chunk}\n"
        "\n"
    )
    with open(path, "wb") as fh:
        fh.write(header.encode("ascii"))
        fh.write(payload)
    return path


def main():
    n = 0
    # test_heatshrink_static.c: window 8, lookahead 4, input buffer 32,
    # size = 1,2,4,...,32768 and seed = 1..100.
    for exp in range(16):
        size = 1 << exp
        for seed in range(1, 101):
            write_fix(
                f"static/s{size}_seed{seed}.hsf",
                "static", 8, 4, 32, 0, 0, fill(size, seed),
            )
            n += 1

    # Dynamic single-byte lookahead fuzz: window 8, lookahead 3..7,
    # size = 1..65536, seed 1..10. input_buffer 64 matches the regression
    # cases in the same file (sixty_four_k uses this buffer size).
    for lookahead in range(3, 8):
        for exp in range(17):
            size = 1 << exp
            for seed in range(1, 11):
                write_fix(
                    f"dyn/w8_l{lookahead}_s{size}_seed{seed}.hsf",
                    "dyn", 8, lookahead, 64, 0, 0, fill(size, seed),
                )
                n += 1

    # Dynamic multi-byte window fuzz: window 11, lookahead 6..8.
    for lookahead in range(6, 9):
        for exp in range(17):
            size = 1 << exp
            for seed in range(1, 11):
                write_fix(
                    f"dyn/w11_l{lookahead}_s{size}_seed{seed}.hsf",
                    "dyn", 11, lookahead, 64, 0, 0, fill(size, seed),
                )
                n += 1

    # input_buffer sweep at one (size, seed, window, lookahead) point.
    # ibs 64 is already the dyn fuzz file for this input.
    payload_128 = fill(128, 1)
    for ibs in (32, 128, 256, 512, 1024, 2048, 4096, 8192):
        write_fix(
            f"ibs/w8_l3_s128_seed1_ibs{ibs}.hsf",
            "dyn", 8, 3, ibs, 0, 0, payload_128,
        )
        n += 1

    vectors = [
        ("vectors/enc_literals.hsf", "dyn", 8, 7, 256, 0, 0, bytes(range(5))),
        ("vectors/enc_aaaaa.hsf", "dyn", 8, 7, 256, 0, 0, b"aaaaa"),
        ("vectors/enc_repeat.hsf", "dyn", 8, 3, 256, 0, 0, b"abcdabcd"),
        ("vectors/enc_repeat_tail.hsf", "dyn", 8, 3, 256, 0, 0, b"abcdabcde"),
        ("vectors/enc_sink_fit.hsf", "dyn", 8, 7, 256, 0, 0, b"*" * 256),
        ("vectors/enc_sink_partial.hsf", "dyn", 8, 7, 256, 0, 0, b"*" * 512),
        ("vectors/dec_foo.hsf", "dyn", 7, 3, 256, 0, 0, bytes([0xB3, 0x5B, 0xED, 0xE0])),
        ("vectors/dec_foofoo.hsf", "dyn", 7, 6, 256, 0, 0, bytes([0xB3, 0x5B, 0xED, 0xE0, 0x41, 0x00])),
        ("vectors/dec_aaaaa.hsf", "dyn", 8, 7, 256, 0, 0, bytes([0xB0, 0x80, 0x01, 0x80])),
        ("vectors/dec_suspend_lit.hsf", "dyn", 7, 6, 256, 0, 1, bytes([0xB3, 0x5B, 0xED, 0xE0, 0x40, 0x80])),
        ("vectors/dec_suspend_backref.hsf", "dyn", 7, 6, 256, 0, 4, bytes([0xB3, 0x5B, 0xED, 0xE0, 0x40, 0x80])),
        ("vectors/dec_foofoo_byte_sink.hsf", "dyn", 7, 6, 256, 1, 0, bytes([0xB3, 0x5B, 0xED, 0xE0, 0x41, 0x00])),
        ("vectors/alpha.hsf", "dyn", 8, 3, 256, 0, 0, b"abcdefghijklmnopqrstuvwxyz"),
        ("vectors/repetition.hsf", "dyn", 8, 3, 256, 0, 0, b"abcabcdabcdeabcdefabcdefgabcdefgh"),
        ("vectors/alpha_tiny.hsf", "dyn", 8, 3, 256, 1, 1, b"abcdefghijklmnopqrstuvwxyz"),
        ("vectors/repetition_tiny.hsf", "dyn", 8, 3, 256, 1, 1, b"abcabcdabcdeabcdefabcdefgabcdefgh"),
        ("vectors/small_ibs5.hsf", "dyn", 8, 3, 5, 0, 0, bytes(ord("a") + (i % 26) for i in range(5))),
        ("vectors/reg_backref_337_seed3.hsf", "dyn", 8, 3, 64, 0, 0, fill(337, 3)),
        ("vectors/reg_index_507_seed3.hsf", "dyn", 8, 3, 64, 0, 0, fill(507, 3)),
        ("vectors/ff256.hsf", "dyn", 8, 4, 256, 0, 0, b"\xff" * 256),
        ("vectors/ff512.hsf", "dyn", 8, 4, 256, 0, 0, b"\xff" * 512),
    ]
    for rel, alloc, window, lookahead, ibs, sink, poll, payload in vectors:
        write_fix(rel, alloc, window, lookahead, ibs, sink, poll, payload)
        n += 1

    print(n)


if __name__ == "__main__":
    main()
