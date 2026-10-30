# AILAB-40 verification

Date: 2026-10-30. Base: `34ad114`. Branch:
`jbp-kynze/ailab-40-symbol-properties`.

## Change and boundaries

The `library` toolset now accepts typed custom fields and a default Footprint
when creating a symbol. `edit_symbol_properties` atomically upserts library
fields, preserving unmentioned presentation, glyphs, units, pins and other
symbols. Name/value readback remains compatible; presentation details and saved
file source metadata are additive. See [the API](SYMBOL_PROPERTIES.md).

Work is confined to this local fork branch. No production design, installed
binary, remote branch or upstream repository was changed. Sibling tasks
AILAB-41, AILAB-42 and AILAB-43 were unassigned in Backlog when checked; this
change owns the symbol-property module and the small registrations in library.rs.
Footprint mask/paste and padstack implementations remain separate tasks.

## Independent KiCad verification

Run the actual built server over stdio, then use KiCad's own loader/saver and
SVG exporter:

```powershell
python crates/konnect-core/tests/verify_symbol_properties.py `
  --server <path-to-built-konnect.exe> `
  --kicad-cli 'C:/Program Files/KiCad/10.0/bin/kicad-cli.exe' `
  --output <new-empty-output-directory>
```

Requires Python 3 and `sexpdata`. The verifier refuses an existing output
directory and requires KiCad **10.0.6**. It creates an isolated server config
and state directory, never opens production source files, and writes libraries
only through the server or KiCad's native upgrade command.

Observed result with the updated server:

- Two symbols, including a dual amplifier with six pins.
- Ten custom field values checked after native loading and saving, including
  quotes, backslashes, Unicode, a newline and an empty string.
- Default Footprint, visibility, explicit zero, omitted y/angle, and added fields
  checked independently after editing and a second native save round-trip.
- Unrelated units, artwork and the second symbol preserved.
- Repeating an edit was byte-identical. Duplicate creation and invalid input
  were refused with byte-identical saved libraries.
- Three SVG exports succeeded.

Sanitization: field names reproduce the PCB573-74 import workflow. All part
names, manufacturer names, identifiers, values, geometry and provenance strings
in the verifier are synthetic. No private workbook, part record, source hash or
private source path is included in these deliverables.

## Regression evidence and review

The original schema rejected the new Footprint argument and the editor was not
served; schema/dispatch tests failed before implementation. Repeat-edit
whitespace drift and omitted-coordinate rounding were reproduced and corrected.

A fresh read-only review identified source-span errors for compact/spaced
S-expressions, incomplete validation of existing position fields, and an
unbound semantic-readback snapshot. Three regressions failed before those
fixes. Property-specific token spans, unconditional presentation validation,
candidate validation before persistence and expected-snapshot readback addressed
the findings. The reviewer rechecked the fixes and found no remaining material
issues. A parser-rejected Unicode whitespace form is tested as a no-write
refusal rather than expanding parser scope.

Tests also cover duplicate fields, malformed libraries, inherited-symbol
refusal, stale-source conflicts, write failures and missing/concurrently replaced
readback. Failures after entering the write boundary conservatively report
`mutation_outcome_uncertain`, never a rollback promise.

The workspace tests exposed the new tool's stale advertised library count and
documentation totals. The library count was updated and documentation counts
regenerated with `cargo xtask fix-doc-counts`.

## Local checks

Commands used for final validation:

```text
cargo test --workspace --locked --lib --tests --quiet
cargo clippy -p konnect-core --lib --tests -- -D warnings
cargo fmt --all -- --check
git diff --check
python -m py_compile crates/konnect-core/tests/verify_symbol_properties.py
```

Observed final results: **2,387 passed, 0 failed, 45 ignored** across 34 workspace
test executables, including all ten property regressions. Clippy with warnings
denied, formatting, diff whitespace checks and Python compilation passed. The
native verifier also passed against the rebuilt development server after the
review fixes. Ignored tests remain unexercised; they are not counted as passes.

Validation is local on Windows; hosted CI and other platforms were not exercised,
since no push or publication is authorized by this task.
