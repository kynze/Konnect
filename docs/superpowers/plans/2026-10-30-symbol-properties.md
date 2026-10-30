# Reusable symbol properties implementation plan

> **For agentic workers:** Use superpowers:executing-plans to implement this plan task by task.

**Goal:** Implement AILAB-40's approved reusable symbol properties and default footprint.

**Architecture:** Share typed property validation and source-span patching between creation and editing. Use existing compare-and-swap atomic writers, preserve unrelated source bytes, and read persisted fields back before reporting success.

**Tech Stack:** Rust, serde, JSON Schema, konnect-sexp, KiCad 10.0.6.

**Spec:** Approved design recorded in AILAB-40 and the user approval on 2026-10-30.

## Global constraints

- No production design edits, installed-binary replacement, or publishing.
- Preserve glyphs, pins, units, and unrelated properties.
- Malformed input must leave files unchanged; repeated calls must not duplicate fields.
- Use sanitized fixtures and independent KiCad 10.0.6 verification.

## Review focus

- Escaped quotes, backslashes, Unicode and line breaks survive persistence.
- Zero coordinates and false visibility differ from omission.
- Duplicate names and conflicting standard-field inputs are refused.
- Concurrent modification and write/read failures cannot report success.
- Inherited symbols and malformed existing libraries are refused when faithful editing cannot be established.

### Task 1: Property creation, editing and readback

**Files:** `crates/konnect-core/src/tools/library.rs`, new `library_symbol_properties.rs`, `library_symbol_properties_tests.rs`, and `crates/konnect-core/src/router/registry.rs` (advertised library tool count).

**Interfaces:** `footprint: string`; `properties: [{name, value, visible?, x?, y?, angle?}]`. New `edit_symbol_properties(library_path, symbol_name, properties?, footprint?)`. New fields default hidden at (0,0,0); editing preserves omitted presentation attributes. Readback retains `properties` map and adds `property_details`.

- [x] Write router/schema tests and creation/edit persistence tests before implementation; observe failures.
- [x] Implement strict typed validation, property upsert and atomic compare-and-swap writes. Reject duplicates, nulls, unknown keys and invalid numeric/rotation values. Reserved create fields use existing top-level parameters; Footprint uses `footprint`.
- [x] Test invalid batches, non-existent target, duplicate existing fields, malformed library, inherited symbols, escaping, partial patches, multi-unit preservation, and repeat-call byte equality.
- [x] Run focused tests, then core and workspace suites, formatting and clippy.

### Task 2: Independent verification and documentation

**Files:** `docs/SYMBOL_PROPERTIES.md`, `docs/AILAB-40-VERIFICATION.md`, `tool-directory.md`, isolated verification script under `crates/konnect-core/tests/`.

**Interfaces:** Build actual MCP server, exercise served tools, and use KiCad 10.0.6's symbol loader/exporter on sanitized artifacts.

- [x] Verify served schema/dispatch and persisted fields independently with KiCad 10.0.6, including round-trip and preservation.
- [x] Document API, defaults, refusal and outcome behavior, commands and measured results.
- [x] Review full diff, fix material findings, commit task-owned paths with AILAB-40 subject, and record commit and evidence in Plane before Done.

## Execution record

- User approved design and execution: "yes go."
- Worktree: `jbp-kynze/ailab-40-symbol-properties`, based on `34ad114` to retain existing local fixes.
- Ruling: execute inline without another approval pause; approval already covers this implementation.
- Pre-flight: task 2 consumes task 1's served schema and readback; names match above.
- First broader suite exposed a stale advertised tool count (229 loaded versus 228 advertised); update the library registry from 17 to 18 tools.
- Independent review found noncanonical lexical layouts could confuse borrowed pad token scanning, malformed positions could bypass value-only validation, and readback needed binding to the expected snapshot. Add reproducing tests before fixes; use property-specific token spans and unconditional presentation validation.
