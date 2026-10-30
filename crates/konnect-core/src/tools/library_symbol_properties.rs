//! Typed library-field patches. Only touched fields are changed; no tree reserialization.
use super::*;
use serde::Deserialize;
use std::collections::HashSet;

pub(super) fn schema() -> serde_json::Value {
    json!({"type":"array","description":"Named string fields. New fields default hidden at (0,0,0); edits preserve omitted presentation. Coordinates in mm, angles in degrees.",
        "items":{"type":"object","additionalProperties":false,"required":["name","value"],"properties":{
            "name":{"type":"string","minLength":1},"value":{"type":"string"},
            "visible":{"type":"boolean"},"x":{"type":"number"},"y":{"type":"number"},
            "angle":{"type":"number","enum":[0,90,180,270]}}}})
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PropertyUpdate {
    name: String,
    value: String,
    visible: Option<bool>,
    x: Option<f64>,
    y: Option<f64>,
    angle: Option<f64>,
}

pub(super) fn parse_updates(
    args: &serde_json::Value,
    creating: bool,
) -> Result<Vec<PropertyUpdate>, CallToolResult> {
    let mut updates = Vec::new();
    let mut names = HashSet::new();
    if let Some(fields) = args.get("properties") {
        let fields = fields
            .as_array()
            .ok_or_else(|| invalid_library_argument("properties", "must be an array"))?;
        for (index, field) in fields.iter().enumerate() {
            let key = format!("properties[{index}]");
            // Option<T> accepts null in serde, but omission alone means preserve.
            if field
                .as_object()
                .is_some_and(|o| o.values().any(serde_json::Value::is_null))
            {
                return Err(invalid_library_argument(
                    &key,
                    "null is not an omitted attribute",
                ));
            }
            let update: PropertyUpdate = serde_json::from_value(field.clone())
                .map_err(|e| invalid_library_argument(&key, e.to_string()))?;
            if update.name.trim().is_empty()
                || update.name.chars().any(char::is_control)
                || update.value.contains('\0')
            {
                return Err(invalid_library_argument(
                    &key,
                    "name must be nonblank without control characters; value cannot contain NUL",
                ));
            }
            if !names.insert(update.name.clone()) {
                return Err(invalid_library_argument(&key, "duplicate property name"));
            }
            if creating
                && ["Reference", "Value", "Datasheet", "Footprint"].contains(&update.name.as_str())
            {
                return Err(invalid_library_argument(&key,"use reference_prefix, value, datasheet or footprint for standard creation fields"));
            }
            if [update.x, update.y, update.angle]
                .into_iter()
                .flatten()
                .any(|v| !v.is_finite())
                || update
                    .angle
                    .is_some_and(|v| ![0.0, 90.0, 180.0, 270.0].contains(&v))
            {
                return Err(invalid_library_argument(
                    &key,
                    "coordinates must be finite and angle must be 0, 90, 180 or 270",
                ));
            }
            updates.push(update);
        }
    }
    if let Some(footprint) = args.get("footprint") {
        let value = footprint
            .as_str()
            .filter(|v| !v.contains('\0'))
            .ok_or_else(|| invalid_library_argument("footprint", "must be a string without NUL"))?;
        if names.contains("Footprint") {
            return Err(invalid_library_argument(
                "footprint",
                "conflicts with properties entry Footprint",
            ));
        }
        updates.push(PropertyUpdate {
            name: "Footprint".into(),
            value: value.into(),
            visible: None,
            x: None,
            y: None,
            angle: None,
        });
    }
    if !creating && args.get("properties").is_none() && args.get("footprint").is_none() {
        return Err(invalid_library_argument(
            "properties",
            "provide properties or footprint",
        ));
    }
    Ok(updates)
}

fn quoted(text: &str) -> String {
    // KiCad string escaping, including controls supported by konnect-sexp.
    format!(
        "\"{}\"",
        escape_library_string(text)
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    )
}

fn library_root(content: &str) -> Result<SexpNode, CallToolResult> {
    let start = content.len() - content.trim_start().len();
    let (_, end) = find_balanced_block(content, start)
        .filter(|(_, end)| content[*end..].trim().is_empty())
        .ok_or_else(|| {
            invalid_library_argument("library_path", "malformed library or trailing data")
        })?;
    let root = parse_sexp(&content[start..end])
        .map_err(|e| invalid_library_argument("library_path", e.to_string()))?;
    if root.head() != Some("kicad_symbol_lib") {
        return Err(invalid_library_argument(
            "library_path",
            "expected kicad_symbol_lib",
        ));
    }
    Ok(root)
}

pub(super) fn read_for_create(path: &Path, name: &str) -> Result<Option<String>, CallToolResult> {
    // Symbol names are used throughout the existing geometry builder. Refuse
    // unsafe names until that builder supports escaped identifiers everywhere.
    if name.is_empty()
        || name
            .chars()
            .any(|c| c.is_control() || ['"', '\\', ':', '/'].contains(&c))
    {
        return Err(invalid_library_argument(
            "name",
            "unsupported symbol name character",
        ));
    }
    let content = match read_consistent(path) {
        Ok(content) => content,
        Err(konnect_sexp::SexpError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None)
        }
        Err(e) => return Err(invalid_library_argument("library_path", e.to_string())),
    };
    let root = library_root(&content)?;
    if root
        .find_all("symbol")
        .iter()
        .any(|s| s.get(1).and_then(SexpNode::as_str) == Some(name))
    {
        return Err(invalid_library_argument(
            "name",
            "symbol already exists; use edit_symbol_properties to update fields",
        ));
    }
    Ok(Some(content))
}

/// Token boundaries must match the parser, including quotes adjacent to atoms.
/// Pad headers have a narrower lexical contract and must not be reused here.
fn field_token(source: &str, offset: usize) -> Option<(usize, usize)> {
    let start = skip_space(source, offset);
    let mut end = start;
    if source.as_bytes().get(start) == Some(&b'"') {
        end += 1;
        while end < source.len() {
            match source.as_bytes()[end] {
                b'\\' => end += 2,
                b'"' => return Some((start, end + 1)),
                _ => end += 1,
            }
        }
        return None;
    }
    end = start
        + source[start..]
            .chars()
            .take_while(|c| !c.is_whitespace() && !['(', ')', '"'].contains(c))
            .map(char::len_utf8)
            .sum::<usize>();
    (end > start).then_some((start, end))
}

fn skip_space(source: &str, offset: usize) -> usize {
    offset
        + source[offset..]
            .chars()
            .take_while(|c| c.is_whitespace())
            .map(char::len_utf8)
            .sum::<usize>()
}

/// Direct children of this exact root, without rediscovering a tag elsewhere.
fn child_blocks(source: &str) -> Result<Vec<(usize, usize)>, CallToolResult> {
    let start = source.len() - source.trim_start().len();
    if source.as_bytes().get(start) != Some(&b'(') {
        return Err(invalid_library_argument(
            "library_path",
            "expected an S-expression root",
        ));
    }
    let (_, end) = find_balanced_block(source, start)
        .filter(|(_, end)| source[*end..].trim().is_empty())
        .ok_or_else(|| invalid_library_argument("library_path", "malformed root"))?;
    let mut blocks = Vec::new();
    let mut offset = start + 1;
    while offset < end - 1 {
        offset = skip_space(source, offset);
        if offset >= end - 1 {
            break;
        }
        if source.as_bytes()[offset] == b'(' {
            let span = find_balanced_block(source, offset)
                .filter(|(_, b)| *b < end)
                .ok_or_else(|| invalid_library_argument("library_path", "malformed child block"))?;
            blocks.push(span);
            offset = span.1;
        } else {
            offset = field_token(source, offset)
                .ok_or_else(|| invalid_library_argument("library_path", "malformed token"))?
                .1;
        }
    }
    Ok(blocks)
}

fn validate_property(property: &SexpNode) -> Result<(), CallToolResult> {
    for tag in ["at", "effects", "hide"] {
        if property.find_all(tag).len() > 1 {
            return Err(invalid_library_argument(
                "properties",
                format!("duplicate existing {tag} attribute"),
            ));
        }
    }
    if let Some(at) = property.find("at") {
        if at.children().is_none_or(|c| c.len() != 4)
            || (1..=3).any(|i| at.get_f64(i).is_none_or(|v| !v.is_finite()))
        {
            return Err(invalid_library_argument(
                "properties",
                "existing position must contain three finite numbers",
            ));
        }
    }
    for parent in [Some(property), property.find("effects")]
        .into_iter()
        .flatten()
    {
        let hides = parent.find_all("hide");
        if hides.len() > 1
            || hides.iter().any(|n| {
                n.children().is_none_or(|c| c.len() > 2)
                    || n.get(1)
                        .is_some_and(|n| !matches!(n.as_str(), Some("yes" | "no")))
            })
        {
            return Err(invalid_library_argument(
                "properties",
                "malformed existing visibility",
            ));
        }
    }
    Ok(())
}

fn property_value_range(block: &str) -> Option<(usize, usize)> {
    let (_, head_end) = field_token(block, 1)?;
    let (_, name_end) = field_token(block, head_end)?;
    field_token(block, name_end)
}

fn patch_property(block: &str, update: &PropertyUpdate) -> Result<String, CallToolResult> {
    let node =
        parse_sexp(block).map_err(|e| invalid_library_argument("properties", e.to_string()))?;
    let (start, end) = property_value_range(block)
        .ok_or_else(|| invalid_library_argument("properties", "malformed field"))?;
    let mut edits = vec![SexpEdit::replace(start, end, quoted(&update.value))];
    let children = child_blocks(block)?;
    if update.x.is_some() || update.y.is_some() || update.angle.is_some() {
        if let Some(&(a, b)) = children
            .iter()
            .find(|&&(a, b)| parse_sexp(&block[a..b]).is_ok_and(|n| n.head() == Some("at")))
        {
            let at = &block[a..b];
            let (_, mut offset) = field_token(at, 1).unwrap();
            for value in [update.x, update.y, update.angle] {
                let (start, end) = field_token(at, offset)
                    .filter(|&(s, e)| at[s..e].parse::<f64>().is_ok_and(f64::is_finite))
                    .ok_or_else(|| {
                        invalid_library_argument("properties", "existing position is malformed")
                    })?;
                if let Some(value) = value {
                    edits.push(SexpEdit::replace(a + start, a + end, fmt_f64(value)));
                }
                offset = end;
            }
        } else {
            let replacement = format!(
                " (at {} {} {})",
                fmt_f64(update.x.unwrap_or(0.0)),
                fmt_f64(update.y.unwrap_or(0.0)),
                fmt_f64(update.angle.unwrap_or(0.0))
            );
            edits.push(SexpEdit::insert(block.rfind(')').unwrap(), replacement));
        }
    }
    if let Some(visible) = update
        .visible
        .filter(|visible| *visible == is_hidden(&node))
    {
        // KiCad 10 uses a top-level property hide flag. Older libraries can
        // carry the hide atom or list inside effects; remove both forms.
        for &(a, b) in &children {
            let child = parse_sexp(&block[a..b]).unwrap();
            if child.head() == Some("hide") {
                edits.push(SexpEdit::delete(a, b));
            }
            if child.head() == Some("effects") {
                let effects = &block[a..b];
                let mut i = 1;
                while i < effects.len() - 1 {
                    i = skip_space(effects, i);
                    if effects.as_bytes().get(i) == Some(&b'(') {
                        let (s, e) = find_balanced_block(effects, i).unwrap();
                        if parse_sexp(&effects[s..e]).is_ok_and(|n| n.head() == Some("hide")) {
                            edits.push(SexpEdit::delete(a + s, a + e));
                        }
                        i = e;
                    } else if let Some((s, e)) = field_token(effects, i) {
                        if &effects[s..e] == "hide" {
                            edits.push(SexpEdit::delete(a + s, a + e));
                        }
                        i = e;
                    } else {
                        break;
                    }
                }
            }
        }
        if !visible {
            edits.push(SexpEdit::insert(block.rfind(')').unwrap(), " (hide yes)"));
        }
    }
    // Merge insertions at a shared closing offset to preserve deterministic order.
    let mut merged: Vec<SexpEdit> = Vec::new();
    for edit in edits {
        if edit.start == edit.end {
            if let Some(existing) = merged
                .iter_mut()
                .find(|e| e.start == edit.start && e.end == edit.end)
            {
                existing.replacement.push_str(&edit.replacement);
                continue;
            }
        }
        merged.push(edit);
    }
    Ok(apply_edits(block.to_string(), merged))
}

pub(super) fn patch_symbol(
    source: &str,
    updates: &[PropertyUpdate],
) -> Result<String, CallToolResult> {
    let root =
        parse_sexp(source).map_err(|e| invalid_library_argument("symbol_name", e.to_string()))?;
    if root.head() != Some("symbol") {
        return Err(invalid_library_argument("symbol_name", "malformed symbol"));
    }
    if root.find("extends").is_some() {
        return Err(invalid_library_argument(
            "symbol_name",
            "inherited symbol fields require an inheritance-aware editor",
        ));
    }
    let mut fields = std::collections::HashMap::new();
    for (start, end) in child_blocks(source)? {
        let node = parse_sexp(&source[start..end]).unwrap();
        if node.head() != Some("property") {
            continue;
        }
        validate_property(&node)?;
        let name = node
            .get(1)
            .and_then(SexpNode::as_str)
            .ok_or_else(|| invalid_library_argument("properties", "existing field has no name"))?;
        if !matches!(node.get(2), Some(SexpNode::Str(_)))
            || fields.insert(name.to_string(), (start, end)).is_some()
        {
            return Err(invalid_library_argument(
                "properties",
                "existing fields are malformed or duplicated",
            ));
        }
    }
    let mut edits = Vec::new();
    let mut added = String::new();
    for update in updates {
        if let Some(&(start, end)) = fields.get(&update.name) {
            let patched = patch_property(&source[start..end], update)?;
            if patched != source[start..end] {
                edits.push(SexpEdit::replace(start, end, patched));
            }
        } else {
            let visible = update.visible.unwrap_or(false);
            added.push_str(&format!(
                "\n    (property {} {} (at {} {} {}) (effects (font (size 1.27 1.27))){} )",
                quoted(&update.name),
                quoted(&update.value),
                fmt_f64(update.x.unwrap_or(0.0)),
                fmt_f64(update.y.unwrap_or(0.0)),
                fmt_f64(update.angle.unwrap_or(0.0)),
                if visible { "" } else { " (hide yes)" }
            ));
        }
    }
    if !added.is_empty() {
        edits.push(SexpEdit::insert(source.rfind(')').unwrap(), added));
    }
    let patched = apply_edits(source.to_string(), edits);
    // Validate the candidate before it can cross the persistence boundary.
    let candidate = parse_sexp(&patched)
        .map_err(|e| invalid_library_argument("properties", format!("invalid candidate: {e}")))?;
    for property in candidate.find_all("property") {
        validate_property(property)?;
    }
    Ok(patched)
}

fn is_hidden(property: &SexpNode) -> bool {
    let effects = property.find("effects");
    property
        .find("hide")
        .is_some_and(|n| n.get(1).and_then(SexpNode::as_str) != Some("no"))
        || effects.is_some_and(|n| {
            n.children().unwrap_or(&[]).iter().any(|n| {
                n.as_str() == Some("hide")
                    || (n.head() == Some("hide")
                        && n.get(1).and_then(SexpNode::as_str) != Some("no"))
            })
        })
}

pub(super) fn property_details(symbol: &SexpNode) -> serde_json::Value {
    let mut details = serde_json::Map::new();
    for property in symbol.find_all("property") {
        let Some(name) = property.get(1).and_then(SexpNode::as_str) else {
            continue;
        };
        let hidden = is_hidden(property);
        let at = property.find("at");
        details.insert(name.into(),json!({"value":property.get(2).and_then(SexpNode::as_str),"visible":!hidden,
            "x":at.and_then(|n|n.get_f64(1)),"y":at.and_then(|n|n.get_f64(2)),"angle":at.and_then(|n|n.get_f64(3))}));
    }
    json!(details)
}

fn uncertain(path: &Path, reason: impl ToString) -> CallToolResult {
    CallToolResult::error_kind(
        ToolErrorKind::MutationOutcomeUncertain {
            operation: "write_symbol_properties".into(),
            path: path.display().to_string(),
            reason: reason.to_string(),
        },
        "Library write or readback could not be verified; inspect the saved file before retrying",
    )
}

pub(super) fn persist(
    path: &Path,
    original: Option<&str>,
    next: &str,
) -> Result<(), CallToolResult> {
    // Check conflicts before the write boundary separately from uncertain I/O.
    if let Some(original) = original {
        let current = read_consistent(path)
            .map_err(|e| invalid_library_argument("library_path", e.to_string()))?;
        if current != original {
            return Err(CallToolResult::error_kind(
                ToolErrorKind::Conflict {
                    paths: vec![path.display().to_string()],
                },
                "Library changed before writing",
            ));
        }
        if current == next {
            return Ok(());
        }
        write_atomic_if_unchanged(path, original, next).map_err(|e| uncertain(path, e))?;
    } else {
        write_new_atomic(path, next).map_err(|e| uncertain(path, e))?;
    }
    if read_consistent(path).map_err(|e| uncertain(path, e))? != next {
        return Err(uncertain(path, "saved bytes do not match"));
    }
    Ok(())
}

pub(super) fn readback(
    path: &Path,
    name: &str,
    expected: &str,
) -> Result<serde_json::Value, CallToolResult> {
    let content = read_consistent(path).map_err(|e| uncertain(path, e))?;
    if content != expected {
        return Err(uncertain(path, "library changed before semantic readback"));
    }
    let root =
        library_root(&content).map_err(|_| uncertain(path, "saved library cannot be parsed"))?;
    let matches: Vec<_> = root
        .find_all("symbol")
        .into_iter()
        .filter(|s| s.get(1).and_then(SexpNode::as_str) == Some(name))
        .collect();
    if matches.len() != 1 {
        return Err(uncertain(path, "saved symbol is missing or duplicated"));
    }
    let symbol = matches[0];
    let mut properties = serde_json::Map::new();
    for field in symbol.find_all("property") {
        if let (Some(name), Some(value)) = (
            field.get(1).and_then(SexpNode::as_str),
            field.get(2).and_then(SexpNode::as_str),
        ) {
            properties.insert(name.into(), json!(value));
        }
    }
    Ok(json!({"properties":properties,"property_details":property_details(symbol)}))
}

pub(super) async fn handle_edit_symbol_properties(
    args: &serde_json::Value,
    _ctx: &ToolContext,
) -> anyhow::Result<CallToolResult> {
    let path = get_path(args, "library_path")?;
    let name = match require_str(args, "symbol_name") {
        Ok(name) => name,
        Err(e) => return Ok(e),
    };
    let run = || -> Result<serde_json::Value, CallToolResult> {
        let updates = parse_updates(args, false)?;
        let original = read_consistent(&path)
            .map_err(|e| invalid_library_argument("library_path", e.to_string()))?;
        library_root(&original)?;
        let matches: Vec<_> = child_blocks(&original)?
            .into_iter()
            .filter(|&(s, e)| {
                parse_sexp(&original[s..e]).is_ok_and(|n| {
                    n.head() == Some("symbol") && n.get(1).and_then(SexpNode::as_str) == Some(name)
                })
            })
            .collect();
        if matches.len() != 1 {
            return Err(invalid_library_argument(
                "symbol_name",
                "expected exactly one matching symbol",
            ));
        }
        let (start, end) = matches[0];
        let replacement = patch_symbol(&original[start..end], &updates)?;
        let next = apply_edits(
            original.clone(),
            vec![SexpEdit::replace(start, end, replacement)],
        );
        persist(&path, Some(&original), &next)?;
        let mut result = readback(&path, name, &next)?;
        result["success"] = json!(true);
        result["changed"] = json!(next != original);
        result["source"] = json!("saved_file");
        result["symbol"] = json!(name);
        result["library"] = json!(path);
        Ok(result)
    };
    Ok(match run() {
        Ok(result) => CallToolResult::text(result.to_string()),
        Err(error) => error,
    })
}
