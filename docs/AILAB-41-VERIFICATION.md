# AILAB-41 verification

Status: implemented and verified locally; not merged, pushed, tagged or released. Installed Konnect 0.13.0 / 6bbe3e4 untouched.

Start SHA: 110edf57cc2f6a259035992ba9ca11be9ad7c98c (AILAB-49 resolver commits atop AILAB-40). Branch `jbp-kynze/ailab-41-mask-paste`.

## Behaviour

`create_footprint` pads and `edit_footprint_pad` accept `solder_mask_margin`, `solder_paste_margin` (mm) and `solder_paste_margin_ratio`.

| request | create | edit |
|---|---|---|
| omitted | no token (inherits) | existing token preserved byte-for-byte |
| `null` | no token | token removed (inheritance restored) |
| non-zero number | token written | token set/replaced; duplicates collapsed |
| `0` or a value rounding to 0 | **refused** | **refused** |
| non-number, non-finite, out of range | refused | refused |

Values go through integer nanometres (ratio: millionths) with rounding and are written from the integer: +0.075 mm is `0.075`, never `0.074999`. Limits are a Konnect guard, not a KiCad one: margins |v| <= 10 mm, ratio in [-1, 1]. Refusals happen before any write and leave the file byte-identical. Edited pad blocks are re-parsed and checked against the request before the write.

Token placement follows KiCad's writer (mask, paste margin, paste ratio, immediately before `clearance` / `zone_connect` / `thermal_*` / `options` / `primitives` / `uuid`, else after the last child) and reuses the file's own separators, so set-then-clear restores the exact original bytes (tested on one-line pads and on a CRLF KiCad 10 fixture).

Readback: `get_footprint_info include_pads=true` adds `mask_paste` per pad: for each setting `local` (pad token or null), `source` (`pad` | `footprint` | `unknown`) and `effective`. Effective is the pad value, else the footprint-level token, else **null/unknown**: board and project defaults are not in a `.kicad_mod`, so none is invented. An unreadable token fails the call rather than being dropped. `edit_footprint_pad` responses include the pad-local values read from the written pads.

## Deviation from the original brief: explicit zero

The brief asked for explicit zero to be a local override. KiCad 10.0.6 cannot store one. Observed natively (`pcbnew` 10.0.6, scratch config): a pad with `(solder_mask_margin 0) (solder_paste_margin 0) (solder_paste_margin_ratio 0)` loads with all three unset, and native save writes none of the tokens. Non-zero values round-trip exactly. The cause was not confirmed in KiCad source.

Decision (user, 2026-11-03, "option 1"): refuse zero with an error naming KiCad 10.0.6 and pointing to `null`. An existing zero token in a file is read back as inherited with `file_token_zero: true` (pad) or `footprint_token_zero: true`, matching KiCad. "Paste margin 0 and ratio 0" acceptance is therefore met as refusal plus native proof of the drop, not as a stored override. If a later KiCad version stores zero, this is the one place to revisit.

## Evidence

Red before the product change (served dispatch): all 7 initial tests failed, e.g. `Additional properties are not allowed ('solder_mask_margin' was unexpected)` from `create_footprint`, and the missing `mask_paste` readback.

Green after: `cargo test -p konnect-core --lib pad_mask_paste_tests` 10 passed / 0 failed (served schema, create exact values and inheritance, edit preserve/change/clear/add, idempotent repeat, byte-exact set-then-clear, malformed/out-of-range refusals with byte-identical file, zero refusal for all three fields on create and edit, KiCad-style token placement on a CRLF fixture, footprint-level effective source, unreadable-token failure, existing zero-token readback).

Broader check, once: `cargo test -p konnect-core --lib tools::library:: -- --test-threads=1` with TEMP/TMP under `C:\k49w\iso41`: **189 passed, 0 failed** (179 pre-existing + 10 new).

Native KiCad 10.0.6: `python tests/ailab41_native_roundtrip.py <task-built konnect.exe> "C:/Program Files/KiCad/10.0/bin/python.exe"` -> 4 PASS lines:
1. footprint authored by the served tool loads, saves and reloads natively with mask 75000 / 125000 nm, paste margin -50000 / 25000 nm, ratio -0.1 / 0.05, and the untouched pad unset;
2. served edits (change in place, clear, add, omit) read identically natively; unrelated pad size/position unchanged;
3. served readback of the KiCad-saved file matches the native values;
4. native drops pad-local 0 on load/save, and served edit refuses 0.

The script uses scratch config/documents roots, an inproc endpoint and a scratch library; no IPC, editor, default socket, installed config or design repository was touched. The only hand-written file is a zero-token scratch fixture in phase 4, created to observe KiCad's behaviour.

Also run: rustfmt --check on both new files (clean) and on library.rs for the edited hunks, `git diff --check`, `py_compile`.

## Limits
- Only KiCad 10.0.6 on Windows; behaviour of other KiCad versions (including zero handling) is unverified.
- Board/project-level mask and paste defaults are never resolved; `effective` is null when neither pad nor footprint sets a value.
- Footprint-level settings are read, not authored.
- `crates/konnect/assets/skills/kicad-library/SKILL.md` does not yet mention the new fields (unowned shared file; follow-up).
- The ranges above are Konnect guards, not KiCad limits.
- Task binary: `C:/tmp/konnect-ailab41-target/debug/konnect.exe` (debug, not a release build).
