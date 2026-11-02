# AILAB-49 verification

Status: resolver acceptance complete for local Quality Check; no release or integration performed. Final reviewer recheck remains the coordinator's gate.

## Scope and behavior

Expected base d38a6ed6a118bc51f775517a4a1e9b64a9ed9964 and origin/upstream remotes verified in the initial dispatch; resumed at owned WIP e07ae83a868c516773db2556535cf124def478ad. Installed 0.13.0 / 6bbe3e4 was neither replaced nor used as modified-build evidence.

The unchanged expand_lib_uri signature delegates to a dedicated module declared inside library.rs. Standard third-party roots select a nonempty exported override first, then a configured kicad_common.json variable, then the KiCad default root; existence is checked only after source selection. A missing explicit override refuses instead of silently choosing another same-name library. An empty exported value is ignored as KiCad does. Other variable kinds retain their previous behavior. KIPRJMOD stays bound to table/project context. No personal paths are hard-coded and settings reads remain read-only.

## Independent KiCad evidence

Installed `C:/Program Files/KiCad/10.0/bin/kicad-cli.exe version`: 10.0.6. Standalone bundled Python `pcbnew.Version()` also reports 10.0.6, with scratch KICAD_CONFIG_HOME and KICAD_DOCUMENTS_HOME.

Upstream source pinned to tag 10.0.6:

- [COMMON_SETTINGS::InitializeEnvironment](https://github.com/KiCad/kicad-source-mirror/blob/10.0.6/common/settings/common_settings.cpp#L768) initializes the versioned 3RD_PARTY variable even when not exported or configured.
- [PATHS::GetDefault3rdPartyPath](https://github.com/KiCad/kicad-source-mirror/blob/10.0.6/common/paths.cpp#L120) uses the user document path and appends 3rdparty; getUserDocumentPath checks KICAD_DOCUMENTS_HOME, then appends the platform KiCad directory and major/minor version.
- [Platform directory case](https://github.com/KiCad/kicad-source-mirror/blob/10.0.6/include/paths.h) defines KiCad for Windows/macOS and kicad for other platforms.
- [Unix base](https://github.com/KiCad/kicad-source-mirror/blob/10.0.6/libs/kiplatform/os/unix/environment.cpp#L87) uses user data, while [Windows](https://github.com/KiCad/kicad-source-mirror/blob/10.0.6/libs/kiplatform/os/windows/environment.cpp#L90) and [macOS](https://github.com/KiCad/kicad-source-mirror/blob/10.0.6/libs/kiplatform/os/apple/environment.mm#L75) use Documents.

No installed user configuration was needed or read for evidence. No personal paths are hard-coded in the implementation.

## Observed red/green

1. Original missing default: before adding the fallback, a standalone Rust harness with an injected documents root and an existing third-party library with spaces returned None instead of the path. The identical harness passed after adding the fallback.
2. Reviewed P1: `cargo test -p konnect-core --lib ailab49_missing_override -- --nocapture` failed before correction: left was the existing default Parts.pretty, right was None. After selecting the effective source before target validation, the regression passed for both exported and configured overrides.
3. Empty export: a standalone Rust harness failed with None instead of the default path before filtering empty exported values; the identical harness then exited 0. Persistent tests cover both configured and default fallback for an empty export.

## Final commands and counts

- `cargo test -p konnect-core --lib uri_resolver -- --nocapture`: 7 passed before the empty-export case was added.
- `cargo test -p konnect-core --lib tools::library:: -- --test-threads=1`: **178 passed, 1 failed, 0 ignored**. All **8 resolver tests passed**, covering precedence, missing targets and overrides, unknown variables, version isolation, empty export, spaces, Windows separators, KIPRJMOD and stock footprint-directory compatibility.
- The one broader failure was existing `symbol_source_tests::a_sheet_in_a_subdirectory_resolves_against_the_project_table`, before URI resolution: ProjectConflict named the fixture root plus chk.kicad_sch and chk2.kicad_sch under the normal user TEMP ancestry. A targeted rerun with another child of normal TEMP still failed; the exact unchanged compiled test with TEMP/TMP set to a unique directory under C:/tmp **passed (1 passed, 0 failed)**. No unrelated file was changed or removed. The full group was not rerun after this environment isolation because the coordinator requested final handoff.
- `cargo build -p konnect --bin konnect`: final incremental build succeeded after the empty-export change (29.90s). Task binary: C:/tmp/konnect-ailab49-target/debug/konnect.exe.
- `python tests/ailab49_library_resolution.py <task-built-binary> --root-source <mode>` passed for **7 modes**: default, configured, exported, missing-configured, missing-exported, empty-default, empty-configured. Each mode checked its expected lookup/refusal plus unknown-variable and missing-directory refusals: **21 served assertions passed**. The default run included `--native-python C:/Program Files/KiCad/10.0/bin/python.exe`: native 10.0.6 loaded the same builder-generated footprint and independently reported one pad (**1 native payload assertion passed**).
- `python -m py_compile tests/ailab49_library_resolution.py`, owned-file rustfmt check and `git diff --check`: passed.

The served harness creates the footprint and library tables through the built server's existing tools; it never patches design/library-table text. It uses disposable child-process config, state and documents roots and an explicit private inproc endpoint; no IPC tool is invoked. Pure resolver tests inject sources and do not mutate process environment; harness environment changes exist only in child processes.

## Shared refresh boundary and limits

`pcb_footprint_update.rs::plan_updates` lines 636-647 calls unchanged `library::resolve_footprint_path` and maps Err to `footprint_library_resolution_failed`. That resolver flattens the same tables through expand_lib_uri as served get_footprint_info. This task verifies the common resolver and served lookup/refusal, not the refresh diagnostic envelope or native PCB replacement.

Native FootprintLoad on an expanded path proves payload compatibility, not native variable expansion. Default path semantics are independently grounded in pinned KiCad 10.0.6 source above. Windows execution is observed; Linux/macOS logic is source-grounded but not executed there. No live editor/default API endpoint, native IPC, user design/configuration changes, installed server replacement, push, tag, merge, deployment or release occurred. No ignored tests or full workspace-suite claim.

## Handoff

Reviewed P1 and empty-export defects are corrected and tested. Remaining validation limit: full broader group has its original environment-dependent 178/1 result, with the failing test passing in isolated C:/tmp; coordinator may rerun the group there if required. Next owner is coordinator/reviewer for final SHA review, integration and release gates. This worker stops at Quality Check and starts no follow-on task.

Build setup used existing Rust/protoc/CMake with CARGO_BUILD_JOBS=2, task-owned CARGO_TARGET_DIR=C:/tmp/konnect-ailab49-target and worktree-local KONNECT_STATE_DIR; no shared target/state or installed configuration was changed. Earlier interrupted build/setup failures are not counted as regression evidence.
