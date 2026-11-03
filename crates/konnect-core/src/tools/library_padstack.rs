//! AILAB-42: faithful per-layer copper padstack authoring and readback.
//!
//! KiCad 10.0.6 stores a stack as the pad header (the front layer: shape,
//! size, roundrect ratio) plus `(padstack (mode front_inner_back) (layer
//! "Inner" …) (layer "B.Cu" …))`. `(layers "*.Cu" …)` is unchanged, so a
//! reader that copies the header onto `*.Cu` misdescribes the pad. Only
//! `front_inner_back` is authored; `custom` stacks depend on the board's
//! copper layer count and are read but never rewritten.
use super::pad_mask_paste::format_micro;
use super::*;

const SHAPES: [&str; 4] = ["circle", "rect", "oval", "roundrect"];
/// Shapes the readback will name; anything else fails the read.
const READABLE_SHAPES: [&str; 7] = [
    "circle",
    "rect",
    "oval",
    "roundrect",
    "trapezoid",
    "chamfered_rect",
    "custom",
];
const MAX_SIZE_MM: f64 = 100.0;
const MODE: &str = "front_inner_back";
const LAYER_KEYS: [&str; 4] = ["shape", "width", "height", "roundrect_rratio"];
/// Layer-block children Konnect models; any other child is "unmodelled".
const MODELLED: [&str; 3] = ["shape", "size", "roundrect_rratio"];
/// Pad header children that make the header geometry non-standard.
const HEADER_UNSUPPORTED: [&str; 6] = [
    "chamfer_ratio",
    "chamfer",
    "rect_delta",
    "options",
    "primitives",
    "offset",
];
/// Children KiCad writes after the header ratio.
const AFTER_HEADER_RATIO: [&str; 15] = [
    "chamfer_ratio",
    "chamfer",
    "net",
    "solder_mask_margin",
    "solder_paste_margin",
    "solder_paste_margin_ratio",
    "clearance",
    "zone_connect",
    "thermal_bridge_width",
    "thermal_bridge_angle",
    "thermal_gap",
    "options",
    "primitives",
    "padstack",
    "uuid",
];

#[derive(Clone, PartialEq, Debug)]
pub(super) struct LayerSpec {
    shape: String,
    width: String,
    height: String,
    ratio: Option<String>,
}

#[derive(Clone, PartialEq, Debug)]
pub(super) struct Stack {
    front: LayerSpec,
    inner: LayerSpec,
    back: LayerSpec,
}

#[derive(Clone, PartialEq, Debug)]
pub(super) enum Change {
    Keep,
    Clear,
    Set(Stack),
}

type ArgError = (String, String);

fn micro(value: f64) -> i64 {
    (value * 1e6).round() as i64
}

fn parse_layer(value: &serde_json::Value, path: &str) -> Result<LayerSpec, ArgError> {
    let object = value
        .as_object()
        .ok_or_else(|| (path.to_string(), "must be an object".to_string()))?;
    for key in object.keys() {
        if !LAYER_KEYS.contains(&key.as_str()) {
            return Err((
                format!("{path}.{key}"),
                format!(
                    "unsupported key '{key}'; supported: shape, width, height, roundrect_rratio"
                ),
            ));
        }
    }
    let shape = match object.get("shape").and_then(|v| v.as_str()) {
        Some(shape) if SHAPES.contains(&shape) => shape.to_string(),
        Some(shape) => {
            return Err((
                format!("{path}.shape"),
                format!("unsupported shape '{shape}'; expected circle, rect, oval or roundrect"),
            ))
        }
        None => return Err((format!("{path}.shape"), "missing or not a string".into())),
    };
    let mut size = |key: &str| -> Result<String, ArgError> {
        let field = format!("{path}.{key}");
        let number = object
            .get(key)
            .and_then(|v| v.as_f64())
            .ok_or_else(|| (field.clone(), "missing or not a number".to_string()))?;
        if !number.is_finite() || micro(number) <= 0 || number > MAX_SIZE_MM {
            return Err((
                field,
                format!("must be > 0 and <= {MAX_SIZE_MM} mm; got {number}"),
            ));
        }
        Ok(format_micro(micro(number)))
    };
    let width = size("width")?;
    let height = size("height")?;
    let ratio = match object.get("roundrect_rratio") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => {
            let field = format!("{path}.roundrect_rratio");
            if shape != "roundrect" {
                return Err((field, "only valid when shape is roundrect".into()));
            }
            let ratio = value
                .as_f64()
                .ok_or_else(|| (field.clone(), "must be a number".to_string()))?;
            if !(0.0..=0.5).contains(&ratio) {
                return Err((field, format!("must be between 0 and 0.5; got {ratio}")));
            }
            Some(format_micro(micro(ratio)))
        }
    };
    Ok(LayerSpec {
        shape,
        width,
        height,
        ratio,
    })
}

/// `value` is the `padstack` argument. `null` clears when `allow_clear`
/// (edit) and means "no stack" otherwise (create).
pub(super) fn parse(
    value: &serde_json::Value,
    prefix: &str,
    allow_clear: bool,
) -> Result<Change, ArgError> {
    if value.is_null() {
        return Ok(if allow_clear {
            Change::Clear
        } else {
            Change::Keep
        });
    }
    let object = value.as_object().ok_or_else(|| {
        (
            prefix.to_string(),
            "must be an object (or null)".to_string(),
        )
    })?;
    for key in object.keys() {
        if !["mode", "front", "inner", "back"].contains(&key.as_str()) {
            return Err((
                prefix.to_string(),
                format!("unsupported key '{key}'; supported: mode, front, inner, back"),
            ));
        }
    }
    match object.get("mode").and_then(|v| v.as_str()) {
        Some(MODE) => {}
        Some(other) => {
            return Err((
                format!("{prefix}.mode"),
                format!("unsupported mode '{other}'; only {MODE} can be authored (custom stacks depend on the board's copper layer count)"),
            ))
        }
        None => return Err((format!("{prefix}.mode"), format!("required; expected {MODE}"))),
    }
    let layer = |side: &str| -> Result<LayerSpec, ArgError> {
        let path = format!("{prefix}.{side}");
        match object.get(side) {
            None | Some(serde_json::Value::Null) => Err((
                path,
                "required: populate front, inner and back explicitly".to_string(),
            )),
            Some(value) => parse_layer(value, &path),
        }
    };
    Ok(Change::Set(Stack {
        front: layer("front")?,
        inner: layer("inner")?,
        back: layer("back")?,
    }))
}

/// Supplied pad header values must describe the front layer.
pub(super) fn check_header(
    stack: &Stack,
    shape: Option<&str>,
    width: Option<f64>,
    height: Option<f64>,
    ratio: Option<f64>,
) -> Result<(), String> {
    let front = &stack.front;
    let mismatch = |what: &str, got: String, want: &str| {
        Err(format!(
            "pad {what} {got} must equal padstack.front ({want}): the pad header describes the front copper layer"
        ))
    };
    if let Some(shape) = shape {
        if shape != front.shape {
            return mismatch("shape", shape.to_string(), &front.shape);
        }
    }
    if let Some(width) = width {
        if format_micro(micro(width)) != front.width {
            return mismatch("width", width.to_string(), &front.width);
        }
    }
    if let Some(height) = height {
        if format_micro(micro(height)) != front.height {
            return mismatch("height", height.to_string(), &front.height);
        }
    }
    if let Some(ratio) = ratio {
        if Some(format_micro(micro(ratio))) != front.ratio {
            return mismatch(
                "roundrect_rratio",
                ratio.to_string(),
                front.ratio.as_deref().unwrap_or("none"),
            );
        }
    }
    Ok(())
}

/// Create-time check: the pad must be through-hole with copper on all layers.
pub(super) fn check_create_pad(pad_type: &str, layers: &[&str]) -> Result<(), String> {
    if pad_type != "thru_hole" {
        return Err(format!(
            "a per-layer padstack needs a thru_hole pad; this pad is '{pad_type}'"
        ));
    }
    if !layers.contains(&"*.Cu") {
        return Err("a per-layer padstack needs copper on all layers (*.Cu); the pad's layers list lacks it".into());
    }
    Ok(())
}

/// Header ratio token for a created pad, from the front layer.
pub(super) fn header_ratio_token(stack: &Stack) -> String {
    stack
        .front
        .ratio
        .as_ref()
        .map(|r| format!("(roundrect_rratio {r})"))
        .unwrap_or_default()
}

fn render_layer(name: &str, layer: &LayerSpec, layout: Option<(&str, &str, &str)>) -> String {
    let ratio = layer.ratio.as_ref();
    match layout {
        None => format!(
            "(layer \"{name}\" (shape {}) (size {} {}){})",
            layer.shape,
            layer.width,
            layer.height,
            ratio
                .map(|r| format!(" (roundrect_rratio {r})"))
                .unwrap_or_default()
        ),
        Some((nl, ind, unit)) => {
            let mut out = format!(
                "(layer \"{name}\"{nl}{ind}{unit}{unit}(shape {}){nl}{ind}{unit}{unit}(size {} {})",
                layer.shape, layer.width, layer.height
            );
            if let Some(r) = ratio {
                out.push_str(&format!("{nl}{ind}{unit}{unit}(roundrect_rratio {r})"));
            }
            out.push_str(&format!("{nl}{ind}{unit})"));
            out
        }
    }
}

/// The `(padstack …)` block. `layout` is (newline, child indent, indent unit)
/// for a multi-line pad, `None` for the one-line style Konnect creates.
pub(super) fn render(stack: &Stack, layout: Option<(&str, &str, &str)>) -> String {
    match layout {
        None => format!(
            "(padstack (mode {MODE}) {} {})",
            render_layer("Inner", &stack.inner, None),
            render_layer("B.Cu", &stack.back, None)
        ),
        Some((nl, ind, unit)) => format!(
            "(padstack{nl}{ind}{unit}(mode {MODE}){nl}{ind}{unit}{}{nl}{ind}{unit}{}{nl}{ind})",
            // The layer sits one unit in, its children two.
            render_layer("Inner", &stack.inner, Some((nl, ind, unit))),
            render_layer("B.Cu", &stack.back, Some((nl, ind, unit))),
        ),
    }
}

fn children(pad_block: &str) -> Vec<(usize, usize, String)> {
    find_direct_child_blocks(pad_block, "pad")
        .into_iter()
        .filter_map(|(s, e)| {
            let node = parse_sexp(&pad_block[s..e]).ok()?;
            Some((s, e, node.head()?.to_string()))
        })
        .collect()
}

struct Existing {
    range: (usize, usize),
    mode: String,
    layer_names: Vec<String>,
    extras: Vec<String>,
}

impl Existing {
    fn editable(&self) -> Result<(), String> {
        if self.mode != MODE {
            return Err(format!(
                "the pad uses a '{}' per-layer stack that depends on the board's copper layer count; it cannot be rewritten or cleared here",
                self.mode
            ));
        }
        if self.layer_names != ["Inner", "B.Cu"] {
            return Err(format!(
                "the existing {MODE} stack has unexpected layers {:?}; it cannot be rewritten faithfully",
                self.layer_names
            ));
        }
        if !self.extras.is_empty() {
            return Err(format!(
                "the existing padstack carries tokens Konnect cannot preserve ({}); rewriting it would lose them",
                self.extras.join(", ")
            ));
        }
        Ok(())
    }
}

fn existing(pad_block: &str) -> Option<Existing> {
    let (s, e, _) = children(pad_block)
        .into_iter()
        .find(|c| c.2 == "padstack")?;
    let node = parse_sexp(&pad_block[s..e]).ok()?;
    let mut out = Existing {
        range: (s, e),
        mode: String::new(),
        layer_names: Vec::new(),
        extras: Vec::new(),
    };
    for child in node.children().unwrap_or(&[]) {
        match child.head() {
            Some("mode") => {
                out.mode = child
                    .get(1)
                    .and_then(SexpNode::as_str)
                    .unwrap_or("")
                    .to_string()
            }
            Some("layer") => {
                out.layer_names.push(
                    child
                        .get(1)
                        .and_then(SexpNode::as_str)
                        .unwrap_or("")
                        .to_string(),
                );
                for extra in child.children().unwrap_or(&[]).iter().skip(2) {
                    if let Some(head) = extra.head() {
                        if !MODELLED.contains(&head) {
                            out.extras
                                .push(format!("{}:{head}", out.layer_names.last().unwrap()));
                        }
                    }
                }
            }
            Some(other) => out.extras.push(other.to_string()),
            None => {}
        }
    }
    Some(out)
}

/// A shape/width/height edit on a padstack pad without the stack would leave
/// the header and the stack describing different front layers.
pub(super) fn guard_header_edit(pad_block: &str, pad_number: &str) -> Result<(), String> {
    if existing(pad_block).is_some() {
        return Err(format!(
            "pad '{pad_number}' has a per-layer padstack; shape/width/height describe its front layer, so change them through the padstack argument instead"
        ));
    }
    Ok(())
}

fn node_str<'a>(node: &'a SexpNode, index: usize) -> Option<&'a str> {
    node.get(index).and_then(SexpNode::as_str)
}

fn insert_child(pad_block: &str, before: &[&str], text: &str) -> Option<SexpEdit> {
    let kids = children(pad_block);
    if let Some((s, _, _)) = kids.iter().find(|c| before.contains(&c.2.as_str())) {
        let sep = &pad_block[leading_whitespace_start(pad_block, *s)..*s];
        return Some(SexpEdit::insert(*s, format!("{text}{sep}")));
    }
    if let Some((s, e, _)) = kids.last() {
        let sep = &pad_block[leading_whitespace_start(pad_block, *s)..*s];
        return Some(SexpEdit::insert(*e, format!("{sep}{text}")));
    }
    pad_block
        .rfind(')')
        .map(|close| SexpEdit::insert(close, format!(" {text}")))
}

fn rewrite_header(pad_block: String, front: &LayerSpec) -> Result<String, String> {
    let (shape_start, shape_end) = super::pad_shape_range(&pad_block)
        .ok_or_else(|| "could not locate the pad shape token".to_string())?;
    let mut edits = vec![SexpEdit::replace(
        shape_start,
        shape_end,
        front.shape.clone(),
    )];
    let kids = children(&pad_block);
    let size = kids
        .iter()
        .find(|c| c.2 == "size")
        .ok_or_else(|| "the pad has no (size …) token".to_string())?;
    edits.push(SexpEdit::replace(
        size.0,
        size.1,
        format!("(size {} {})", front.width, front.height),
    ));
    let ratio = kids.iter().find(|c| c.2 == "roundrect_rratio");
    match (&front.ratio, ratio) {
        (Some(r), Some(c)) => edits.push(SexpEdit::replace(
            c.0,
            c.1,
            format!("(roundrect_rratio {r})"),
        )),
        (None, Some(c)) => edits.push(SexpEdit::delete(
            leading_whitespace_start(&pad_block, c.0),
            c.1,
        )),
        (Some(r), None) => {
            if let Some(edit) = insert_child(
                &pad_block,
                &AFTER_HEADER_RATIO,
                &format!("(roundrect_rratio {r})"),
            ) {
                edits.push(edit);
            }
        }
        (None, None) => {}
    }
    Ok(apply_edits(pad_block, edits))
}

/// Apply `change` to one pad block.
pub(super) fn apply(
    pad_block: String,
    change: &Change,
    pad_number: &str,
) -> Result<String, String> {
    let current = existing(&pad_block);
    match change {
        Change::Keep => Ok(pad_block),
        Change::Clear => match current {
            None => Ok(pad_block),
            Some(current) => {
                current.editable()?;
                let (s, e) = current.range;
                Ok(apply_edits(
                    pad_block.clone(),
                    vec![SexpEdit::delete(leading_whitespace_start(&pad_block, s), e)],
                ))
            }
        },
        Change::Set(stack) => {
            let node = parse_sexp(&pad_block).map_err(|e| format!("pad does not parse: {e}"))?;
            let copper = node
                .find("layers")
                .and_then(SexpNode::children)
                .unwrap_or_default()
                .iter()
                .skip(1)
                .filter_map(SexpNode::as_str)
                .collect::<Vec<_>>();
            check_create_pad(node_str(&node, 2).unwrap_or(""), &copper)
                .map_err(|reason| format!("pad '{pad_number}': {reason}"))?;
            if !SHAPES.contains(&node_str(&node, 3).unwrap_or("")) {
                return Err(format!(
                    "pad '{pad_number}' has a non-standard shape '{}'; it cannot take a padstack",
                    node_str(&node, 3).unwrap_or("")
                ));
            }
            let bad: Vec<_> = children(&pad_block)
                .into_iter()
                .map(|c| c.2)
                .filter(|h| HEADER_UNSUPPORTED.contains(&h.as_str()))
                .collect();
            if !bad.is_empty() {
                return Err(format!(
                    "pad '{pad_number}' has header geometry tokens ({}) that a padstack rewrite would not preserve",
                    bad.join(", ")
                ));
            }
            if let Some(current) = &current {
                current
                    .editable()
                    .map_err(|e| format!("pad '{pad_number}': {e}"))?;
            }
            let rewritten = rewrite_header(pad_block, &stack.front)?;
            // Re-locate after the header edit, then replace or insert the block.
            let layout = layout_of(&rewritten);
            let text = render(
                stack,
                layout
                    .as_ref()
                    .map(|(nl, ind, unit)| (nl.as_str(), ind.as_str(), unit.as_str())),
            );
            match existing(&rewritten) {
                Some(current) => {
                    let (s, e) = current.range;
                    Ok(apply_edits(rewritten, vec![SexpEdit::replace(s, e, text)]))
                }
                None => {
                    let edit = insert_child(&rewritten, &["teardrops"], &text)
                        .ok_or_else(|| "could not place the padstack block".to_string())?;
                    Ok(apply_edits(rewritten, vec![edit]))
                }
            }
        }
    }
}

/// (newline, child indent, indent unit) of a multi-line pad block, taken from
/// the last child that starts on its own line (a child appended inline, such
/// as ` (drill 0.8)`, says nothing about indentation). `None` for a one-line
/// pad.
fn layout_of(pad_block: &str) -> Option<(String, String, String)> {
    let nl = if pad_block.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    for (s, _, _) in children(pad_block).into_iter().rev() {
        let ws = &pad_block[leading_whitespace_start(pad_block, s)..s];
        if ws.contains('\n') {
            let ind = ws.rsplit('\n').next().unwrap_or("").to_string();
            let unit = if ind.contains('\t') { "\t" } else { "  " };
            return Some((nl.to_string(), ind, unit.to_string()));
        }
    }
    None
}

pub(super) struct Readback {
    pub(super) padstack: serde_json::Value,
    /// Replacement for `copper_layers` when the stack is not `normal`.
    pub(super) copper_layers: Option<Vec<serde_json::Value>>,
}

fn layer_json(
    name: &str,
    shape: &str,
    node: &SexpNode,
    report_unmodelled: bool,
) -> Result<serde_json::Value, String> {
    if !READABLE_SHAPES.contains(&shape) {
        return Err(format!("layer {name} has unknown shape '{shape}'"));
    }
    let size = node
        .find("size")
        .and_then(|s| Some((s.get_f64(1)?, s.get_f64(2)?)))
        .ok_or_else(|| format!("layer {name} has no readable size"))?;
    let mut out = json!({
        "layer": name,
        "shape": shape,
        "size": {"x": size.0, "y": size.1},
    });
    if let Some(ratio) = node.find("roundrect_rratio").and_then(|n| n.get_f64(1)) {
        out["corner_rounding_ratio"] = json!(ratio);
    }
    let unmodelled: Vec<&str> = if report_unmodelled {
        node.children()
            .unwrap_or(&[])
            .iter()
            .skip(2)
            .filter_map(SexpNode::head)
            .filter(|h| !MODELLED.contains(h))
            .collect()
    } else {
        Vec::new()
    };
    if !unmodelled.is_empty() {
        out["unmodelled"] = json!(unmodelled);
    }
    Ok(out)
}

fn read_inner(pad: &SexpNode) -> Result<Readback, String> {
    let stack = pad
        .children()
        .unwrap_or(&[])
        .iter()
        .find(|c| c.head() == Some("padstack"));
    let Some(stack) = stack else {
        return Ok(Readback {
            padstack: json!({"mode": "normal"}),
            copper_layers: None,
        });
    };
    let mode = stack
        .children()
        .unwrap_or(&[])
        .iter()
        .find(|c| c.head() == Some("mode"))
        .and_then(|m| node_str(m, 1))
        .ok_or("no mode")?;
    let header_shape = node_str(pad, 3).ok_or("pad has no shape")?;
    // The header is the front layer: its size and roundrect ratio are the front's.
    let mut header = layer_json("F.Cu", header_shape, pad, false)?;
    let header_extra: Vec<&str> = pad
        .children()
        .unwrap_or(&[])
        .iter()
        .filter_map(SexpNode::head)
        .filter(|h| HEADER_UNSUPPORTED.contains(h))
        .collect();
    if !header_extra.is_empty() {
        header["unmodelled"] = json!(header_extra);
    }
    let layers: Vec<(String, &SexpNode)> = stack
        .children()
        .unwrap_or(&[])
        .iter()
        .filter(|c| c.head() == Some("layer"))
        .map(|c| (node_str(c, 1).unwrap_or("").to_string(), c))
        .collect();
    let read_layer = |name: &str, node: &SexpNode| -> Result<serde_json::Value, String> {
        let shape = node
            .find("shape")
            .and_then(|s| node_str(s, 1))
            .ok_or_else(|| format!("layer {name} has no shape"))?;
        layer_json(name, shape, node, true)
    };
    match mode {
        MODE => {
            let find = |name: &str| layers.iter().find(|(n, _)| n == name);
            let inner = find("Inner").ok_or("front_inner_back stack has no Inner layer")?;
            let back = find("B.Cu").ok_or("front_inner_back stack has no B.Cu layer")?;
            if layers.len() != 2 {
                return Err(format!(
                    "front_inner_back stack has unexpected layers {:?}",
                    layers.iter().map(|l| l.0.as_str()).collect::<Vec<_>>()
                ));
            }
            let (inner, back) = (read_layer("Inner", inner.1)?, read_layer("B.Cu", back.1)?);
            Ok(Readback {
                padstack: json!({"mode": MODE, "front": header, "inner": inner, "back": back}),
                copper_layers: Some(vec![header, inner, back]),
            })
        }
        "custom" => {
            let mut list = Vec::new();
            for (name, node) in &layers {
                list.push(read_layer(name, node)?);
            }
            let mut all = vec![header];
            all.extend(list.iter().cloned());
            Ok(Readback {
                padstack: json!({"mode": "custom", "layers": list}),
                copper_layers: Some(all),
            })
        }
        other => Err(format!("unknown padstack mode '{other}'")),
    }
}

pub(super) fn read_pad(pad: &SexpNode) -> Result<Readback, String> {
    read_inner(pad).map_err(|e| format!("padstack: {e}"))
}

fn same(layer: &serde_json::Value, spec: &LayerSpec) -> bool {
    let text = |v: &serde_json::Value| v.as_f64().map(|n| format_micro(micro(n)));
    layer["shape"] == spec.shape.as_str()
        && text(&layer["size"]["x"]).as_deref() == Some(spec.width.as_str())
        && text(&layer["size"]["y"]).as_deref() == Some(spec.height.as_str())
        && text(&layer["corner_rounding_ratio"]) == spec.ratio
        && layer.get("unmodelled").is_none()
}

/// Check, before anything is written, that the rewritten pad reads back as requested.
pub(super) fn verify(pad_block: &str, change: &Change) -> Result<(), String> {
    let node = parse_sexp(pad_block).map_err(|e| format!("edited pad does not parse: {e}"))?;
    let read = read_pad(&node)?;
    match change {
        Change::Keep => Ok(()),
        Change::Clear => (read.padstack["mode"] == "normal")
            .then_some(())
            .ok_or_else(|| "padstack was not cleared".to_string()),
        Change::Set(stack) => {
            let ps = &read.padstack;
            let ok = ps["mode"] == MODE
                && same(&ps["front"], &stack.front)
                && same(&ps["inner"], &stack.inner)
                && same(&ps["back"], &stack.back);
            ok.then_some(())
                .ok_or_else(|| "padstack did not read back as requested".to_string())
        }
    }
}
