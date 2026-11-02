# AILAB-50 independent review — WIP interruption checkpoint

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
