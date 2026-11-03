# AILAB-41 plan: typed pad mask / paste settings

Start SHA: 110edf57cc2f6a259035992ba9ca11be9ad7c98c (AILAB-49 resolver commits, AILAB-40 included).
Branch: jbp-kynze/ailab-41-mask-paste (worktree ailab-41-mask-paste).

## Contract

Per-pad fields on `create_footprint.pads[]` and `edit_footprint_pad`:

| field | unit | KiCad token |
|---|---|---|
| `solder_mask_margin` | mm | `(solder_mask_margin v)` |
| `solder_paste_margin` | mm (absolute) | `(solder_paste_margin v)` |
| `solder_paste_margin_ratio` | ratio (-0.1 = -10 %) | `(solder_paste_margin_ratio v)` |

Semantics
- create: omitted or `null` -> token absent (inherits).
- edit: omitted -> existing token preserved byte-for-byte; non-zero number
  -> local override written; `null` -> token removed (inheritance restored).
- **Zero is refused** (decision 2026-11-03): KiCad 10.0.6 reads a pad-local `0`
  as unset and drops it on save (observed natively), so it cannot be stored.
  An existing zero token reads back as inherited with `file_token_zero: true`.
  See docs/AILAB-41-VERIFICATION.md.
- Margins are converted to integer nanometres with rounding, then written from
  the integer (0.075 -> `0.075`, never `0.074999`). Ratio is rounded to 1e-6.
- Refused before any write: non-number, NaN/inf, |margin| > 10 mm, ratio outside
  [-1, 1], unknown pad.  Refusal leaves the file byte-identical.
- Token position follows KiCad's writer: after net/pinfunction/pintype/die_length,
  before clearance / zone_connect / thermal_* / options / primitives / uuid.
  Existing duplicates are collapsed to one.

Readback (`get_footprint_info include_pads=true`), per pad `mask_paste`:
`{ solder_mask_margin|solder_paste_margin|solder_paste_margin_ratio:
   { local, source: "pad"|"footprint"|"unknown", effective } }`
- `local` = pad token or null.
- `effective` = pad token, else footprint-level token, else **null** with
  source `unknown` (board/project defaults are not knowable from a .kicad_mod).
- A token whose value is unreadable makes the call fail, as an unreadable pad does.

## Ownership
Only library.rs authoring/readback sections plus new modules
`library_pad_mask_paste.rs` and `library_pad_mask_paste_tests.rs` (declared in library.rs).
No change to pcb_components.rs: readback is attached in library.rs by pad index.

## Order
1. Tests first (served schema, dispatch, persistence, readback, refusal, idempotence) - observe red.
2. Module + wiring - green.
3. Native KiCad 10.0.6 round-trip script (tests/ailab41_native_roundtrip.py).
4. One broader `tools::library::` run with isolated TEMP.
5. docs/AILAB-41-VERIFICATION.md, commit.
