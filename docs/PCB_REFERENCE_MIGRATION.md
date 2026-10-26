# Reference-only PCB synchronization

`update_pcb_from_schematic` accepts optional `reference_only: true` for an
annotation migration on a **closed, saved PCB**. Default live full-sync behavior
is unchanged. This explicit mode is a file transaction inside Konnect, not an
implicit fallback after a live IPC failure.

It requires one saved footprint per exported symbol UUID path. It never relinks
by reference, adds parts, changes values/fit state, reassigns pad nets, moves
geometry, or refreshes libraries. Final references are checked simultaneously,
allowing permutations while refusing collisions with retained board-only items.
Missing/ambiguous identities and unassigned schematic footprints are refused.
Unannotated or non-ASCII targets are rejected before returning a ready plan.
Skipped symbols are retained without renaming.

Close and save the PCB before both dry-run and apply:

```json
{
  "schematic": "C:/trial/hardware/design.kicad_sch",
  "board": "C:/trial/hardware/design.kicad_pcb",
  "reference_only": true,
  "dry_run": true
}
```

Review `status`, `coverage`, `changes` and `diagnostics`. Apply with `dry_run:false`
and the exact `expected_plan_revision` from the reviewed dry-run. The revision
covers the complete saved board bytes and source plan. Open-board editor locks
are refused. A changed board or schematic requires a fresh dry-run.

Each rename edits only the validated quoted Reference value in its UUID-owned
footprint, using shared string-aware S-expression spans and the guarded atomic
writer. Replacement is parsed and checked before writing. Exact saved readback
is required. Every other byte is retained, including unit mappings, unknown
fields, drawing tables, net codes, copper, fill caches and reference positions.
There is no native undo entry; the file transaction and Git diff provide recovery.
No `save_project` call is needed. Reopen the resulting PCB for visual inspection.
`coverage.transport` is `saved_board_file`; atomicity is
`revision_checked_atomic_file_replace`.

This avoids observed KiCad 10.0.6 IPC limitations: footprint updates are staged
as replacements, and field updates reach an inherited CopyFrom implementation
that only accepts PCB_TEXT. The footprint protobuf also lacks saved unit-map
metadata. Sources: [update handler](https://github.com/KiCad/kicad/blob/10.0.6/pcbnew/api/api_handler_pcb.cpp),
[text CopyFrom](https://github.com/KiCad/kicad/blob/10.0.6/pcbnew/pcb_text.cpp),
[footprint serialization](https://github.com/KiCad/kicad/blob/10.0.6/pcbnew/footprint.cpp).
Real-copy IPC attempts were refused/rolled back before this saved-file path was
chosen; no real design was edited by those attempts.

Generated net names containing previous references deliberately stay unchanged.
Those names are internal net identities, not component reference fields. A
subsequent full sync can still report their differences; that requires a separate
reviewed net-name migration rather than relaxing routed-net guards.

MEZZA-194 initially encountered 146 permutation refusals and two routed generated
net-name refusals in the existing full-sync planner. Regression coverage includes
swaps, final collisions, missing identities, byte preservation of unit/unknown
metadata, stale plans, editor locks, invalid targets and exact saved readback.
