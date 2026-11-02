# AILAB-49 third-party URI resolution

Scope: preserve library caller signatures and existing exported/configured/install precedence. Add KiCad default third-party root after exported and configured variables, with injected context for deterministic tests. Do not change authoring APIs or library tables by text.

Evidence: KiCad 10.0.6 CLI version observed. Upstream tag 10.0.6 common/settings/common_settings.cpp InitializeEnvironment initializes versioned 3RD_PARTY using PATHS::GetDefault3rdPartyPath; common/paths.cpp derives it from KICAD_DOCUMENTS_HOME or platform documents, then KiCad/<major>.0/3rdparty.

Sequence: extract URI resolver with context injection; reproduce default-root failure before adding fallback; test root precedence, absent targets, unknown variables, KIPRJMOD, spaces and Windows separators. Exercise built server stdio with IPC isolated and builder-created sanitized library; compare native loading if safely available. Run focused tests and one broader relevant check, review owned diff and commit locally. No live editor or installed configuration changes.

## Resumed dispatch

Reproduce reviewed P1 with an explicit missing override and an existing default same-name library. For standard third-party roots, select the effective exported/configured/default source before checking target existence; keep legacy resolution for other variable kinds. Complete focused tests, reuse the isolated build for served default/configured/exported lookups and refusals, compare native payload loading, then run one broader library test group and commit. No native IPC or editor is needed.
