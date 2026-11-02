# AILAB-48 verification

## Current checkpoint

Library-refresh-only implementation is ready for independent review; **native verification and overall AILAB-48 acceptance remain open**. No release, installed runtime/configuration change, default IPC connection or production design access occurred. Coordinator owns further integration and Plane state.

Verified base: d38a6ed6a118bc51f775517a4a1e9b64a9ed9964; origin https://github.com/kynze/Konnect.git. CARGO_TARGET_DIR is this worktree's `target-ailab48`, KONNECT_STATE_DIR is `.state-ailab48`, CARGO_BUILD_JOBS=2. Existing local Rust/protoc/CMake toolchain was reused; no installation. Pre-existing `.serena/` and untracked build output are excluded from commits.

## Library refresh slice

The typed refresh now reads and emits optional pad mask/paste margin and ratio, clearance, die length, zone connection, thermal gap/width/angle without silently dropping them. Explicit zero remains present. Unsupported pad clauses still refuse by name and malformed overrides refuse before mutation. Existing description and model refresh is reused and verified, including ordered model lists. Independent live GetItems readback and a second in-commit stale check precede publishing the single undo transaction; typed IPC uncertainty/recovery responses are preserved. Native inherited thermal angle 90 is normalized for comparison without removing explicit values from emitted messages.

Observed regression red:

- `cargo test -p konnect-core --lib pad_local_mask_and_thermal_settings_survive_refresh -- --nocapture`: 0 passed / 1 failed; `pad clause 'solder_mask_margin' is not supported by typed library refresh`.
- Isolated compiled test binary, `thermal_bridge_angle_is_representable`: 0 passed / 1 failed; `pad clause 'thermal_bridge_angle' is not supported by typed library refresh`.

Observed green: `cargo test -p konnect-core --lib tools::pcb_footprint_update::tests:: -- --nocapture` on the separated library-refresh slice: **39 passed / 0 failed / 0 ignored**, 7.18 seconds. Includes exact all-setting values on both sides at 0/37/90/180/270 degrees, zero/omitted checks, invalid/refused values, 72 instances including 66 formerly rejected clause cases, existing model/description refresh, served schema/dispatch, unchanged stale/no-commit behavior, one native transaction in the mock, update failure drop and independently observed readback-mismatch drop/restored input. Served success uses four footprint snapshots across dry run plus apply (plan, in-commit recheck, independent readback), one update batch and one commit. `rustfmt` and `git diff --check` passed for owned changes.

Earlier combined WIP runs were 29 passed / 7 failed then 31 passed / 5 failed, followed by 36/36 after normalization corrections; these do not substitute for the separated slice's result. Initial toolchain PATH/CMake failures and a test import error were corrected. One overlapping executable invocation caused an intermediate Windows LNK1104; later compilation completed. No ignored/native test has been treated as passing.

## Deferred work and limits

Replacement and bulk-model work is preserved separately in task TEMP (`ailab48-full.rs`) and deliberately excluded from the first library-refresh commit. Its original changed-ID regression was observed red (`Test:Socket` did not match `Test:Replacement`), and the 57-model regression was observed red (full refresh touched Pads/Graphics/Attributes/Metadata/Models rather than Models). The combined WIP later passed those limited pure tests, but broader missing/ambiguous mapping, routed-centre movement, bulk IPC apply, stale guards and native transforms still need explicit final coverage. `pcb_sync.rs` and `placed_footprint_models.rs` are unchanged.

Exclusive scratch IPC lease was granted for this dispatch. No scratch editor/socket was pre-provisioned; exact KiCad 10.0.6 source confirms the socket uses wxStandardPaths::GetTempDir and config uses KICAD_CONFIG_HOME, so a bounded isolated TEMP/config setup will be attempted after the first commit. Native 10.0.6 refresh/readback/save/reopen/undo and actual copper connectivity remain **unverified** at this checkpoint. No default-endpoint fallback is authorized.

Still required: negative-control evidence for new guards, final broader relevant check, bounded native attempt or explicit limitation, independent Quality Check, and remaining task acceptance. Parent AILAB-47 / URI resolution belongs to separate work. Do not claim AILAB-47 or full AILAB-48 complete from this slice.

Native isolation source references: [10.0.6 API server](https://raw.githubusercontent.com/KiCad/kicad-source-mirror/10.0.6/common/api/api_server.cpp), [10.0.6 settings path](https://raw.githubusercontent.com/KiCad/kicad-source-mirror/10.0.6/common/paths.cpp), [10.0.6 API preference](https://raw.githubusercontent.com/KiCad/kicad-source-mirror/10.0.6/common/settings/common_settings.cpp).


## Final user-requested handoff

User requested final wrap-up during native preparation; no further implementation or broad suites were started. First independently reviewable library-refresh commit: `0f4fd01d6efa1d0bcc806d795c2e1e0735b46b1e` (39 focused tests green). Overall task outcome is **failed/incomplete checkpoint**, because native, negative-control and remaining replacement/bulk acceptance are open; this is not a release or complete Quality Check.

Deferred replacement/bulk work is durably preserved as `docs/AILAB-48-DEFERRED.patch`, relative to the first commit's source. It is **not applied to the runtime source** and is not approved for integration. The earlier combined version passed 36 limited focused tests; additional all-setting tests and merged served/readback tests in the preserved patch have not been compiled together. Next owner should inspect the patch, apply it only in a task-owned worktree, then cover missing/ambiguous mapping, movement opt-in, exact side transforms, stale/no-mutation outcomes, 57-model bounded IPC apply and failure outcomes. Keeping this patch separate allows review of the library slice without including unfinished features.

Native attempt was bounded to exact 10.0.6 source inspection and scratch fixture preparation through installed pcbnew native Python APIs. Preparation stopped before FootprintSave/SaveBoard because the SWIG LSET constructor rejected positional/list layer arguments; the discovered AddLayer API was not tried after wrap-up arrived. No scratch editor was launched, no scratch socket was provisioned, no default/live IPC was contacted, and there is no running task-owned native process to stop. Only task TEMP directories/config JSON and scripts/source downloads remain; no protected design/library files were written by this preparation. Exclusive scratch lease may be released by the coordinator. Native preservation/save/reopen/undo remain explicitly unverified.

Final local checks: owned changes reviewed; `git diff --check` passed. No final broad suite or hosted CI was started after the wrap-up request. Next owner/coordinator must perform the remaining checks and independent review before integration/release. Installed 0.13.0 / 6bbe3e4 was never replaced or used as proof of modified code. No production repository was opened.
