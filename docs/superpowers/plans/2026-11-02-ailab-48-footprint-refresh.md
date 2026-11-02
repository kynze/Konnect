# AILAB-48 footprint refresh plan

Use the existing library refresh planner and native IPC transaction rather than add a parallel writer. Preserve pad-local settings as typed protobuf messages, rejecting unsupported or malformed clauses before mutation. Extend the existing tool with explicit reference-to-library-ID replacement and a models-only scope, so one dry-run plus one apply serves a bulk selection.

Preserve instance identity/linkage, position, side, orientation and nets; require unambiguous pad-number mapping on replacement. Routed copper remains untouched; moved connected pad centres must refuse until reconnection is explicitly handled. Hash selection, requested operation, library content and board state; verify by independent live readback within the undo transaction.

Sequence: regression red for mask margin/thermal angle; typed preservation green; replacement guards and back-side tests; bounded 57-model batch and 72/66-pad analogue; served schema/dispatch and in-process transaction failures; focused broader check and owned-file review. No protected file text writes, installed-server changes or default IPC. Native KiCad 10.0.6 save/reopen/undo requires coordinator scratch IPC lease and remains open if unavailable.

## Concrete interfaces and verification

Owned implementation is `crates/konnect-core/src/tools/pcb_footprint_update.rs`; `pcb_sync.rs` will direct changed-ID conflicts to the reviewed replacement workflow rather than silently changing packages during schematic sync. Existing public tool name and default full refresh stay compatible. Add `scope: full|models` and `replacements: [{reference, library_id}]` plus `allow_routed_pad_movement` (false by default). Arrays avoid introducing a new extensible JSON map. Replacement entries must select exactly one reference; duplicate/missing/filtered-out entries refuse.

Typed library pad overrides carry explicit presence (including zero) for mask/paste margins and ratio, clearance, die length, thermal angle/gap/width and zone connection. Unknown pad clauses remain precise refusals. Models-only refresh replaces the model list without rebuilding pads or other library domains, allowing unrelated unsupported geometry to remain untouched.

Readback compares requested content and preserved instance/net data against a separate GetItems response before publishing the single commit. Refusal and stale revisions never open a transaction; update/readback failures drop it with existing IPC uncertainty handling. Routed movement requires explicit opt-in and reports pad numbers, untouched copper and connectivity not verified; actual connectivity needs native DRC.

- [ ] Observe regression failure for each original pad clause.
- [ ] Implement and verify typed pad preservation, zero/omitted distinction and invalid-clause refusal.
- [ ] Observe changed-ID regression; implement replacement/mapping guards, side transforms and stale behavior.
- [ ] Verify 72 instances (66 with the former clauses) and 57-model bulk planning/apply through a bounded batch.
- [ ] Verify served schema/dispatch and update/readback/transaction failure outcomes.
- [ ] Request scratch IPC lease, document any unavailable native save/reopen/undo proof.
- [ ] Final focused broader check, owned diff review, verification report and local commits.
