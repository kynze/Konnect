# AILAB-50 independent review of AILAB-49 third-party library resolution

## Final verdict: APPROVED for release readiness of the AILAB-49 resolver, with non-blocking P3 notes

No P0, P1 or P2 finding remains. The two earlier defects (wrong-default substitution and empty-export handling) are fixed in code and covered by tests that I ran. The remaining items are small divergences from KiCad or evidence gaps, none of which can select a wrong library.

- Reviewed commit: `dcb29c55df44fe754e73f0dfb306ad5e1cdc9049` (tip of `jbp-kynze/konnect-integration`, the Merge of AILAB-45; also carries AILAB-41/42/48). The AILAB-49 commits `e07ae83`, `110edf5` and `f6303e8` are ancestors of it. `library_uri_resolver.rs` and `tests/ailab49_library_resolution.py` are unchanged between `f6303e8` and the reviewed tip.
- Review date: 2026-11-13. Reviewer worktree branch: `jbp-kynze/ailab-50-review`.

## What I executed

`cargo test -p konnect-core --lib uri_resolver` (Windows, Rust 1.96.0, `CARGO_TARGET_DIR=<worktree>/target-ailab50`, jobs=2): **9 passed, 0 failed, 0 ignored**. That includes `ailab49_missing_override_does_not_select_default_library`, `ailab49_empty_export_uses_configured_or_default_root`, `ailab49_empty_configured_value_falls_back_to_the_default_root`, `ailab49_preserves_exported_configured_default_precedence`, and the Windows separator test. The count matches the claim in AILAB-49-VERIFICATION.md (9 on Windows).

I did NOT run: the served harness `tests/ailab49_library_resolution.py` (7 modes), the native `pcbnew` load, the broader `tools::library::` group (claimed 178 passed / 1 environment-dependent failure), or any Linux/macOS build.

## Re-check of prior findings

| Prior finding | Status | Evidence |
|---|---|---|
| P1: missing explicit override resolved a same-name default library | **Resolved** | `library_uri_resolver.rs:41-68` picks the source (nonempty export, then nonempty configured, then default) before testing existence, and returns `None` if the chosen target is absent. Unit test at `:141-160` covers both export and configured; passes. |
| P2: empty exported `KICAD10_3RD_PARTY` treated as an explicit root | **Resolved** | `:50` filters empty exports, matching KiCad 10.0.6 `InitializeEnvironment` (`wxGetEnv(...) && !envValue.IsEmpty()`). Tests at `:114-138` pass. |
| Evidence boundary: native comparison loads an absolute path only | **Still true, now disclosed** | AILAB-49-VERIFICATION.md says native `FootprintLoad` on an expanded path proves payload compatibility, not variable expansion. This is honest. See F5. |
| Default-root construction vs KiCad source | **Confirmed** | `paths.cpp` `getUserDocumentPath` uses `KICAD_DOCUMENTS_HOME`, else `GetDocumentsPath()`, then `KiCad`/`kicad` and major.minor, then `3rdparty`. Windows and macOS use Documents (`dirs::document_dir`); Unix uses the user data dir, which `g_get_user_data_dir` (XDG_DATA_HOME or `~/.local/share`) and `dirs::data_local_dir` both give (`library.rs:1799-1809`). |

## Findings (all non-blocking)

### F1 — P3: empty configured value is ignored, KiCad keeps it
`library_uri_resolver.rs:53-58`. KiCad 10.0.6 `common_settings.cpp` (settings loader, around L205-232) calls `m_Env.vars[key].SetValue(val)` with no emptiness check, so a configured `""` becomes the variable's value, and `${KICAD10_3RD_PARTY}/X.pretty` expands to `/X.pretty`. Konnect instead falls back to the default root. This resolves to a library KiCad would not load. The AILAB-49 report says this was not checked; I checked the source, but did not observe it in a native session. Reproducer: `kicad_common.json` with `{"environment":{"vars":{"KICAD10_3RD_PARTY":""}}}` plus an existing default-root `Parts.pretty`; Konnect resolves the default, KiCad (per source) does not. Fix is optional: either keep the leniency and document it as parity-breaking, or refuse.

### F2 — P3: empty `KICAD_DOCUMENTS_HOME` is ignored
`library.rs:1805-1809`. KiCad's `PATHS::getUserDocumentPath` uses `wxGetEnv(...)` returning true even for an empty value, so it builds a path relative to the working directory. Konnect falls back to the platform Documents directory. Same kind of leniency as F1; unlikely in practice.

### F3 — P3: `KICAD9_3RD_PARTY` / `KICAD8_3RD_PARTY` get default roots
`library_uri_resolver.rs:41-46`. KiCad 10.0.6 only registers the running version's variable, so these only resolve there if exported or configured. AILAB-49-VERIFICATION.md states this as deliberate leniency; I agree it is stated truthfully and is covered by `ailab49_default_root_is_version_specific`. Note it in release notes, not a code change.

### F4 — P3: configured keys and values are not trimmed
`library.rs:1790`. KiCad trims both key and value (`Trim(true).Trim(false)`) when loading `environment.vars`. A hand-edited value with trailing whitespace resolves in KiCad but fails the existence check here (fails closed, so no wrong library). Reproducer: `"KICAD10_3RD_PARTY": " C:/Parts "`.

### F5 — P3 (evidence): served and native claims are accurate but narrow
`tests/ailab49_library_resolution.py:60-112`. By reading, the claim "7 modes, 21 served assertions, 1 native assertion" is internally consistent and the decoy design (override root plus a 2-pad same-name library in the default root) does detect a silent default fallback. Gaps: there is no served mode for an empty value inside `kicad_common.json` (only unit test coverage), and the native step loads an already-expanded absolute path, so no native `${KICAD10_3RD_PARTY}` expansion, no library-table lookup and no refresh envelope is demonstrated. The `empty-configured` mode actually sets an empty *export* with a nonempty configured value, so its name is slightly misleading. I did not rerun the harness, so I state it as reviewed-by-reading, not independently reproduced.

### Observations with no finding
- KIPRJMOD stays bound to the table's directory (`:31-34`) and is tested with a hostile exported value.
- Paths with spaces and Windows backslash tails are tested (`:183-201`, `:353-371`); `trim_start_matches(['/', '\'])` handles both.
- Missing roots, missing default document root, unknown and malformed `${` variables return `None` (`:242-270`).
- Environment restoration: the new resolver tests inject sources and never touch the process environment. Existing `expand_lib_uri_expands_a_kicad_env_var` restores `KICAD10_FOOTPRINT_DIR` through a guard. Pre-existing, unrelated: `library.rs:9696` sets `KICAD10_SYMBOL_DIR` under `KICAD_ENV_LOCK` without restoring it; not introduced by AILAB-49 and not related to 3RD_PARTY.
- `integration.rs:1769` has a separate `KICAD*_3RD_PARTY` handling for freerouting. Out of scope for this resolver.

## Review limits

- No KiCad GUI, IPC, native session or PCB573 repository was opened; no installed config was read.
- Only Windows was executed. Linux and macOS behavior is source inference from KiCad 10.0.6 (`paths.cpp`, `common_settings.cpp`, `kiplatform/os/*/environment.*`).
- I did not run the served harness, the native `pcbnew` check, or the broad `tools::library::` group, so the 178/1 count and the "isolated TEMP passes" claim are unverified by me.
- KiCad behavior for F1/F2/F4 is from source reading only.
- No product code was edited; `target-ailab50/` is untracked build output and not committed.

---

# Earlier checkpoints (superseded by the verdict above)


Status: incomplete, no readiness verdict. The coordinator requested a clean interruption before the final author commit was supplied/rechecked. This report preserves the initial snapshot review only; it does not approve release, merge or native PCB behavior. AILAB-45 delivered files remain unchanged.

## Reviewed snapshot

Read-only target: the assigned ailab-49-library-resolution worktree. Initial HEAD was `d38a6ed6a118bc51f775517a4a1e9b64a9ed9964`; the resolver and harness were untracked and library.rs modified. No final author SHA was supplied or reviewed.

Snapshot hashes (SHA-256):

| Artifact | Hash |
|---|---|
| `git diff -- crates/konnect-core/src/tools/library.rs` stdout | `1fc5bf92906e0fccd2653f0dc64f29659124c7694a2259af5c59428dd80bca4e` |
| `crates/konnect-core/src/tools/library_uri_resolver.rs` raw bytes | `e9094f67db57efc6b5cdf910378bd864e9d4c11d74cb103fa96abf8a6fb240cc` |
| `tests/ailab49_library_resolution.py` raw bytes | `1ccee9d8f428bed0a35d53a5a4bca480ec4a52c2c20637af6038f81bb52b3742` |
| `docs/AILAB-49-VERIFICATION.md` raw bytes | `cc8b183fb90c20127083381268d583cccc0fbb0c4ef1e1cce439ef8fda49cc96` |

## P1 — Explicit missing override resolves a different default library

Location: `library_uri_resolver.rs`, initial snapshot lines 41–56 (process/config candidate existence checks) and 70–80 (new default fallback). The new third-party default is reached after an explicitly selected environment/configured root fails its existence check. An existing same-named library in the default location then resolves successfully, potentially selecting the wrong footprint instead of refusing the broken override.

Observed independent reproducer: copy the exact resolver snapshot into TEMP as a Rust module; create only `KiCad/10.0/3rdparty/Parts.pretty` under an injected temporary document root; resolve `${KICAD10_3RD_PARTY}/Parts.pretty` twice, first with a process value pointing at a missing root and then with a configured value pointing at a missing root. Compile a dependency-free wrapper with existing `rustc --edition=2021`; no Cargo build, author target, protected-file write or native editor is involved. Both cases returned the default directory; the desired-contract assertion that both results are unresolved exited **101**, while compilation exited **0**.

Expected behavior is source-grounded, not independently observed in a native KiCad session: [KiCad 10.0.6 InitializeEnvironment](https://github.com/KiCad/kicad-source-mirror/blob/10.0.6/common/settings/common_settings.cpp#L768) selects a nonempty external value independently of filesystem existence. Its [settings loader](https://github.com/KiCad/kicad-source-mirror/blob/10.0.6/common/settings/common_settings.cpp#L188) retains external precedence over settings. A default should therefore be used only without an effective override for this newly supported variable; a missing selected target should remain unresolved. Existing legacy stock-variable fallback policy need not be broadly refactored for this correction.

Sent promptly to the coordinator in messages `msg_3eb7dba54933` and `msg_bcbdedf07ddc`. Author owns correction and regression tests, especially missing explicit root + existing default root, with distinct footprints to detect silent substitution. The current test labelled precedence deliberately permits falling from an absent external target to settings; it does not test or justify faithful new-variable behavior when only the default target exists.

## Evidence boundary — native comparison is absolute-path payload loading

The inspected served harness calls `pcbnew.FootprintLoad` with an already-expanded absolute `.pretty` directory. This can establish that native KiCad loads the same footprint file and pad count; it does not exercise native `${KICAD10_3RD_PARTY}` expansion, default root selection, environment/config precedence, a library-table lookup, refresh diagnostic envelopes or native IPC replacement. The author report should retain this distinction; source evidence supports path-policy inference without inventing observed native resolution coverage.

## Source-grounded checks completed before interruption

The default-root construction is consistent with the pinned source for an absent override: [paths.cpp](https://github.com/KiCad/kicad-source-mirror/blob/10.0.6/common/paths.cpp#L32) uses KICAD_DOCUMENTS_HOME as the base when present, adds the platform KiCad directory and version, then adds 3rdparty. [paths.h](https://github.com/KiCad/kicad-source-mirror/blob/10.0.6/include/paths.h#L31) chooses `KiCad` on Windows/macOS and `kicad` elsewhere. Platform implementations use Documents on [Windows](https://github.com/KiCad/kicad-source-mirror/blob/10.0.6/libs/kiplatform/os/windows/environment.cpp#L90) and [macOS](https://github.com/KiCad/kicad-source-mirror/blob/10.0.6/libs/kiplatform/os/apple/environment.mm#L75), and the user-data directory on [Unix](https://github.com/KiCad/kicad-source-mirror/blob/10.0.6/libs/kiplatform/os/unix/environment.cpp#L87). Cross-platform behavior remains source inference; only the small reproducer executed, on Windows.

Static inspection found explicit tests for spaces, missing default targets, missing document root, unknown/malformed variables, version-specific directories, KIPRJMOD contextual binding, stock-directory lookup and Windows backslashes. These author tests were read, not executed by this reviewer; no full build/test suite was duplicated. The wrapper's KICAD_DOCUMENTS_HOME and platform-directory acquisition, relative/empty-root semantics, existing-directory corner cases and older-version defaults were not fully independently exercised. The code retains contextual KIPRJMOD handling; that observation alone is not a full regression verdict.

## Remaining release-review blockers and next owner

1. Author corrects or resolves the explicit-override precedence finding and supplies regression evidence.
2. Coordinator supplies the final full author commit; reviewer rechecks its exact diff and evidence before any readiness verdict.
3. Keep native default-root expansion and refresh/native IPC verification explicitly unverified unless separately demonstrated safely.

No production/default IPC, installed configuration/binary, real design, PCB573 repository, author source/test/report modification, nested worker or remote publication was used. Coordinator owns interruption state and later review resumption; author owns corrections. This local WIP report is the only owned change for AILAB-50.

## Resumed dispatch checkpoint and final user wrap-up

Review resumed under dispatch `ctx_de4e05aaef6c`; the coordinator explicitly superseded the first interruption, then requested final wrap-up before the author supplied a final SHA. Target HEAD observed at resumption was `e07ae83a868c516773db2556535cf124def478ad` (WIP). The interim read-only diff showed an uncommitted branch that chooses the third-party source before testing existence, addressing the original P1 in code, but this was not revalidated against a final author commit and is **not a closed finding** in this review.

A follow-up was sent as `msg_6de590df2153`: the uncommitted guard used `process_var(var).map(...)`, treating an empty exported `KICAD10_3RD_PARTY` as an explicit root. KiCad's pinned source requires a nonempty external value. An empty external value should allow configured/default selection; otherwise joining a relative tail can refuse a valid default or select a current-working-directory library. This is a source-grounded P2 follow-up; no additional independent execution was performed for it. Author owns the empty-process-value filter and regression covering both existing configured and default targets; configured-empty semantics are separate.

No final author full SHA was supplied, and the coordinator's final wrap-up explicitly ended the wait. Final-source/test/evidence review therefore remains pending; no final readiness verdict is issued. The independent executed evidence remains exactly the initial dependency-free TEMP Rust compilation (exit 0), two demonstrated wrong-default resolutions, and desired-contract assertion failure (exit 101). No new Cargo builds, broad suites, served/native calls, scratch editor provisioning or IPC occurred during resumed review. The coordinator reported the author's served checks were still in progress; that is status, not reviewer-observed passing evidence.

Next worker: obtain the final AILAB-49 commit and updated verification report, recheck the P1 override and empty-environment regressions, execute a small isolated resolver matrix if needed, and issue the readiness verdict only against that exact final SHA. Keep native URI expansion, refresh diagnostics, native IPC and cross-platform behavior separately qualified. This is a safe local WIP checkpoint with incomplete acceptance, not a completed review.
