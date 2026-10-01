# Bugbot review rules: heatshrink C-to-Rust migration

This repository ports the C library declared in `heatshrink_encoder.h and heatshrink_decoder.h` to Rust. `heatshrink-core` holds all logic and forbids `unsafe`. `heatshrink-ffi` is the thin C ABI shim and is the only crate allowed to contain `unsafe`. The original C sources stay in-tree as the behavioral spec.

Apply all six rules below on every review. Each violation is a **blocking** Bug.

## Rule 1: `unsafe` outside `heatshrink-ffi`

If a changed file anywhere outside `heatshrink-ffi/` adds any of the following, add a **blocking** Bug titled "unsafe outside heatshrink-ffi", naming the file and line:

- an `unsafe` block, `unsafe fn`, `unsafe impl`, or `unsafe trait`
- an `unsafe extern` block
- `#[allow(unsafe_code)]` or `#![allow(unsafe_code)]`

This covers `heatshrink-core/src/`, `src/bin/`, `tests/`, `benches/`, `examples/`, `build.rs`, and any other crate in the workspace.

Also add a **blocking** Bug titled "unsafe outside heatshrink-ffi" if a PR removes, comments out, or weakens `#![forbid(unsafe_code)]` in `heatshrink-core/src/lib.rs` (for example by changing it to `deny` or `warn`).

The word `unsafe` inside comments, doc comments, or string literals doesn't count, and neither does the `forbid(unsafe_code)` attribute itself.

## Rule 2: every `PARITY_EXCEPTIONS.md` entry has a matching test

`PARITY_EXCEPTIONS.md` lists intentional C-vs-Rust behavior changes. Each table row with an ID of the form `PE-NNN` names a test in its **Test** column.

Add a **blocking** Bug titled "Parity exception without matching test", listing the affected IDs, if any of these is true:

- A `PE-NNN` row's **Test** column is empty, or names a function that isn't a `#[test]` in `heatshrink-core/tests/exceptions.rs`.
- The named test is marked `#[ignore]`.
- A PR adds a `PE-NNN` row without adding its test in the same PR.
- A PR deletes or renames a test in `heatshrink-core/tests/exceptions.rs` while a row still refers to the old name.

If the table contains only the placeholder `—` row, there are no exceptions: the rule passes and `heatshrink-core/tests/exceptions.rs` doesn't need to exist.

## Rule 3: FFI signatures match `heatshrink_encoder.h and heatshrink_decoder.h` exactly

Check every function in `heatshrink-ffi` exported with `#[no_mangle]` or `#[unsafe(no_mangle)]` as `pub extern "C" fn` or `pub unsafe extern "C" fn`. Compare it with the declaration of the same name in `heatshrink_encoder.h and heatshrink_decoder.h`.

Before comparing, expand any export macro used by this header (for example `N/A_PUBLIC(type)` → `type` when that macro exists), and resolve the header's typedefs to their underlying C types. If `N/A` is unused by the header, compare against the plain prototypes.

These must all match exactly:

- symbol name
- `extern "C"` calling convention (unless the header documents another convention for this build)
- parameter count and order (a C `(void)` parameter list means no parameters)
- each parameter type and the return type

Use these common mappings as a starting point and **extend for types this header actually uses**:

| C | Rust |
|---|------|
| `int` (and `int` typedefs) | `c_int` |
| `char` | `c_char` |
| `size_t` | `usize` |
| `double` | `f64` |
| `void` return | no return type |
| `const T *` | `*const T` |
| `T *` | `*mut T` |
| `const char **` | `*mut *const c_char` |
| struct `S *` / `const S *` | `*mut S` / `*const S`, where `S` is the `#[repr(C)]` type |

For any mismatch, add a **blocking** Bug titled "FFI signature differs from heatshrink_encoder.h and heatshrink_decoder.h" that quotes the header declaration (with its line number) and the Rust signature.

Also add a **blocking** Bug with that title if either of these is true, unless `MIGRATION.md` names the function as out of scope:

- A public function in `heatshrink_encoder.h and heatshrink_decoder.h` (after expanding this header's export macro, if any) has no export in `heatshrink-ffi`.
- A `#[no_mangle]` export in `heatshrink-ffi` isn't declared in `heatshrink_encoder.h and heatshrink_decoder.h`.

## Rule 4: every intentional change (`CH-NNN`) has a pinning test

`MIGRATION.md` lists intentional behavior changes that aren't SonarQube findings (undefined-behavior fixes, hook-visible allocation or free changes) in a table whose IDs have the form `CH-NNN`. Each row names a test in its **Pinning test** column, as `file::test_name` or `file::test_a`, `::test_b`.

Add a **blocking** Bug titled "Intentional change without pinning test", listing the affected IDs, if any of these is true:

- A `CH-NNN` row's **Pinning test** column is empty, or names a function that isn't a `#[test]` in the named file (or, if no file is given, anywhere under `heatshrink-core/tests/` or `heatshrink-ffi/tests/`).
- The named test is marked `#[ignore]`.
- A PR adds a `CH-NNN` row without adding its test in the same PR.
- A PR deletes or renames a test while a `CH-NNN` row still refers to the old name.
- A `CH-NNN` row's **Approval** column is empty, when the table has one.

Also add this Bug if a PR adds a `CH-NNN`-style change to `PARITY_EXCEPTIONS.md` (a row whose **Sonar key** is empty, `—`, or not a Sonar rule or hotspot key). That file is for SonarQube findings on original C only.

If `MIGRATION.md` has no `CH-NNN` rows, the rule passes.

## Rule 5: `.cursor/parity.json` never loosens a ready module

`.cursor/parity.json` may contain a `modules` map. Each module has `driver_args`, a boolean `ready`, and optional `fixtures`. Once a module is ready, the parity gate pins it, and it must stay exactly as it is.

Compare `.cursor/parity.json` in the PR with the base branch. For every module whose `ready` is `true` in the base, add a **blocking** Bug titled "parity.json loosens a ready module", naming the module, if the PR:

- changes its `ready` to `false`, or removes the `ready` key;
- removes the module, or removes the whole `modules` map;
- changes its `driver_args` in any way, including reordering;
- adds, removes, or changes its `fixtures`, or removes the key.

Changing a module from `false` to `true` and adding new modules is allowed. So is editing a module that isn't ready in the base.

## Rule 6: Sonar on new Rust vs original C

SonarQube SECURITY findings and Security Hotspots on **new Rust** (`heatshrink-core`, `heatshrink-ffi`, and other Rust the port adds) must be mitigated. Findings on **original C** stay informational for the port: they may become `PE-NNN` rows, but the C sources must not be patched to clear them.

Add a **blocking** Bug titled "Rust Sonar security finding left open" if a PR:

- Introduces or leaves unresolved a known SECURITY issue or Security Hotspot in changed Rust (for example the PR discussion, `MIGRATION.md`, or a linked Sonar report names an open finding on a Rust path and the PR does not fix it or record a user-approved won't-fix).
- Adds a `PE-NNN` row whose **C file:line** points at a Rust file, or whose purpose is to waive a Rust-only Sonar finding (Rust mitigations are not parity exceptions).

Add a **blocking** Bug titled "C patched to satisfy Sonar" if a PR edits product `*.c` / `*.h` (the oracle) to clear a Sonar finding, unless `MIGRATION.md` already documents that exact edit as an approved, non-Sonar change with a `CH-NNN` row (almost never appropriate for Sonar cleanup).

This rule does **not** require CI or the PR to fail because overall / legacy C SECURITY ratings are non-A. Flag missing Rust mitigations and C-for-Sonar patches only.

## Known gaps

These rules don't check:

- Whether a SonarQube analysis was actually run, or whether a CI quality gate is configured for new-code-only security. Rule 6 only fires on evidence already in the PR.
- `#[repr(C)]` struct field order, types, or layout against the header's structs.
- Non-default calling conventions beyond what Rule 3 can see from the header text.
- Whether a pinning or exception test actually asserts the changed behavior. Rules 2 and 4 check only that it exists and isn't ignored.
- Other loosening of `.cursor/parity.json` (narrowed top-level `fixtures`, changed `c_cmd`, `oracle_sources`, or `compare_stderr`). The parity gate's pins catch edited fixtures and oracle sources, but not every config change.
- The parity gate's state file, which lives outside the repository.
