//! AILAB-41: pad-local solder-mask margin, solder-paste margin and paste ratio.
//!
//! Values are converted to integer nanometres (ratio: millionths) with
//! rounding and written from the integer, so `0.075` is never saved as
//! `0.074999`.
//!
//! KiCad 10.0.6 reads a pad-local `0` for all three settings as *unset* and
//! drops the token when it saves (observed natively; see
//! docs/AILAB-41-VERIFICATION.md). A zero override therefore cannot be stored,
//! so a zero request is refused and an existing zero token is reported as
//! inherited, exactly as KiCad treats it.
use super::*;

/// Largest pad-local margin Konnect will write, in mm. KiCad does not bound
/// the token; this guards against unit mistakes (e.g. nm passed as mm).
const MAX_MARGIN_MM: f64 = 10.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Margin,
    Ratio,
}

/// KiCad's writer order for these three tokens.
pub(super) const FIELDS: [(&str, Kind); 3] = [
    ("solder_mask_margin", Kind::Margin),
    ("solder_paste_margin", Kind::Margin),
    ("solder_paste_margin_ratio", Kind::Ratio),
];

/// Tokens KiCad writes after the mask/paste group; a new token goes before
/// the first of these that exists.
const AFTER_GROUP: &[&str] = &[
    "clearance",
    "zone_connect",
    "thermal_bridge_width",
    "thermal_bridge_angle",
    "thermal_gap",
    "options",
    "primitives",
    "zone_layer_connections",
    "padstack",
    "teardrops",
    "uuid",
];

#[derive(Clone, PartialEq, Debug)]
pub(super) enum Change {
    Keep,
    Clear,
    /// Token text as written to the file, e.g. `0.075`.
    Set(String),
}

#[derive(Clone, Debug)]
pub(super) struct Spec {
    changes: [Change; 3],
}

impl Spec {
    pub(super) fn is_empty(&self) -> bool {
        self.changes.iter().all(|c| *c == Change::Keep)
    }

    /// Text of the tokens to append when creating a pad (omitted/null: none).
    pub(super) fn create_tokens(&self) -> String {
        let mut out = String::new();
        for ((tag, _), change) in FIELDS.iter().zip(&self.changes) {
            if let Change::Set(text) = change {
                out.push_str(&format!(" ({tag} {text})"));
            }
        }
        out
    }
}

/// Format an integer count of millionths as a KiCad number: no exponent, no
/// trailing zeros, `0` for zero.
fn format_micro(micro: i64) -> String {
    if micro == 0 {
        return "0".into();
    }
    let sign = if micro < 0 { "-" } else { "" };
    let abs = micro.unsigned_abs();
    let (whole, frac) = (abs / 1_000_000, abs % 1_000_000);
    if frac == 0 {
        format!("{sign}{whole}")
    } else {
        let frac = format!("{frac:06}");
        format!("{sign}{whole}.{}", frac.trim_end_matches('0'))
    }
}

const ZERO_REFUSAL: &str = "a pad-local 0 (or a value that rounds to 0) cannot be stored: KiCad 10.0.6 reads it as unset and drops it. Pass null to inherit, or a non-zero value";

fn parse_one(value: &serde_json::Value, kind: Kind, allow_clear: bool) -> Result<Change, String> {
    if value.is_null() {
        return Ok(if allow_clear {
            Change::Clear
        } else {
            Change::Keep
        });
    }
    let Some(number) = value.as_f64() else {
        return Err("must be a number (or null to clear) when supplied".into());
    };
    if !number.is_finite() {
        return Err("must be finite".into());
    }
    if (number * 1e6).round() == 0.0 {
        return Err(ZERO_REFUSAL.into());
    }
    match kind {
        Kind::Margin if number.abs() > MAX_MARGIN_MM => Err(format!(
            "must be within +/-{MAX_MARGIN_MM} mm; got {number}"
        )),
        Kind::Ratio if !(-1.0..=1.0).contains(&number) => {
            Err(format!("ratio must be between -1 and 1; got {number}"))
        }
        _ => Ok(Change::Set(format_micro((number * 1e6).round() as i64))),
    }
}

/// Read the three fields from a create pad object or an edit argument object.
/// `prefix` names the object in refusals (`pads[2].` or empty).
pub(super) fn parse(
    object: &serde_json::Value,
    prefix: &str,
    allow_clear: bool,
) -> Result<Spec, (String, String)> {
    let mut changes = [Change::Keep, Change::Keep, Change::Keep];
    for (slot, (tag, kind)) in changes.iter_mut().zip(FIELDS) {
        if let Some(value) = object.get(tag) {
            *slot = parse_one(value, kind, allow_clear)
                .map_err(|reason| (format!("{prefix}{tag}"), reason))?;
        }
    }
    Ok(Spec { changes })
}

/// Direct `(tag …)` children of a pad block, as byte ranges.
fn token_ranges(pad_block: &str, tag: &str) -> Vec<(usize, usize)> {
    find_direct_child_blocks(pad_block, "pad")
        .into_iter()
        .filter(|&(s, e)| parse_sexp(&pad_block[s..e]).is_ok_and(|child| child.head() == Some(tag)))
        .collect()
}

/// Apply `spec` to one pad block. Untouched fields keep their exact bytes.
pub(super) fn apply(mut pad_block: String, spec: &Spec) -> String {
    for (index, ((tag, _), change)) in FIELDS.iter().zip(&spec.changes).enumerate() {
        if *change == Change::Keep {
            continue;
        }
        let existing = token_ranges(&pad_block, tag);
        let mut edits = Vec::new();
        match (change, existing.split_first()) {
            (Change::Set(text), Some((&(s, e), duplicates))) => {
                edits.push(SexpEdit::replace(s, e, format!("({tag} {text})")));
                for &(s, e) in duplicates {
                    edits.push(SexpEdit::delete(leading_whitespace_start(&pad_block, s), e));
                }
            }
            (Change::Set(text), None) => {
                let later: Vec<&str> = FIELDS[index + 1..]
                    .iter()
                    .map(|(t, _)| *t)
                    .chain(AFTER_GROUP.iter().copied())
                    .collect();
                let children = find_direct_child_blocks(&pad_block, "pad");
                let before = children.iter().find(|&&(s, e)| {
                    parse_sexp(&pad_block[s..e])
                        .is_ok_and(|c| c.head().is_some_and(|h| later.contains(&h)))
                });
                if let Some(&(s, _)) = before {
                    let sep = &pad_block[leading_whitespace_start(&pad_block, s)..s];
                    edits.push(SexpEdit::insert(s, format!("({tag} {text}){sep}")));
                } else if let Some(&(s, e)) = children.last() {
                    let sep = &pad_block[leading_whitespace_start(&pad_block, s)..s];
                    edits.push(SexpEdit::insert(e, format!("{sep}({tag} {text})")));
                } else if let Some(close) = pad_block.rfind(')') {
                    edits.push(SexpEdit::insert(close, format!(" ({tag} {text})")));
                }
            }
            (Change::Clear, _) => {
                for &(s, e) in &existing {
                    edits.push(SexpEdit::delete(leading_whitespace_start(&pad_block, s), e));
                }
            }
            (Change::Keep, _) => {}
        }
        pad_block = apply_edits(pad_block, edits);
    }
    pad_block
}

/// The value of a direct child token: `Ok(None)` when absent, `Err` when
/// present but not a finite number. KiCad's parser keeps the last duplicate.
fn token_value(node: &SexpNode, tag: &str) -> Result<Option<f64>, String> {
    let mut found = None;
    for child in node.children().unwrap_or(&[]) {
        if child.head() == Some(tag) {
            match child.get_f64(1) {
                Some(v) if v.is_finite() => found = Some(v),
                _ => return Err(format!("{tag} is not a finite number")),
            }
        }
    }
    Ok(found)
}

/// Per-pad readback. Effective is the pad's own value, else the footprint's,
/// else unknown: board and project defaults are not stored in a .kicad_mod.
/// A `0` token is what KiCad reads as unset, so it is reported as inherited
/// and flagged (`file_token_zero` / `footprint_token_zero`).
pub(super) fn read_pad(pad: &SexpNode, footprint: &SexpNode) -> Result<serde_json::Value, String> {
    let mut out = serde_json::Map::new();
    for (tag, _) in FIELDS {
        let raw_local = token_value(pad, tag).map_err(|e| format!("pad: {e}"))?;
        let raw_inherited = token_value(footprint, tag).map_err(|e| format!("footprint: {e}"))?;
        let local = raw_local.filter(|v| *v != 0.0);
        let inherited = raw_inherited.filter(|v| *v != 0.0);
        let (source, effective) = match (local, inherited) {
            (Some(v), _) => ("pad", json!(v)),
            (None, Some(v)) => ("footprint", json!(v)),
            (None, None) => ("unknown", serde_json::Value::Null),
        };
        let mut entry = json!({
            "local": local,
            "source": source,
            "effective": effective,
        });
        if raw_local == Some(0.0) {
            entry["file_token_zero"] = json!(true);
        }
        if raw_inherited == Some(0.0) {
            entry["footprint_token_zero"] = json!(true);
        }
        out.insert(tag.to_string(), entry);
    }
    Ok(serde_json::Value::Object(out))
}

/// Check that a rewritten pad block reads back as `spec` requested, before
/// anything is written.
pub(super) fn verify(pad_block: &str, spec: &Spec) -> Result<(), String> {
    let node = parse_sexp(pad_block).map_err(|e| format!("edited pad does not parse: {e}"))?;
    for ((tag, _), change) in FIELDS.iter().zip(&spec.changes) {
        let observed = token_value(&node, tag)?;
        let ok = match change {
            Change::Keep => true,
            Change::Clear => observed.is_none(),
            Change::Set(text) => {
                observed.is_some_and(|v| format_micro((v * 1e6).round() as i64) == *text)
            }
        };
        if !ok {
            return Err(format!("{tag} did not read back as requested"));
        }
    }
    Ok(())
}
