# Parity exceptions

Intentional C vs Rust divergences from **SonarQube SECURITY issues and Security Hotspots** on library C modules. Non-security quirks stay matched (see `PARITY.md`).

This file covers Sonar findings on **original C** **only**. SECURITY findings and hotspots on newly written Rust are fixed in place (and listed under "Rust Sonar mitigations" in `MIGRATION.md`); they are not rows here. Every other intentional change (undefined-behavior fixes such as use-after-free, NULL dereference, or stack exhaustion, and hook-visible allocation or free changes) is a `CH-NNN` row under "Behavior changed on purpose (approved)" in `MIGRATION.md`, with a reason, user approval, and a pinning test. Defects found in the C oracle itself (for example by AddressSanitizer) are documented in `PARITY.md` under "Oracle defect found", not here.

The original C sources are unchanged and remain the oracle. Unlisted C/Rust diffs still fail `make parity` / the differential harness.

Each row is proposed to the user and added only after the user approves it; record who approved it and when in **Approval**.

Each row must have one `#[test]` in `heatshrink-core/tests/exceptions.rs` that:

1. Feeds an input that triggers the C bug or unsafe path.
2. Asserts Rust's safe result.
3. If practical, runs the C oracle and asserts C still shows the old behavior.

If a project fixture is the exception input, name it in **Fixture**.

Cite C lines in the current source. If the SonarQube analysis predates edits to the file, map each finding to its current line and give the Sonar line in **Sonar key** (for example `c:S6069 @ lib.c:614`).

| ID | C file:line | Sonar key | Finding | C behavior | Rust behavior | Approval | Test | Fixture |
|----|-------------|-----------|---------|------------|---------------|----------|------|---------|
| — | — | — | — | — | — | — | — | — |

**None** for this port. No driver-compared behavior was changed for SonarQube findings.
