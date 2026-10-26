# MEZZA-197 verification

2026-10-26 · Windows · Rust 1.96.0 · KiCad 10.0.6

## Result

The local MCP server supports explicit sheet-instance ordering and atomic
instance-specific reference migration. Disposable mezzanine copies passed the
checks below. Production schematic, project and PCB hashes stayed unchanged.
This is tooling acceptance; production migration remains MEZZA-193.

Branch: `fix/MEZZA-197-repeated-sheet-pages`, based on upstream `c7e87cd`.
Initial reused-file/readback repair: `cf778cd`. The completion commit adds the
options described in [the API guide](SCHEMATIC_INSTANCE_MIGRATION.md).

## Automated checks

- Observed new migration tests failing before implementation.
- Observed the independent-review component-merging regression failing before
  its fix. Final reference groups retain original component identity, and
  duplicate saved sheet records are rejected during planning.
- `cargo test -p konnect-core --lib`: **1683 passed, 0 failed, 21 ignored**.
- `cargo clippy -p konnect-core --lib --tests -- -D warnings`: passed.
- `cargo fmt --all`, `git diff --check`, `cargo build -p konnect`: passed.
- Independent review found two issues; both were fixed. Follow-up review
  confirmed resolution with no further concrete findings.

## Copy trial

Fresh copies excluded copied editor locks. A separate debug server ran through
MCP stdio with an explicit local config. Every design mutation used its MCP tool.

The accepted order is root 1, high-speed 2, cluster a 3 with connectors 1–4 on
pages 4–7, cluster b 8 with connectors 1–4 on pages 9–12.

The map changed **264 references**: 156 physical components and 108 internal
power symbols. `LOGO1` was preserved. The inventory covered 313 current-project
symbol-unit records plus 12 foreign-project records.

1. Dry-run left all copied design hashes unchanged.
2. Apply changed three physical schematic files and returned saved-file readback.
3. Every saved page/reference record and rendered Reference cache matched the
   map. All unselected and foreign records were preserved.
4. Non-annotation semantic data stayed unchanged. Comparison accounted only
   for equivalent serializer forms: unordered symbol/sheet/pin field order and
   `(fields_autoplaced yes)` versus `(fields_autoplaced)`. Polygon point order
   and arbitrary nested content were not normalized away. An initial strict
   comparison detected these differences; their cause was established before
   acceptance.
5. Independent KiCad XML exports retained all **171 physical component identities
   and attributes** and **556 UUID/pin net groups**. Explicitly named nets retained
   their names. KiCad-generated names were checked by pin membership because
   they can incorporate renamed designators.
6. Native KiCad loaded the saved migration for fresh XML and PDF exports. The
   raw PDF title blocks were **1 through 12 in order**, without post-processing.
7. A page-only repeat call was byte-identical. Reference maps use old-reference
   preconditions and are intentionally not blindly replayed.
8. Production protected-file hashes before and after the trial were identical.

## Local handoff

Checkout: `C:/Users/jbperrier_kynze/workspace/tools/Konnect-mezza197`.
Runnable server: `target/debug/konnect.exe --config .local/mezza197-config.toml`.
Use MCP stdio `initialize`, then `tools/call`. The installed server is unchanged;
subsequent source mutations must use this tested server or a build containing
the completion commit.

Ignored local evidence:

- `.local/mezza197-final-trial.py`: trial driver and independent comparison.
- `.local/mezza197-final-copy-trial.json`: plans, readback, hierarchy and summary.
- `.local/mezza197-final-core.log`, `.local/mezza197-clippy.log`,
  `.local/mezza197-final-build.log`, `.local/mezza197-migration-red.log`,
  `.local/mezza197-review-red.log`: test/build evidence.
- `%TEMP%/mezza197-accepted-migration/`: explicit `migration-args.json`,
  `before.xml`, `after.xml`, `native-after.pdf`, and migrated hardware copy.

For MEZZA-193, make a fresh copy and derive arguments against current saved
source. Repeat dry-run/apply/readback/export checks. Do not replay the temporary
arguments against a different root or already-migrated design.

## Limits

No production migration, PCB synchronization, post-migration ERC/DRC or fabrication
build was performed. No interactive GUI save was exercised; native KiCad loaded
the saved files through its XML/PDF exporters. No installed binary/config
replacement, push, tag, upstream issue/PR or manufacturing approval occurred.
