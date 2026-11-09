# AILAB-43 release plan (draft, 2026-11-09)

Status: integrated and tested locally and pushed as a branch. **Not merged to main, not versioned, not tagged, not built for release, installed Konnect untouched** (0.13.0 / 6bbe3e4 at `%LOCALAPPDATA%\Programs\Konnect\v0.13.0\konnect.exe`). Every step marked APPROVAL below waits for an explicit go-ahead.

## What is in the branch

Branch `jbp-kynze/ailab-43-library-release` (origin: kynze/Konnect), tip `65fd8c8`, 19 commits over `main` (c7e87cd), 47 files, +10078/-132. Linear history:

| Commits | Work |
|---|---|
| cf778cd .. 34ad114 (6) | MEZZA-194/196/197 migration tools that the library work was built on |
| d38a6ed | AILAB-40 reusable symbol properties and default footprints |
| e07ae83, 110edf5, f6303e8 | AILAB-49 third-party library resolution (e07ae83 is a WIP checkpoint commit) |
| 40671e8, 2bc08ba | AILAB-41 pad mask/paste margins (zero refused: KiCad 10.0.6 treats a pad-local 0 as unset) |
| 182025a, 7f33140 | AILAB-42 per-layer copper padstacks (`front_inner_back` authoring; custom stacks read-only) |
| 2d35f76, 2811bc1, 4b814d5, b0f69b4 | AILAB-48 library refresh: pad-local settings preserved, verified after the commit, rollback by a restoring commit |
| 65fd8c8 | schema-usage guard fix for the AILAB-41 parameters (found by the first full-workspace run) |

Deferred and not in the branch: AILAB-48 guarded footprint swaps and bulk models (`docs/AILAB-48-DEFERRED.patch`), `kicad-library` SKILL.md text for the AILAB-41/42 fields, `get_component_pads` header-geometry copy onto `*.Cu`.

Per-task evidence: `docs/AILAB-41-VERIFICATION.md`, `AILAB-42-VERIFICATION.md`, `AILAB-48-VERIFICATION.md`, `AILAB-49-VERIFICATION.md`.

## Checks done on this tip

- `cargo test --workspace --no-fail-fast` (debug, TEMP outside `C:\tmp`): 41 suites, 2449 passed, 0 failed, 47 ignored (the ignored ones need a live KiCad GUI).
- Native KiCad 10.0.6 scripts ran per task before integration (41: 5 phases, 42: 6 phases, 48: apply + stale + no-op, 49: 7 modes). They have **not** been re-run on this exact integrated tip.
- Not verified natively: the 72-footprint and 57-model bulk scenarios, back-side transforms, spoke width.

## Steps before a release (all APPROVAL unless noted)

1. (no approval) Re-run the native scripts on the integrated tip with the hidden-desktop editor; record results here.
2. APPROVAL: merge to `main` (or open a PR from the pushed branch; a PR is the lighter option).
3. APPROVAL: version bump (workspace is `0.13.0`; the installed build is also `0.13.0`, so a new build must carry a new version, e.g. `0.13.1`) and tag. `release.yml` exists; it must not be triggered until the tag is approved. Read it first to know what a tag publishes.
4. Release build from the tagged commit: `cargo build --release -p konnect`, record commit SHA, `rustc --version`, build date, SHA-256 of `konnect.exe`, and `konnect --version` output as provenance.
5. APPROVAL: install side by side as `%LOCALAPPDATA%\Programs\Konnect\v<new>\konnect.exe`; do not overwrite `v0.13.0`.
6. APPROVAL: point the MCP client configuration at the new path.

## Rollback and restart

- Rollback is a path switch: `v0.13.0\konnect.exe` stays in place and is the known-good build; point the MCP client configuration back and restart the client.
- Restart: running `konnect.exe` processes belong to MCP clients (about 16 are running now, several days old) and keep the old binary until their client restarts, so adoption and rollback both need a client restart. Which client configurations name the path was not inspected yet and must be checked before step 6.
- Source rollback: backup tags `backup/pre-restack-ailab-49|41|42` hold the pre-integration tips; the branch is unpushed-to-main, so reverting is dropping the branch.
- Data: the new tools write library files and live-board edits only on request; the AILAB-48 apply is one KiCad commit that can be undone with Ctrl-Z, plus a restoring commit if its own check fails.
