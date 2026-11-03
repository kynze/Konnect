# AILAB-42 plan: faithful per-layer copper padstack

Start SHA: c85390a (AILAB-41) atop 110edf5 (AILAB-49). Branch jbp-kynze/ailab-42-padstack.

## Native format (KiCad 10.0.6, observed by building pads with pcbnew in a scratch config)

Header `(pad N thru_hole <front shape> (size front) (roundrect_rratio front?) ...)` then
`(padstack (mode front_inner_back) (layer "Inner" (shape ..) (size ..) [(roundrect_rratio ..)]) (layer "B.Cu" ...))`.
`mode custom` lists every inner layer up to the board maximum (In1..In30) plus B.Cu, with
per-layer `thermal_bridge_angle` / `zone_connect`: it depends on the board's layer count.
`(layers "*.Cu" "*.Mask")` stays as written, which is why a reader that copies the header
geometry onto `*.Cu` misdescribes the pad. Fixtures: tests/fixtures/ailab42_padstack_*_native.kicad_mod
(generated natively, synthetic).

## Contract
Authoring (create_footprint pads[].padstack, edit_footprint_pad.padstack):
`{ "mode": "front_inner_back", "front": L, "inner": L, "back": L }`, L = {shape circle|rect|oval|roundrect,
width, height, roundrect_rratio (roundrect only, 0..0.5)}. All three layers required (switching mode resets defaults).
- create: omitted/null = normal pad. edit: omitted = unchanged; null = back to normal (only for stacks Konnect can read completely); object = set.
- Header shape/size/ratio are the front layer. Supplied pad shape/width/height must equal padstack.front.
- Refused before any write (file byte-identical): non-thru_hole pad or no *.Cu layer; unsupported shape/mode; missing layer;
  non-positive or > 100 mm size; ratio out of range; existing custom-mode stack; existing stack with tokens not modelled
  (offset, chamfer, zone_connect, ...); header/front mismatch; shape/width/height edit on a padstack pad without `padstack`
  (would desync the header from the stack).
- Mask/paste (AILAB-41), drill, position, unrelated pads and artwork are preserved.

Readback (get_footprint_info include_pads): per pad `padstack` = {mode normal} | {mode front_inner_back, front, inner, back}
| {mode custom, layers[]}, each layer {layer, shape, size{x,y}, corner_rounding_ratio?, unmodelled[]?}. For non-normal stacks
`copper_layers` is replaced by the true per-layer list (F.Cu from header, Inner/B.Cu or In*.Cu from the stack). Unknown mode
or unreadable layer fails the call.

## Order
1 tests first (served dispatch), observe red; 2 module library_padstack.rs; 3 native round-trip script;
4 one broader library run; 5 docs + commit; 6 Plane comment + state.
