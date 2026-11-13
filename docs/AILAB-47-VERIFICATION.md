# AILAB-47 verification

Scope: the parts of AILAB-47 not already delivered by AILAB-48/49: (2) revision-guarded footprint-ID replacement and (3) bulk application of a library footprint's 3D models. Items 1 and 4 are confirmed done on the base (below). **Native KiCad verification of this work is unverified**; see Limits.

Base: `dcb29c5` (`jbp-kynze/konnect-integration`). Branch: `jbp-kynze/ailab-47-swap-bulk`. Implementation commit: `42804c9`; this doc was adjusted in a follow-up commit. CARGO_TARGET_DIR `target-ailab47`, CARGO_BUILD_JOBS=2, no overlapping cargo runs; existing Rust/protoc/CMake reused (`.local` of the sibling worktree), nothing installed.

## What was built

Start point: `docs/AILAB-48-DEFERRED.patch`. It was made against the pre-verification source; `git apply --reject` applied all functional hunks onto the current tip (the post-commit verify/restore flow of AILAB-48 is intact and is what these new paths go through). The two rejected hunks were formatting-only test reflows and were dropped; rustfmt was run on the file.

- `update_footprints_from_library` gains `replacements: [{reference, library_id}]` (like KiCad F8: new footprint ID, same position, rotation, side, locked state, KIID, symbol path/linkage, pad nets, pad identities/symbol pins), `scope: "full"|"models"` and `allow_routed_pad_movement` (default false). Same dry-run default, `expected_plan_revision` guard, in-commit stale re-check, single commit, independent readback and compensating-commit rollback as the library refresh. The filters (including replacements, scope, opt-in) are part of the plan revision.
- Pad mapping is by logical pad number and must be unique on both sides for every connected pad; otherwise a `footprint_update_conflict` conflict names the pad and the old/new counts. Missing or ambiguous mapping, unknown reference, unresolvable library, reference excluded by filters: conflicts, nothing prepared.
- A replacement that moves the centre of a pad on a routed net is refused (`routed_pad_movement_requires_opt_in`, naming the pads) unless the reviewed plan carries `allow_routed_pad_movement=true`. Copper is never modified; the response states connectivity is `not_verified_requires_drc`.
- `scope: "models"` is the bulk form of `set_placed_footprint_models`: one dry run lists before/after models per footprint (`model_inspections`), apply sends one `UpdateItems` batch in one commit; only the Models domain of the selected footprints changes (selection by `references` and/or `library_ids`). It does not parse or rebuild pads, so libraries the full refresh refuses still work.
- `update_pcb_from_schematic`'s `footprint_id_changed` conflict now tells the caller to use `update_footprints_from_library` replacements (message text only).

## Red then green

- Red (negative controls, each mutation applied to the finished code, then reverted; the new tests are the guards):
  - routed-pad guard disabled: 2 failed (`swap_that_moves_a_routed_pad_needs_explicit_opt_in`, `served_swap_over_routed_pads_refuses_then_applies_only_with_the_reviewed_opt_in`), 62 passed.
  - connected-pad uniqueness check disabled: 3 failed (`replacement_refuses_ambiguous_connected_pad_mapping_without_changing_input`, `swap_with_a_missing_connected_pad_is_refused_naming_the_pad_and_counts`, `swap_with_an_ambiguous_connected_pad_mapping_is_refused_by_the_planner`), 61 passed.
  - models scope falling back to the full rebuild: 3 failed (`fifty_seven_model_refreshes_are_one_bulk_plan`, `served_bulk_models_apply_57_footprints_in_one_commit_touching_only_models`, `bulk_models_scope_does_not_rebuild_pads_even_when_the_library_has_unsupported_pad_clauses`), 61 passed.
- Also red during authoring: my first expectation for the missing-pad message ("removes connected pad") failed; the code reports the stricter unique-mapping message first, the test was corrected to that.
- Green: `cargo test -p konnect-core --lib tools::pcb_footprint_update::tests::`: after applying the patch 48 passed / 0 failed; with the 16 added tests **64 passed / 0 failed / 0 ignored**.

Coverage added (all in `crates/konnect-core/src/tools/pcb_footprint_update.rs`): identity/linkage/net preservation; missing and ambiguous mapping; unknown/unselected/unresolvable inputs; routed-centre movement refusal, opt-in and revision binding (unit and served through a mock IPC with a routed track); front/back side at 0/37/90/180/270 degrees (position, rotation, side, pad layers, no-op rebuild); served schema and tools/list; invalid arguments refused before IPC with `invalid_argument`; stale guards (wrong replacement set, wrong scope, bad revision: no commit opened, no update sent); 57-footprint bulk apply in exactly one batch and one commit with only Models changed, reference-selected subset leaving others byte-identical; failure outcomes for swap and 57-model bulk (update failure drops the commit, readback mismatch rolled back with originals restored, unconfirmed restore never claims rollback).

## Full run

`cargo test --workspace --lib --tests --no-fail-fast` with TEMP/TMP pointed at a clean directory (`C:\kz47tmp`): **2460 passed, 0 failed, 45 ignored** across all test binaries (the ignored are pre-existing native/slow tests, not run).

Honest note on earlier runs: with the default shared `%TEMP%` the same command shows ~99-104 failures, all in schematic/ownership/IPC-message tests. Cause: unrelated stray `*.kicad_sch` files in `%TEMP%` make project-ownership checks conflict, and the tests that expect "no listener" / library paths depend on the environment. They are unrelated to this change (none touch `pcb_footprint_update`); they pass with a clean TEMP. I did not run the base commit separately.

`rustfmt --edition 2021 --check` clean on the two changed files; `git diff --check` clean.

## Items 1 and 4 (confirmed, not changed)

- Item 1 (pad clauses, models and description refresh): AILAB-48 commits `2d35f76`, `4b814d5`, `b0f69b4`, with native 10.0.6 evidence in `docs/AILAB-48-VERIFICATION.md`; its tests are inside the 64.
- Item 4 (`${KICAD10_3RD_PARTY}` resolution): AILAB-49 `110edf5`, `f6303e8`.

## Limits

- Native KiCad verification of replacement and bulk models is **not done and not authorized**: no KiCad GUI or IPC endpoint was started or contacted (a user kicad.exe was running on the machine and was not touched). All IPC behaviour is against the in-process mock that mimics KiCad 10.0.6 (updates visible only after commit push).
- A native run would need: the scratch hidden-desktop editor harness of AILAB-48 (`tests/ailab48_native_*.py`) extended with a swap fixture (two libraries, a placed footprint with nets, rotated, on both sides, with and without routed copper) and a 57-footprint board; check dry-run plan, stale revision, apply, saved-board reopen (footprint ID, position, orientation, side, pad nets, models), a second dry run being a no-op, and Edit > Undo returning to the original in one/two steps. Real copper connectivity after a routed-pad swap (DRC) is also unverified.
- The PCB573 acceptance scenario (66 footprints, D1 swap) was not run: that design repository is off limits.
- The in-commit stale recheck against a live concurrent edit is covered only by the existing revision guard tests, not by an injected concurrent change.
