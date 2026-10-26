# Schematic instance page and reference migration

`renumber_sheet_pages` reads saved schematic files; it does not edit an open
Eeschema document. Close documents before applying. Use a disposable copy for
the initial trial. No PCB synchronization is performed by this tool.

Existing calls without the new options retain depth-first numbering. A reused
physical sheet is updated once per transaction while each instance retains its
own page and component references.

## Inputs

- `schematic`: saved root `.kicad_sch` path.
- `project_name`: optional project instance key, defaulting to the root stem.
- `sheet_order`: optional array of **all non-root full UUID instance paths**,
  ordered from page 2 onward. Include the root schematic UUID at the beginning
  of each path. The root page is unchanged. Duplicate, missing or unknown paths
  are refused; paths are identities, not displayed sheet names or filenames.
- `reference_changes`: optional array of objects with exactly
  `sheet_instance_path`, `old_reference`, and `new_reference`. The path identifies
  one instantiated sheet, including the root UUID. Every placed unit of that
  physical component is changed together. Unselected instances and foreign
  project records are preserved. Designators must be annotated, such as `R301`,
  `U601`, or `#PWR0301`; question marks are refused.
- `dry_run`: optional boolean, default `false`. A present malformed value or
  `null` is refused for each of these options.

For example (substitute actual UUID paths):

```json
{
  "schematic": "C:/trial/hardware/root.kicad_sch",
  "sheet_order": ["/ROOT_UUID/B_UUID", "/ROOT_UUID/A_UUID"],
  "reference_changes": [
    {"sheet_instance_path":"/ROOT_UUID/A_UUID", "old_reference":"R201", "new_reference":"R301"},
    {"sheet_instance_path":"/ROOT_UUID/B_UUID", "old_reference":"R301", "new_reference":"R201"}
  ],
  "dry_run": true
}
```

Use valid UUIDs rather than the placeholders above. Change `dry_run` to `false`
only after inspecting the proposed mapping. The map is simultaneous: reference
swaps do not require intermediate temporary names. Duplicate requests, unknown
old references, inconsistent saved units and collisions with untouched components
are rejected before the transaction. Missing/cyclic children are refused in an
explicit migration. Multi-unit identity requires the same sheet instance,
library identifier and value, with distinct unit numbers.

The rendered `Reference` property is updated when it represents a renamed saved
instance. Other instance records remain independent; a shared sheet cannot have
one rendered property representing all of its distinct instances at once.

## Outputs and verification

`pages` lists each full instance path, parent document, child document, sheet UUID
and page. `references` lists changed symbol units with symbol UUID, instance path,
old/new reference and unit. `changed_files` counts physical files that differ.
`renumbered_count` remains the number of traversed non-root sheets for backwards
compatibility, including unchanged sheets. `source` distinguishes
`validated_plan` from `saved_file_readback`.

Apply verifies the committed sheet and symbol instance records against the plan.
If post-commit readback fails, the error explicitly states that the migration
was committed: inspect the files before retrying. A stale reference map is not
silently replayed. Pure page renumbering is idempotent.

Independently export a KiCad XML netlist and compare connectivity using saved
symbol identities and pin numbers after translating references. Include hidden
power symbols in the annotation audit; physical BOM/netlist components alone
do not cover those designators. PCB reference synchronization, silkscreen review,
ERC/DRC and downstream documentation are separate steps.
