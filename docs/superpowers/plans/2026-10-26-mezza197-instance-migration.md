# Instance migration implementation plan

> **For agentic workers:** Use superpowers:executing-plans inline. Run one final independent review.

**Goal:** Finish MEZZA-197 with a tested local MCP server that can apply the agreed mezzanine page and reference mapping on a copy.

**Architecture:** Extend `renumber_sheet_pages` without changing its omitted-option behavior. `sheet_order` lists all non-root full instance UUID paths in desired page order (page 2 onward). Optional `reference_changes` identify a sheet instance, old reference and new reference. Resolve symbols from their saved instance records; edit every unit of the selected physical component. Accumulate pages and references into one transaction per physical file. Reject missing/duplicate paths, stale references, malformed input and final reference collisions before commit; swaps are valid. Preserve other projects, units, UUIDs and unselected instances. Report committed readback and support `dry_run`.

**Tech stack:** Rust 1.96, existing SchematicCommand/transaction engine, local MCP stdio server, KiCad 10.0.6 XML netlist verification.

**Spec:** MEZZA-197 and the migration crosswalk committed under MEZZA-192 in the hardware repository. No production design edits or installed-server replacement.

## Tasks

- [x] Add failing tests for explicit instance ordering, atomic reference swaps and repeated-sheet isolation, invalid input/preconditions/collisions, and dry runs.
- [x] Extend schema and implement parsing, complete order validation, batched instance reference edits and committed readback using existing model/transaction helpers.
- [x] Run affected tests, full core suite and formatting/lint checks; build separate local server.
- [x] Trial complete crosswalk on fresh hardware copy; verify every page/reference, UUID+pin connectivity, retained fit/value/footprint metadata, and valid KiCad save/reload/export. Preserve production hashes.
- [x] Independently review, fix findings, commit task-owned source/docs, record command/evidence/limits in Plane and complete task after Quality Check.

## Review focus

Repeated physical files versus instance identity; reference swaps and untouched reference collisions; multi-unit components; preserved foreign-project records and cached Reference properties; no writes on refusal/dry-run; readback versus planned results.
