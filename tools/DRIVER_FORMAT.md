# heatshrink differential driver format

Streaming binary codec. Both drivers read one fixture and print the same
sections to stdout. The parity gate compares stdout and the exit status.

## Invocation

```
heatshrink-oracle <fixture> [--sections encode,decode,roundtrip]
heatshrink-driver <fixture> [--sections encode,decode,roundtrip]
```

`<fixture>` is a path. `--sections` is optional. A comma-separated list
selects sections. Unknown names, a missing list, or an empty list exit 2
with `error usage` on stdout. The default is `encode,decode,roundtrip`,
printed in that order (not the order named on the command line: drivers
always emit selected sections in the order encode, then decode, then
roundtrip).

Exit 0 after a fixture is processed, including when the library returns
null, the stream stalls, or a round trip does not match. Those outcomes
are stdout, not crashes. Exit 2 for usage errors, unreadable files, a
malformed header, or `alloc=static` combined with parameters other than
window 8, lookahead 4, and input buffer 32 (`error static_config`).

stderr is not part of the comparison.

## Fixture (HSF1)

Text header, LF only, then a raw payload. Marked `-text` in `.gitattributes`.

```
HSF1
alloc=dyn|static
window=<0-255>
lookahead=<0-255>
input_buffer=<0-65535>
sink_chunk=<u32>
poll_chunk=<u32>

<payload bytes>
```

Keys are required once. `window` and `lookahead` are the codec settings.
`input_buffer` is the decoder input buffer size passed to
`heatshrink_decoder_alloc`. `sink_chunk` 0 sinks the largest slice the
caller still has; any other value caps each `sink` call. `poll_chunk` 0
polls into whatever output space remains; any other value caps each
`poll` call.

`alloc=static` records the static build from `heatshrink_config.h`
(`HEATSHRINK_STATIC_WINDOW_BITS` 8, `HEATSHRINK_STATIC_LOOKAHEAD_BITS` 4,
`HEATSHRINK_STATIC_INPUT_BUFFER_SIZE` 32). One dynamic oracle binary
covers it: those constants are the only legal static header, and the
driver calls the dynamic allocators with them. The static and dynamic
builds are the same state machine. The Rust driver must take window,
lookahead, and input buffer from this header rather than from a
compile-time flag, so the same binary covers every fixture.

## Streaming procedure

Same crank as `compress_and_expand_and_check` in the C tests:

1. Alloc encoder or decoder with the header parameters. Alloc already resets.
2. While input remains:
   - `sink` the next slice (`sink_chunk` or the remainder).
   - If that call consumed the last input byte, `finish` once.
   - `poll` until the result is not `MORE` (numeric 1 for both encoder and decoder). Each poll is limited by `poll_chunk` when that is non-zero.
   - If the input is exhausted, `finish` again. While that `finish` returns `MORE` (numeric 1), `poll` until not `MORE` and `finish` again. A 1-byte poll can observe `EMPTY` while the encoder still owes a flushed byte (`HSES_FLUSH_BITS` falls through to `HSER_POLL_EMPTY` in `heatshrink_encoder.c`); the extra crank matches the tiny-buffer C tests, which loop on `finish == MORE`.
3. Free.

Output storage starts at `in_len + in_len/2 + 4` bytes (minimum 4). If a
poll needs space and none remains, the capacity doubles (the C tests use
the initial size and would abort past it; doubling is shared so a stream
that expands further still has one defined trace).

Stop codes, printed as `status`:

| status | meaning |
|--------|---------|
| `ok` | loop finished |
| `alloc_null` | alloc returned NULL; no `op` lines |
| `error` | sink or poll returned a negative code |
| `stall` | a poll returned MORE and copied 0, or a whole iteration copied nothing |
| `bound` | output length exceeded `in_len * 32 + 65536` |

The bound is an input-only cap so a hostile decode cannot run without
limit. Both drivers stop at the same byte count.

## Sections

`encode` compresses the payload. `decode` decompresses the payload as a
heatshrink stream (it does not have to be well formed). `roundtrip`
compresses, then decompresses the compressed bytes with the same window,
lookahead, input buffer, and chunk sizes.

Result codes are the C enums printed as signed decimals
(`HSER_SINK_OK` 0, `HSER_SINK_ERROR_NULL` -1, `HSER_POLL_MORE` 1,
`HSDR_SINK_FULL` 1, `HSER_FINISH_DONE` 0, `HSER_FINISH_MORE` 1, and the
matching decoder codes).

```
SECTION encode
cfg alloc=dyn window=8 lookahead=7 input_buffer=256 sink_chunk=0 poll_chunk=0 in_len=5
alloc ok
op sink 0 5
op finish 1
op poll 0 6
op finish 0
status ok
nbytes 6
hex 804060503820
END encode
```

`decode` uses the same shape (`alloc`, then `op` lines, then `status`,
`nbytes`, `hex`).

`roundtrip` prints the encode trace (`enc_alloc`, its `op` lines, `status`,
`nbytes`, `hex`) then the decode trace (`dec_alloc`, …) then `match 0` or
`match 1`. Decode runs only when encode status is `ok`; otherwise
`dec_alloc` is `null`. `match` is 1 only when decode status is `ok` and the
bytes equal the payload. Empty output is `hex -`.

`op` lines:

```
op sink <code> <copied>
op poll <code> <copied>
op finish <code>
```

`alloc null` suppresses `op` lines and forces `status alloc_null`,
`nbytes 0`, `hex -`.
