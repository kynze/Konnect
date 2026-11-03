# AILAB-42 verification

Status: implemented and verified locally; not merged, pushed, tagged or released. Installed Konnect 0.13.0 / 6bbe3e4 untouched.

Start SHA: c85390a (AILAB-41) atop 110edf5 (AILAB-49). Branch `jbp-kynze/ailab-42-padstack`.

## Native format (KiCad 10.0.6, observed by building pads with pcbnew in a scratch config)

The pad header carries the front layer (shape, size, roundrect ratio). The rest of the stack is
`(padstack (mode front_inner_back) (layer "Inner" ...) (layer "B.Cu" ...))`. `(layers "*.Cu" "*.Mask")` is unchanged, so a reader that copies the header onto `*.Cu` says every copper layer looks like the front one. That is what `get_footprint_info` did before (the file reader copies the header geometry onto each `*.Cu` entry). `mode custom` writes every inner layer up to the board maximum (In1..In30) plus per-layer `thermal_bridge_angle` / `zone_connect`, so it depends on the board's layer count. Two native-generated, synthetic fixtures are in `crates/konnect-core/tests/fixtures/ailab42_padstack_*_native.kicad_mod`.

## Behaviour

`create_footprint` pads and `edit_footprint_pad` accept `padstack`: `{mode: "front_inner_back", front, inner, back}`; each layer is `{shape: circle|rect|oval|roundrect, width, height, roundrect_rratio?}` (mm; ratio only with roundrect, 0..0.5; size > 0 and <= 100 mm). All three layers are required: nothing is defaulted.

- The pad header is the front layer. On create, a supplied shape/width/height/roundrect_rratio must equal `padstack.front`. On edit the header is rewritten from `front`.
- create: omitted/null = ordinary pad. edit: omitted = unchanged; null = back to an ordinary pad (only for a stack Konnect can read completely); object = set (an existing block is replaced in place, so a repeat edit is byte-identical).
- Mask/paste (AILAB-41), drill, position, other pads and artwork are preserved (tested: bytes after pad 1 identical; set then null restores the original file byte-for-byte).
- Refused before any write, file byte-identical: non-thru_hole pad or no `*.Cu` layer; unsupported mode (`custom`, `normal`), shape, key, or missing layer; non-positive/oversized/non-numeric size; ratio on a non-roundrect or out of range; header/front mismatch; a pad whose existing stack is `custom`, has unexpected layers, or carries tokens Konnect does not model (e.g. `offset`, `zone_connect`); a pad with non-standard header geometry (chamfer, primitives, ...); and a shape/width/height edit on a padstack pad without `padstack` (it would leave header and stack describing different front layers; this guard also protects pads created elsewhere).
- Edited blocks are re-parsed and checked against the request before the write.

Readback (`get_footprint_info include_pads=true`), per pad: `padstack` = `{mode: "normal"}` | `{mode: "front_inner_back", front, inner, back}` | `{mode: "custom", layers: [...]}`; each layer has `layer`, `shape`, `size{x,y}`, `corner_rounding_ratio?` and `unmodelled[]` for layer tokens Konnect does not interpret. For non-normal stacks `copper_layers` is replaced with the true per-layer list (F.Cu from the header, then Inner/B.Cu or In*.Cu), never `*.Cu` copies. An unknown mode, missing layer, unknown shape or unreadable size fails the call. Ordinary pads are unchanged.

## Evidence

Red first (served dispatch): all 11 new tests failed before the product change.

Green: `cargo test -p konnect-core --lib padstack_tests` 11 passed / 0 failed (served schema, create with the fixture stack and per-layer readback, roundrect ratios per layer, edit set / idempotent repeat / replace in place with everything else preserved, null round trip byte-exact, 13-case refusal table plus header edits, SMD and layer-less refusal, native front_inner_back fixture read and unrelated edit leaving the stack bytes alone, native custom fixture read per layer and rewrite refused, unmodelled-token refusal, unreadable/unknown stack fails readback).

Broader check, once: `cargo test -p konnect-core --lib tools::library:: -- --test-threads=1` (TEMP under `C:\k49w\iso42`): **200 passed, 0 failed** (189 from AILAB-41 + 11 new).

Native KiCad 10.0.6: `python tests/ailab42_native_roundtrip.py <task-built konnect.exe> "C:/Program Files/KiCad/10.0/bin/python.exe"` -> 5 PASS lines:
1. a footprint authored by the served tool (rect front/back, circle inner, 0.8 mm drill, mask margin 0.075) loads, saves and reloads natively with those per-layer shapes/sizes (1.6/1.2/1.6 mm), drill and mask unchanged;
2. served readback of KiCad's own saved file reports per-layer copper, not `*.Cu`;
3. a served edit to a roundrect (0.2/0.25) / oval-inner stack is read layer by layer by KiCad, other pad untouched;
4. null returns an ordinary pad natively; a header edit on a stack pad is refused with bytes unchanged;
5. a custom stack built by pcbnew is read per layer and its rewrite is refused with bytes unchanged.

Scratch config/documents roots, inproc endpoint, scratch library; no IPC, editor, default socket, installed config or design repository was touched. rustfmt (new files and edited hunks), `git diff --check` and `py_compile` clean.

## Limits
- KiCad 10.0.6 on Windows only. The zero-handling caveat from AILAB-41 is unrelated, but the same lesson applied: every claim was checked natively.
- Only `front_inner_back` is authored. Per-layer offset, chamfer, thermal/zone settings, custom pad primitives and `custom` mode are read (as far as listed) and refused for rewrite, not authored.
- A `custom` stack's readback lists every layer KiCad wrote (up to 30 inner layers); `front` there is the pad header.
- Plain edits (position, drill, mask/paste, zone_connect, renumber) work on padstack pads; only header-geometry edits are refused.
- `get_component_pads` (board file / IPC) still builds `copper_layers` in `pcb_components.rs` / konnect-ipc by copying header geometry; that code is outside this task's ownership and was not changed.
- `crates/konnect/assets/skills/kicad-library/SKILL.md` does not yet mention `padstack` or the AILAB-41 fields (unowned; follow-up).
- Debug build only: `C:/tmp/konnect-ailab42-target/debug/konnect.exe`.

## Independent review follow-up (2026-11-03, reviewer read c25a404; no data-loss or write-before-check bug)

This branch also carries the AILAB-41 review follow-up (cherry-picked as 0c45841; see docs/AILAB-41-VERIFICATION.md).

Fixed after review:
- Multi-line pads got an over-indented padstack block (layer children five tabs deep instead of four). `render` now passes the padstack's own indent. Negative control: with the old indentation re-introduced, 3 new tests fail; with the fix restored all pass.
- `layout_of` now takes the indent from the last child that starts on its own line, not from an inline-appended child such as ` (drill 0.9)`.
- Readback now reports every unmodelled header token (`chamfer_ratio`, `rect_delta`, `primitives`, `options`, `offset`, `chamfer`) in `front.unmodelled`, not just `offset`.
- Tests added: re-applying the same stack to a KiCad-native pad is byte-identical in LF, CRLF and space-indented forms; a changed layer is written with the file's own indentation (exact expected text); a new stack on a CRLF multi-line pad has the exact expected block and removing it restores the original; `match_all` + `new_number` + `zone_connect` + `drill` + `padstack` on duplicate pad numbers, and first-match-only without `match_all`; header-token readback and refusal.
- Native script phase 6: a served edit of a pad KiCad itself saved leaves the file byte-identical for an identical stack, and KiCad reads back a changed inner layer.

Counts: `padstack_tests` 16 passed; library group once, isolated TEMP: **210 passed, 0 failed**; native script 6 PASS lines.

Kept as designed (reviewer's finding 3): one pad with an unknown mode, a missing layer or an unreadable size fails the whole `get_footprint_info include_pads` call rather than being skipped, so nothing is silently dropped. A roundrect layer with no ratio token reports no `corner_rounding_ratio` (KiCad applies its own default).
