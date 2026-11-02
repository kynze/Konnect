# AILAB-49 verification

Status: WIP checkpoint at coordinator-requested clean interruption. Acceptance incomplete; no release claim.

## Scope and behavior

Base: d38a6ed6a118bc51f775517a4a1e9b64a9ed9964; origin kynze/Konnect, upstream mixelpixx/Konnect verified. Only library URI resolution, dedicated resolver tests, a uniquely named served harness and task docs are owned. Installed 0.13.0 / 6bbe3e4 was not replaced or used as changed-build proof.

The existing process environment and read-only kicad_common.json environment.vars lookups remain first. The new fallback resolves KICAD10_3RD_PARTY (also supported 9/8 versions) to the corresponding version under KICAD_DOCUMENTS_HOME, or the platform directory used by KiCad: Documents/KiCad on Windows/macOS, user-data/kicad on Unix. The selected target must exist; unknown variables and targets absent from every searched source remain unresolved. **Known P1: the new default fallback can select a same-name library after an explicit exported/configured override target is missing; this differs from KiCad override semantics and blocks release.** KIPRJMOD remains bound to the table/project context rather than exported environment. No caller signature changes.

## Independent KiCad evidence

Installed `C:/Program Files/KiCad/10.0/bin/kicad-cli.exe version`: 10.0.6. Standalone bundled Python `pcbnew.Version()` also reports 10.0.6, with scratch KICAD_CONFIG_HOME and KICAD_DOCUMENTS_HOME.

Upstream source pinned to tag 10.0.6:

- [COMMON_SETTINGS::InitializeEnvironment](https://github.com/KiCad/kicad-source-mirror/blob/10.0.6/common/settings/common_settings.cpp#L768) initializes the versioned 3RD_PARTY variable even when not exported or configured.
- [PATHS::GetDefault3rdPartyPath](https://github.com/KiCad/kicad-source-mirror/blob/10.0.6/common/paths.cpp#L120) uses the user document path and appends 3rdparty; getUserDocumentPath checks KICAD_DOCUMENTS_HOME, then appends the platform KiCad directory and major/minor version.
- [Platform directory case](https://github.com/KiCad/kicad-source-mirror/blob/10.0.6/include/paths.h) defines KiCad for Windows/macOS and kicad for other platforms.
- [Unix base](https://github.com/KiCad/kicad-source-mirror/blob/10.0.6/libs/kiplatform/os/unix/environment.cpp#L87) uses user data, while [Windows](https://github.com/KiCad/kicad-source-mirror/blob/10.0.6/libs/kiplatform/os/windows/environment.cpp#L90) and [macOS](https://github.com/KiCad/kicad-source-mirror/blob/10.0.6/libs/kiplatform/os/apple/environment.mm#L75) use Documents.

No installed user configuration was needed or read for evidence. No personal paths are hard-coded in the implementation.

## Regression and shared refresh boundary

Before adding the fallback, a standalone Rust harness compiled the extracted resolver with no process/config variables, an explicitly injected temporary document root, and an existing `KiCad/10.0/3rdparty/footprints/Vendor Parts.pretty` directory. Its assertion failed with `left: None`, `right: Some(...Vendor Parts.pretty)`; the exact same harness passed after adding the fallback. This isolates the missing default boundary rather than adding an environment-key alias.

`pcb_footprint_update.rs::plan_updates` lines 636-647 calls the unchanged `library::resolve_footprint_path` and maps its Err to `footprint_library_resolution_failed`. That resolver reads and flattens the project/global table through expand_lib_uri, shared with served get_footprint_info. The resolver and served lookup boundary are tested here; the refresh diagnostic envelope and native replacement behavior remain owned by the refresh worker, with no fabricated envelope claim.

## Reproduction setup

Use the existing toolchain with PROTOC set and task-isolated CARGO_TARGET_DIR and KONNECT_STATE_DIR. This Windows run uses `C:/tmp/konnect-ailab49-target` because the longer worktree-local CMake/MSBuild scratch paths exceeded Windows limits; CARGO_BUILD_JOBS=2. No shared target/state or installed binaries are changed.

Observed commands and outcomes:

- Pre-fix standalone rustc harness: assertion failed, None instead of the existing injected third-party library path; after fallback the identical harness exited 0.
- `cargo test -p konnect-core --lib uri_resolver -- --nocapture`: **6 passed, 0 failed, 0 ignored**, 1724 filtered out. Tests cover default root with spaces, exported/configured/default precedence, missing/unresolved variables, version isolation, KIPRJMOD and stock footprint directory behavior, and Windows separators. Pure context injection means no process environment changes need restoration.
- `python -m py_compile tests/ailab49_library_resolution.py`: passed syntax compilation. The served harness itself is **not executed**.
- Owned-file rustfmt was applied after its first check reported formatting differences; final owned-file format check and git diff --check are checkpoint checks.
- `cargo build -p konnect --bin konnect`: started, then intentionally stopped on the coordinator's clean-interruption request; **no completed task-built server** and no served-lookup claim.

Initial build setup failures (not regression evidence): missing cmake on PATH, pip CMake wrapper import, long CMake/MSBuild scratch path; resolved using existing native CMake and short task-owned target. A function-item lifetime compilation error in the extracted wrapper was corrected with a closure before the six passing tests.

Independent review P1 received at interruption: KiCad chooses an explicit process/configured override independent of target existence. The current WIP instead falls through to the new default root when that override target is absent, potentially loading a different same-name footprint. Existing configured-vs-exported missing-target fallback predates this task, but the new third-party default introduces an additional unsafe source; fix the new fallback to apply only when no effective override exists and add a regression where the explicit override target is absent while the default target exists. The passing precedence test currently preserves legacy fall-through and does not establish this guard. Do not release this checkpoint.

Remaining acceptance/release blockers: address the P1 override guard and regression; finish the task-built server; execute the served harness for default/configured/exported roots and negative cases; perform native footprint loading comparison (loading an already-expanded path establishes payload compatibility, not native variable expansion); run one broader library regression group. Native Python import/version and source inspection are observed, but native URI expansion/live refresh/placement and the refreshed diagnostic envelope are not verified. No full workspace suite or cross-platform execution was performed.

## Limits and next owner

Windows execution is observed. Linux/macOS path logic is source-grounded but not executed on those platforms. No live editor, default API endpoint, native IPC, installed server replacement, or user design/configuration mutation is part of this verification. Coordinator owns integration and Quality Check; no merge, push, tag, release or deployment is authorized here.
