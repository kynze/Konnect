# Reusable symbol properties

The `library` toolset can author fields in reusable `.kicad_sym` definitions.
These fields belong to the library symbol; placed schematic annotations are a
separate operation.

```json
{
  "library_path": "./Example.kicad_sym",
  "name": "Example",
  "reference_prefix": "U",
  "pins": [{"number": "1", "name": "IN", "type": "input"}],
  "footprint": "Example:Package",
  "properties": [
    {"name": "MPN", "value": "EXAMPLE-001"},
    {"name": "Manufacturer", "value": "Example manufacturer"},
    {"name": "Rating", "value": "50 V", "visible": true,
     "x": 0, "y": -5.08, "angle": 0}
  ]
}
```

Pass that object to `create_symbol`. All property values are strings. New fields
default to hidden, at `(0, 0)`, with angle `0` and a 1.27 mm font. Coordinates
are symbol-local millimetres; angle is one of `0`, `90`, `180`, `270` degrees.
Names match exactly and are case-sensitive. Empty string values are meaningful.
New coordinates use the existing six-decimal millimetre formatter. Omitted
coordinates keep their original source representation without rounding.

`Reference`, `Value`, `Datasheet`, and `Footprint` have existing top-level
creation arguments (`reference_prefix`, `value`, `datasheet`, and `footprint`).
Use those arguments when creating rather than putting those names in
`properties`. `Description` and other custom fields can be supplied in the list.

To update a saved definition, call `edit_symbol_properties`:

```json
{
  "library_path": "./Example.kicad_sym",
  "symbol_name": "Example",
  "footprint": "Example:RevisedPackage",
  "properties": [{"name": "Rating", "value": "63 V", "visible": false}]
}
```

Each entry upserts a field. An existing field retains omitted visibility and
position attributes, font and other presentation settings. Unmentioned fields,
glyphs, pins, units, and other library entries retain their exact source bytes.
`footprint` is shorthand for updating the `Footprint` value while retaining its
presentation. An explicit empty footprint clears the default; omission preserves
it. Repeating an edit is a byte-identical no-op. Repeating creation of an existing
symbol is refused without writing.

Duplicate input names, conflicting Footprint inputs, unknown entry keys, nulls,
invalid types, unsupported angles, and malformed or ambiguous target definitions
are refused before writing. Editing inherited (`extends`) symbols is refused
because this tool does not resolve inherited field presentation. An empty patch
is refused; an explicit empty `properties` array is accepted as a no-op.

The editor uses source-span edits and an atomic compare-and-swap replacement.
Concurrent changes detected before replacement are conflicts. A failure during
or after replacement is conservatively reported as an uncertain mutation:
inspect the saved library before retrying. No result promises rollback after a
write attempt.
Readback must match the expected saved snapshot; a concurrent replacement before
that observation produces uncertainty instead of reporting another writer's
fields as a successful result.

Success returns `source: "saved_file"`, `success`, `changed`, `properties` (the
compatible name/value map), and `property_details` (value, visibility, x, y and
angle observed in the saved file). Creation keeps its existing unit/pin report.
`get_symbol_info` adds the same presentation details for locally defined fields;
its existing property map remains compatible. Inherited fields are not expanded
by that map or the new details.
