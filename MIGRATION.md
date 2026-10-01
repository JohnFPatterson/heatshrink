# heatshrink C to Rust

Streaming LZSS codec. `heatshrink-core` holds the encoder and decoder state machines. `heatshrink-ffi` is the dynamic C ABI from `heatshrink_encoder.h` and `heatshrink_decoder.h` (the default `HEATSHRINK_DYNAMIC_ALLOC 1` header). Original `*.c` / `*.h` are unchanged.

## Layout

| Path | Role |
|------|------|
| `heatshrink-core` | All codec logic. `#![forbid(unsafe_code)]`. |
| `heatshrink-ffi` | `extern "C"` alloc/free/reset/sink/poll/finish. `staticlib` + `cdylib`. |
| `heatshrink-driver` | Differential driver. Same stdout as `tools/heatshrink-oracle`. |
| `tools/heatshrink-oracle.c` | C oracle. Public headers only. Also links as `build/oracle-ffi`. |
| `tools/hook-trace.c` | `malloc`/`free` trace, built against C and against `heatshrink-ffi`. |
| `tests/inputs/` | HSF1 fixtures from the C test inputs, including the deterministic fuzz loops. |

Window, lookahead, and decoder input-buffer size come from the fixture header. `alloc=static` is the static-build triple from `heatshrink_config.h` (window 8, lookahead 4, input buffer 32) run through the same state machine. `Encoder::from_static_config` and `Decoder::from_static_config` are that configuration without going through `try_new` failure.

## Oracle shape

Streaming binary codec. Public operations are `heatshrink_encoder_*` and `heatshrink_decoder_*`: alloc, free, reset, sink, poll, finish. Both the default dynamic build and the static build used by `test_heatshrink_static.c` are in scope. Driver sections are `encode`, `decode`, and `roundtrip` (`tools/DRIVER_FORMAT.md`).

## Unsafe

`rg unsafe` on `*.rs` outside `target/`:

- `heatshrink-core/src/lib.rs` — `#![forbid(unsafe_code)]` only.
- `heatshrink-ffi/src/lib.rs` — every `unsafe` block has a `SAFETY:` comment. `#![deny(unsafe_op_in_unsafe_fn)]` and `#![deny(clippy::undocumented_unsafe_blocks)]`.
- `heatshrink-ffi/tests/abi.rs` — test wrappers around the unsafe FFI entry points, plus one field read of `input_size` / `input_index`.

FFI `malloc` sizes and order match the dynamic C library (`HEATSHRINK_USE_INDEX 1`):

| Call | Size (window `w`, decoder input `ibs`) |
|------|----------------------------------------|
| encoder object | `32 + (2 << w)` |
| search index | `2 + (2 << w) * 2`, freed first |
| decoder object | `18 + (1 << w) + ibs` |

`hs_index.size` is a `uint16_t`. The port stores the index byte length with the same truncation.

## Export check

Declaration pattern: plain prototypes at column 0 in `heatshrink_encoder.h` and `heatshrink_decoder.h`. There is no export macro (`PREFIX` is N/A).

`nm -g --defined-only target/release/libheatshrink_ffi.a` exports these 12 symbols and no other `heatshrink_*` C names:

`heatshrink_encoder_alloc`, `heatshrink_encoder_free`, `heatshrink_encoder_reset`, `heatshrink_encoder_sink`, `heatshrink_encoder_poll`, `heatshrink_encoder_finish`, `heatshrink_decoder_alloc`, `heatshrink_decoder_free`, `heatshrink_decoder_reset`, `heatshrink_decoder_sink`, `heatshrink_decoder_poll`, `heatshrink_decoder_finish`.

The static build does not declare alloc/free. Those entry points are not a second FFI symbol set; the static sizes are `from_static_config` in the core.

## Behavior changed on purpose (approved)

| ID | Reason | Approval | Pinning test |
|----|--------|----------|--------------|
| — | — | — | — |

No `CH-NNN` rows. Hook-trace matches C, including fail-on-Nth allocation.

## Parity exceptions

No `PE-NNN` rows. See `PARITY_EXCEPTIONS.md`.

## Rust Sonar mitigations

The SonarQube Cloud project linked to this repository (only the long-lived `master` branch is present) has no analysis measures. `search_sonar_issues_in_projects` with `impactSoftwareQualities: ["SECURITY"]` and statuses `OPEN`/`CONFIRMED` returned 0 issues. `search_security_hotspots` with status `TO_REVIEW` returned 0 hotspots.

`sonar analyze` (STANDARD and DEEP) on the library C files and the new Rust files returned `403 Forbidden` (`Vortex analysis is not available on this connection`). The `Sonarqube` MCP namespace is in error and `mcp_auth` cannot complete in this session. `sonar analyze secrets` on `heatshrink-core`, `heatshrink-ffi`, `heatshrink-driver`, and `tools/hook-trace.c` reported no secrets.

No OPEN SECURITY issue or TO_REVIEW hotspot was returned to mitigate. There is no Sonar quality gate in this repository (`.travis.yml` runs `make test` only). A future gate should fail on new-code / Rust security, not on legacy C ratings alone. C sources were not patched.

## Hook trace

`make hook-trace` builds `tools/hook-trace.c` twice with `-Wl,--wrap=malloc -Wl,--wrap=free`: once with `heatshrink_encoder.c` and `heatshrink_decoder.c`, once with `target/release/libheatshrink_ffi.a`. `diff -u` is empty.

```
CASE encoder_round
alloc 544
alloc 1026
free 1026
free 544
CASE encoder_fail_first
alloc_fail 544
CASE encoder_fail_second
alloc 544
alloc_fail 1026
free 544
CASE encoder_invalid
CASE decoder_round
alloc 530
free 530
CASE decoder_fail_first
alloc_fail 530
CASE decoder_invalid
```

`encoder_round` / `decoder_round` also sink, poll, finish, and reset. Those calls allocate nothing further. Invalid parameters allocate nothing. Fail-on-first and fail-on-second match, including the free of the encoder block when the index allocation fails.

## Tests

| C case | Rust |
|--------|------|
| `test_heatshrink_dynamic.c` public API, except the three NULL-free cases below | `heatshrink-core/tests/dynamic_suite.rs` and `heatshrink-ffi/tests/abi.rs` |
| Dynamic fuzz loops (lookahead, size, input buffer, seed) | `pseudorandom_single_byte_lookahead_fuzz`, `pseudorandom_multi_byte_window_fuzz` |
| `test_heatshrink_static.c` pseudorandom loop | `heatshrink-core/tests/static_suite.rs` using `from_static_config` |
| Null argument checks | `null_pointers_return_error_codes` |
| `hsd->input_size` / `input_index` after sink | `decoder_sink_should_sink_data_when_preconditions_hold` and `visible_input_fields_match_after_sink` |

`build/test_dynamic_ffi` (the C file linked to `libheatshrink_ffi.a`) passed encoding (11), regression (4), and integration (12244). `decoder_poll_should_expand*` (4) and `decoder_should_not_get_stuck_*` also passed against the FFI.

Not run to completion against the FFI, because they call `heatshrink_decoder_free` on NULL:

- `decoder_sink_should_reject_null_input_pointer` (`test_heatshrink_dynamic.c:289`)
- `decoder_sink_should_reject_null_count_pointer`
- `decoder_poll_should_reject_null_output_size_pointer`

Each calls `heatshrink_decoder_alloc(256, HEATSHRINK_MIN_WINDOW_BITS, 4)` (window 4, lookahead 4), which returns NULL, then frees that pointer. See the oracle defect in `PARITY.md`. The null-argument status codes are still checked on live objects in `abi.rs`.

## Out of scope

- `heatshrink.c` — command-line tool. Stays C.
- `benchmark` and the Canterbury corpus download in the Makefile.
- `enc_sm.dot` and `dec_sm.dot` — graphviz state diagrams.
- `test_heatshrink_dynamic_theft.c` — libtheft property tests, compiled out unless `HEATSHRINK_HAS_THEFT` is set. The translation unit only declares a dummy struct in the default build.
- `HEATSHRINK_USE_INDEX 0` — both in-scope builds set it to 1.
- `HEATSHRINK_DEBUGGING_LOGS` — off in `heatshrink_config.h`.
