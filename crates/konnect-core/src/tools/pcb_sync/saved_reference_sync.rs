//! Closed-board reference migration: preserve every byte outside reference values.
use super::*;
use konnect_sexp::writer::{
    apply_edits, find_direct_child_blocks, kicad_editor_lock_path, read_consistent,
    write_atomic_if_unchanged, SexpEdit,
};

struct FootprintSpan {
    id: String,
    old: String,
    value_start: usize,
    value_end: usize,
    at_start: usize,
    at_end: usize,
    at_old: String,
    origin: Option<(f64, f64, f64)>,
    angle: Option<f64>,
    symbol_path: Option<String>,
    pads: Vec<PadNetSpan>,
}

/// Saved net name of one pad: the quoted text inside its `(net "...")`.
struct PadNetSpan {
    number: String,
    start: usize,
    end: usize,
    name: String,
}

/// Absolute board placement of one Reference field; the footprint stays put.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct FieldPlacement {
    pub footprint_uuid: String,
    pub x: f64,
    pub y: f64,
    pub angle: Option<f64>,
}

const MAX_FIELD_DISTANCE_MM: f64 = 10.0;

pub(super) fn parse_placements(value: &serde_json::Value) -> Result<Vec<FieldPlacement>> {
    let items = value
        .as_array()
        .context("reference_field_placements must be an array")?;
    let mut seen = HashSet::new();
    let mut placements = Vec::new();
    for item in items {
        let number = |key: &str| {
            item[key]
                .as_f64()
                .filter(|v| v.is_finite())
                .with_context(|| format!("placement {key} must be a finite number"))
        };
        let footprint_uuid = item["footprint_uuid"]
            .as_str()
            .context("placement footprint_uuid must be a string")?
            .to_owned();
        anyhow::ensure!(
            seen.insert(footprint_uuid.clone()),
            "Duplicate placement for footprint {footprint_uuid}"
        );
        let angle = match item.get("angle") {
            None | Some(serde_json::Value::Null) => None,
            Some(v) => Some(
                v.as_f64()
                    .filter(|a| a.is_finite())
                    .context("placement angle must be a finite number")?,
            ),
        };
        placements.push(FieldPlacement {
            footprint_uuid,
            x: number("x")?,
            y: number("y")?,
            angle,
        });
    }
    Ok(placements)
}

fn mm(value: f64) -> String {
    let text = format!("{value:.6}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" || text.is_empty() {
        "0".into()
    } else {
        text.into()
    }
}

fn numbers(block: &str) -> Vec<f64> {
    block
        .trim_matches(|c| c == '(' || c == ')')
        .split_whitespace()
        .skip(1)
        .filter_map(|t| t.parse().ok())
        .collect()
}

/// Footprint-local `(at ...)` text for an absolute field placement.
fn local_at(origin: (f64, f64, f64), placement: &FieldPlacement, old_angle: Option<f64>) -> String {
    let (dx, dy) = (placement.x - origin.0, placement.y - origin.1);
    let (sin, cos) = origin.2.to_radians().sin_cos();
    let local = (dx * cos - dy * sin, dx * sin + dy * cos);
    match placement.angle.or(old_angle) {
        Some(angle) => format!("(at {} {} {})", mm(local.0), mm(local.1), mm(angle)),
        None => format!("(at {} {})", mm(local.0), mm(local.1)),
    }
}

fn annotated(reference: &str) -> bool {
    reference.chars().last().is_some_and(|c| c.is_ascii_digit())
        && reference
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '#')
}

fn snapshot(source: &str) -> Result<(BoardState, Vec<FootprintSpan>)> {
    let tree = konnect_sexp::parse_sexp(source)?;
    anyhow::ensure!(
        tree.head() == Some("kicad_pcb"),
        "Expected one saved KiCad board"
    );
    let mut footprints = Vec::new();
    let mut spans = Vec::new();
    let mut ids = HashSet::new();
    for (start, end) in find_direct_child_blocks(source, "kicad_pcb") {
        let block = &source[start..end];
        let node = konnect_sexp::parse_sexp(block)?;
        if node.head() != Some("footprint") {
            continue;
        }
        let id = node
            .find_str("uuid")
            .context("Footprint has no UUID")?
            .to_owned();
        anyhow::ensure!(ids.insert(id.clone()), "Duplicate footprint UUID");
        let properties: Vec<_> = find_direct_child_blocks(block, "footprint")
            .into_iter()
            .filter_map(|(a, b)| {
                let property = konnect_sexp::parse_sexp(&block[a..b]).ok()?;
                (property.head() == Some("property")
                    && property.get(1).and_then(SexpNode::as_str) == Some("Reference"))
                .then_some((a, b, property))
            })
            .collect();
        anyhow::ensure!(
            properties.len() == 1,
            "Footprint must have one Reference property"
        );
        let (a, b, property) = &properties[0];
        let old = property
            .get(2)
            .and_then(SexpNode::as_str)
            .context("Reference property has no value")?
            .to_owned();
        anyhow::ensure!(annotated(&old), "Reference must be annotated ASCII");
        let literal = format!("\"{old}\"");
        let offset = block[*a..*b]
            .find(&literal)
            .context("Reference value is not quoted")?;
        let field_block = &block[*a..*b];
        let (at_a, at_b) = find_direct_child_blocks(field_block, "property")
            .into_iter()
            .find(|(x, y)| {
                konnect_sexp::parse_sexp(&field_block[*x..*y]).is_ok_and(|n| n.head() == Some("at"))
            })
            .context("Reference property has no position")?;
        let at_old = field_block[at_a..at_b].to_owned();
        let field_numbers = numbers(&at_old);
        anyhow::ensure!(field_numbers.len() >= 2, "Reference position is malformed");
        let origin = find_direct_child_blocks(block, "footprint")
            .into_iter()
            .map(|(x, y)| &block[x..y])
            .find(|text| konnect_sexp::parse_sexp(text).is_ok_and(|n| n.head() == Some("at")))
            .map(numbers)
            .filter(|n| n.len() >= 2);
        let mut pads = Vec::new();
        for (pa, pb) in find_direct_child_blocks(block, "footprint") {
            let pad_text = &block[pa..pb];
            let Ok(pad) = konnect_sexp::parse_sexp(pad_text) else {
                continue;
            };
            if pad.head() != Some("pad") {
                continue;
            }
            let Some(number) = pad.get(1).and_then(SexpNode::as_str) else {
                continue;
            };
            for (na, nb) in find_direct_child_blocks(pad_text, "pad") {
                let net_text = &pad_text[na..nb];
                if !net_text.starts_with("(net") {
                    continue;
                }
                let (Some(open), Some(close)) = (net_text.find('"'), net_text.rfind('"')) else {
                    continue;
                };
                let name = &net_text[open + 1..close];
                if close <= open || name.contains(['"', '\\']) {
                    continue;
                }
                pads.push(PadNetSpan {
                    number: number.to_owned(),
                    start: start + pa + na + open + 1,
                    end: start + pa + na + close,
                    name: name.to_owned(),
                });
            }
        }
        spans.push(FootprintSpan {
            id: id.clone(),
            symbol_path: node
                .find_str("path")
                .filter(|p| !p.is_empty() && *p != "/")
                .map(str::to_owned),
            pads,
            at_start: start + a + at_a,
            at_end: start + a + at_b,
            origin: origin.map(|o| (o[0], o[1], o.get(2).copied().unwrap_or(0.0))),
            angle: field_numbers.get(2).copied(),
            at_old,
            old: old.clone(),
            value_start: start + a + offset + 1,
            value_end: start + a + offset + 1 + old.len(),
        });
        footprints.push(BoardFootprint {
            kiid: id,
            reference: old,
            value: String::new(),
            footprint_id: String::new(),
            symbol_path: node
                .find_str("path")
                .filter(|p| !p.is_empty() && *p != "/")
                .map(str::to_owned),
            pad_nets: BTreeMap::new(),
            pad_numbers: BTreeSet::new(),
            position: Point { x: 0.0, y: 0.0 },
            rotation: 0.0,
            layer: String::new(),
            locked: false,
            dnp: false,
            not_in_schematic: false,
        });
    }
    anyhow::ensure!(!footprints.is_empty(), "No saved footprints found");
    Ok((
        BoardState {
            footprints,
            routed_nets: BTreeMap::new(),
            bounds: Bounds {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 0.0,
                max_y: 0.0,
            },
        },
        spans,
    ))
}

#[cfg(test)]
pub(super) fn handle(
    board: &Path,
    netlist: &str,
    design: &ExportedDesign,
    hierarchy: usize,
    dry_run: bool,
    revision: Option<&str>,
    placements: &[FieldPlacement],
) -> Result<CallToolResult> {
    handle_with(
        board, netlist, design, hierarchy, dry_run, revision, placements, false,
    )
}

/// One planned pad net rename: byte span of the quoted name plus old and new text.
struct NetEdit {
    start: usize,
    end: usize,
    to: String,
    from: String,
    pad: String,
    footprint: String,
    number: String,
}

/// `unconnected-(REF-REST)` split into its reference and the rest.
fn generated_no_connect(name: &str) -> Option<(&str, &str)> {
    let inner = name.strip_prefix("unconnected-(")?.strip_suffix(')')?;
    inner.split_once('-')
}

/// Planned net-name edits for pads whose saved generated no-connect name still
/// embeds a previous reference. Anything else that differs is a refusal.
fn plan_generated_net_names(
    design: &ExportedDesign,
    spans: &[FootprintSpan],
) -> std::result::Result<Vec<NetEdit>, String> {
    let mut edits = Vec::new();
    for span in spans {
        let Some(component) = span.symbol_path.as_deref().and_then(|path| {
            design
                .components
                .iter()
                .find(|component| component.symbol_path == path)
        }) else {
            continue;
        };
        for pad in &span.pads {
            let Some(expected) = component.pad_nets.get(&pad.number) else {
                continue;
            };
            if *expected == pad.name {
                continue;
            }
            let renamed = match (
                generated_no_connect(&pad.name),
                generated_no_connect(expected),
            ) {
                (Some((old_ref, old_rest)), Some((new_ref, new_rest))) => {
                    old_ref != new_ref
                        && old_rest == new_rest
                        && new_ref == component.reference
                        && annotated(old_ref)
                        && annotated(new_ref)
                }
                _ => false,
            };
            if !renamed {
                return Err(format!(
                    "Pad {} of {} has net {:?} but the schematic gives {:?}; only generated no-connect names that differ by reference can be renamed",
                    pad.number, component.reference, pad.name, expected
                ));
            }
            edits.push(NetEdit {
                start: pad.start,
                end: pad.end,
                to: expected.clone(),
                from: pad.name.clone(),
                pad: format!("{}/{}", component.reference, pad.number),
                footprint: span.id.clone(),
                number: pad.number.clone(),
            });
        }
    }
    // Final names must be unique and must not land on a net the board already uses.
    let renamed_old: HashSet<&str> = edits.iter().map(|e| e.from.as_str()).collect();
    let mut final_names: HashMap<&str, usize> = HashMap::new();
    for span in spans {
        for pad in &span.pads {
            if !renamed_old.contains(pad.name.as_str()) {
                *final_names.entry(pad.name.as_str()).or_default() += 1;
            }
        }
    }
    for edit in &edits {
        *final_names.entry(edit.to.as_str()).or_default() += 1;
    }
    if let Some((name, _)) = final_names.iter().find(|(name, n)| {
        **n > 1 && name.starts_with("unconnected-(") && edits.iter().any(|e| e.to == **name)
    }) {
        return Err(format!(
            "Net name {name:?} would be used by more than one pad"
        ));
    }
    Ok(edits)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_with(
    board: &Path,
    netlist: &str,
    design: &ExportedDesign,
    hierarchy: usize,
    dry_run: bool,
    revision: Option<&str>,
    placements: &[FieldPlacement],
    rename_nets: bool,
) -> Result<CallToolResult> {
    if kicad_editor_lock_path(board).is_some_and(|p| p.exists()) {
        return Ok(conflict_result(
            "Reference-only saved-board migration requires the PCB closed in KiCad".into(),
        ));
    }
    let before = read_consistent(board)?;
    let (state, spans) = snapshot(&before)?;
    let mut plan = plan_reference_sync(netlist, design, &state);
    for change in &plan.changes {
        if let PlannedChange::Rename { reference, .. } = change {
            if !annotated(reference) {
                return Ok(conflict_result(
                    "Target reference must be annotated ASCII".into(),
                ));
            }
        }
    }
    // Placement edits: target `(at ...)` text per footprint, skipping no-ops.
    let mut placement_edits = Vec::new();
    let mut placement_report = Vec::new();
    for placement in placements {
        let Some(span) = spans.iter().find(|s| s.id == placement.footprint_uuid) else {
            return Ok(conflict_result(format!(
                "Placement names unknown footprint {}",
                placement.footprint_uuid
            )));
        };
        let Some(origin) = span.origin else {
            return Ok(conflict_result(format!(
                "Footprint {} has no saved position",
                span.old
            )));
        };
        let distance = (placement.x - origin.0).hypot(placement.y - origin.1);
        if distance > MAX_FIELD_DISTANCE_MM {
            return Ok(conflict_result(format!(
                "Placement for {} is {distance:.2} mm from its footprint (limit {MAX_FIELD_DISTANCE_MM} mm)",
                span.old
            )));
        }
        let target = local_at(origin, placement, span.angle);
        if numbers(&target) != numbers(&span.at_old) {
            placement_report.push(serde_json::json!({
                "footprint_uuid": span.id, "reference": span.old,
                "from": span.at_old, "to": target,
            }));
            placement_edits.push((span.id.clone(), span.at_start, span.at_end, target));
        }
    }
    let mut net_edits = Vec::new();
    if rename_nets {
        match plan_generated_net_names(design, &spans) {
            Ok(edits) => net_edits = edits,
            Err(message) => return Ok(conflict_result(message)),
        }
        for edit in &net_edits {
            placement_report.push(serde_json::json!({
                "kind": "generated_net_name", "pad": edit.pad, "from": edit.from, "to": edit.to,
            }));
        }
    }
    let mut hash = Sha256::new();
    hash.update(plan.plan_revision.as_bytes());
    hash.update(before.as_bytes());
    for placement in placements {
        hash.update(format!("{placement:?}").as_bytes());
    }
    hash.update(format!("rename_nets={rename_nets}").as_bytes());
    plan.plan_revision = format!("{:x}", hash.finalize());
    let status = if plan.status == PlanStatus::Conflict {
        "conflict"
    } else if plan.changes.is_empty() && placement_edits.is_empty() && net_edits.is_empty() {
        "noop"
    } else {
        "ready"
    };
    if dry_run || status == "conflict" {
        return response(&plan, status, hierarchy, false, &placement_report);
    }
    if revision != Some(plan.plan_revision.as_str()) {
        return Ok(conflict_result(
            "Saved board or schematic changed; rerun the reference-only dry run".into(),
        ));
    }
    if status == "noop" {
        return response(&plan, "noop", hierarchy, false, &placement_report);
    }
    let mut edits = Vec::new();
    for change in &plan.changes {
        let PlannedChange::Rename { kiid, reference } = change else {
            bail!("Unexpected non-reference change")
        };
        anyhow::ensure!(
            annotated(reference),
            "Target reference must be annotated ASCII"
        );
        let span = spans
            .iter()
            .find(|s| &s.id == kiid)
            .context("Missing footprint source span")?;
        anyhow::ensure!(
            before[span.value_start..span.value_end] == span.old,
            "Stale reference span"
        );
        edits.push(SexpEdit::replace(
            span.value_start,
            span.value_end,
            reference.clone(),
        ));
    }
    for (_, start, end, target) in &placement_edits {
        edits.push(SexpEdit::replace(*start, *end, target.clone()));
    }
    for edit in &net_edits {
        anyhow::ensure!(
            before[edit.start..edit.end] == edit.from,
            "Stale pad net span"
        );
        edits.push(SexpEdit::replace(edit.start, edit.end, edit.to.clone()));
    }
    let replacement = apply_edits(before.clone(), edits);
    let (expected, expected_spans) = snapshot(&replacement)?;
    verify_reference_state(&state, &expected, &plan.changes)?;
    verify_placements(&expected_spans, &placement_edits)?;
    verify_net_names(&expected_spans, &net_edits)?;
    write_atomic_if_unchanged(board, &before, &replacement)?;
    let saved = read_consistent(board)?;
    anyhow::ensure!(
        saved == replacement,
        "Reference migration was written but readback differs; inspect before retrying"
    );
    let (observed, observed_spans) = snapshot(&saved)?;
    verify_reference_state(&state, &observed, &plan.changes)?;
    verify_placements(&observed_spans, &placement_edits)?;
    verify_net_names(&observed_spans, &net_edits)?;
    plan.counts.updated.applied = plan.counts.updated.planned;
    plan.counts.board_only_preserved.applied = plan.counts.board_only_preserved.planned;
    response(&plan, "applied", hierarchy, true, &placement_report)
}

fn verify_net_names(spans: &[FootprintSpan], edits: &[NetEdit]) -> Result<()> {
    for edit in edits {
        let name = spans
            .iter()
            .find(|span| span.id == edit.footprint)
            .and_then(|span| span.pads.iter().find(|pad| pad.number == edit.number))
            .map(|pad| pad.name.as_str());
        anyhow::ensure!(
            name == Some(edit.to.as_str()),
            "Generated net name for {} did not read back as planned",
            edit.pad
        );
    }
    Ok(())
}

fn verify_placements(
    spans: &[FootprintSpan],
    edits: &[(String, usize, usize, String)],
) -> Result<()> {
    for (id, _, _, target) in edits {
        let span = spans
            .iter()
            .find(|s| &s.id == id)
            .context("Placed footprint missing after edit")?;
        anyhow::ensure!(
            numbers(&span.at_old) == numbers(target),
            "Reference placement for {} did not read back as planned",
            span.old
        );
    }
    Ok(())
}

fn response(
    plan: &SyncPlan,
    status: &str,
    hierarchy: usize,
    applied: bool,
    placements: &[serde_json::Value],
) -> Result<CallToolResult> {
    let result = sync_response(plan, status, hierarchy, applied);
    let ToolContent::Text { text } = &result.content[0] else {
        bail!("Expected JSON sync response")
    };
    let mut value: serde_json::Value = serde_json::from_str(text)?;
    value["reference_field_placements"] = serde_json::json!(placements);
    value["coverage"]["transport"] = serde_json::json!("saved_board_file");
    value["coverage"]["atomicity"] = serde_json::json!("revision_checked_atomic_file_replace");
    value["undo"] = serde_json::json!({"available":false,"reason":"closed-board reference-only file transaction"});
    Ok(CallToolResult::json(&value))
}

#[cfg(test)]
mod tests {
    use super::*;
    const BOARD: &str = r#"(kicad_pcb (footprint "R" (uuid "a") (path "/a") (property "Reference" "R1" (at 3 4)) (units (unit (name "A") (pins "1" "2")))) (footprint "R" (uuid "b") (path "/b") (property "Reference" "R2" (at 5 6)) (future_token "untouched")))"#;
    fn design() -> ExportedDesign {
        ExportedDesign {
            components: vec![("R2", "/a"), ("R1", "/b")]
                .into_iter()
                .map(|(reference, path)| DesignComponent {
                    reference: reference.into(),
                    symbol_path: path.into(),
                    value: "ignored".into(),
                    footprint_id: "ignored".into(),
                    dnp: true,
                    pad_nets: BTreeMap::new(),
                })
                .collect(),
            skipped: vec![],
            unassigned: vec![],
        }
    }
    fn body(result: CallToolResult) -> serde_json::Value {
        let ToolContent::Text { text } = &result.content[0] else {
            panic!()
        };
        serde_json::from_str(text).unwrap()
    }
    #[test]
    fn saved_reference_swap_preserves_all_other_bytes_and_reads_back() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("board.kicad_pcb");
        std::fs::write(&path, BOARD).unwrap();
        let dry = body(handle(&path, "netlist", &design(), 1, true, None, &[]).unwrap());
        let mut invalid = design();
        invalid.components[0].reference = "R?".into();
        assert_eq!(
            body(handle(&path, "netlist", &invalid, 1, true, None, &[]).unwrap())["status"],
            "conflict"
        );
        assert_eq!(dry["status"], "ready");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), BOARD);
        let result = body(
            handle(
                &path,
                "netlist",
                &design(),
                1,
                false,
                dry["plan_revision"].as_str(),
                &[],
            )
            .unwrap(),
        );
        assert_eq!(result["status"], "applied");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            BOARD
                .replace("\"R1\"", "\"TEMP\"")
                .replace("\"R2\"", "\"R1\"")
                .replace("\"TEMP\"", "\"R2\"")
        );
        assert_eq!(result["coverage"]["transport"], "saved_board_file");
    }
    #[test]
    fn saved_reference_migration_refuses_open_and_stale_boards_without_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("board.kicad_pcb");
        std::fs::write(&path, BOARD).unwrap();
        let dry = body(handle(&path, "netlist", &design(), 1, true, None, &[]).unwrap());
        std::fs::write(&path, format!("{BOARD}\n")).unwrap();
        assert_eq!(
            body(
                handle(
                    &path,
                    "netlist",
                    &design(),
                    1,
                    false,
                    dry["plan_revision"].as_str(),
                    &[]
                )
                .unwrap()
            )["status"],
            "conflict"
        );
        let lock = kicad_editor_lock_path(&path).unwrap();
        std::fs::write(lock, "test").unwrap();
        assert_eq!(
            body(handle(&path, "netlist", &design(), 1, true, None, &[]).unwrap())["status"],
            "conflict"
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), format!("{BOARD}\n"));
    }

    fn placement(uuid: &str, x: f64, y: f64) -> FieldPlacement {
        FieldPlacement {
            footprint_uuid: uuid.into(),
            x,
            y,
            angle: None,
        }
    }
    const ROTATED: &str = r#"(kicad_pcb (footprint "R" (at 109.7026 74.757224 135) (uuid "a") (path "/a") (property "Reference" "R1" (at -0.804671 1.817564 0) (layer "F.SilkS") (effects (font (size 0.8 0.8)))) (property "Value" "x" (at 0 1 135))) (footprint "R" (at 130 90 90) (uuid "b") (path "/b") (property "Reference" "R2" (at 2.3 0 90) (layer "B.SilkS"))))"#;
    fn same_references() -> ExportedDesign {
        let mut design = design();
        design.components[0].reference = "R1".into();
        design.components[1].reference = "R2".into();
        design.components[0].symbol_path = "/a".into();
        design.components[1].symbol_path = "/b".into();
        design
    }
    #[test]
    fn field_placement_converts_world_to_local_and_changes_only_that_position() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("board.kicad_pcb");
        std::fs::write(&path, ROTATED).unwrap();
        let placements = [placement("a", 110.0, 75.0), placement("b", 130.0, 87.0)];
        let dry = body(handle(&path, "n", &same_references(), 1, true, None, &placements).unwrap());
        assert_eq!(dry["status"], "ready");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), ROTATED);
        let revision = dry["plan_revision"].as_str();
        let applied = handle(
            &path,
            "n",
            &same_references(),
            1,
            false,
            revision,
            &placements,
        )
        .unwrap();
        assert_eq!(body(applied)["status"], "applied");
        // Offsets computed independently of the implementation; every other byte is unchanged.
        let expected = ROTATED
            .replace("(at -0.804671 1.817564 0)", "(at -0.381962 0.038625 0)")
            .replace("(at 2.3 0 90)", "(at 3 0 90)");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
        let again =
            body(handle(&path, "n", &same_references(), 1, true, None, &placements).unwrap());
        assert_eq!(again["status"], "noop");
    }
    #[test]
    fn field_placement_refuses_unknown_distant_and_duplicate_targets() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("board.kicad_pcb");
        std::fs::write(&path, ROTATED).unwrap();
        for bad in [placement("zzz", 109.0, 74.0), placement("a", 140.0, 74.0)] {
            let result = handle(&path, "n", &same_references(), 1, true, None, &[bad]).unwrap();
            assert_eq!(body(result)["status"], "conflict");
        }
        let duplicate = serde_json::json!([{"footprint_uuid":"a","x":1,"y":2},{"footprint_uuid":"a","x":1,"y":2}]);
        assert!(parse_placements(&duplicate).is_err());
        assert!(
            parse_placements(&serde_json::json!([{"footprint_uuid":"a","x":"1","y":2}])).is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), ROTATED);
    }

    const NET_BOARD: &str = r#"(kicad_pcb (footprint "U" (at 1 2) (uuid "u") (path "/u") (property "Reference" "U301" (at 0 0)) (pad "1" smd rect (at 0 0) (net "GND")) (pad "8" smd rect (at 1 0) (net "unconnected-(U601-A6-Pad8)")) (future_token "untouched")) (footprint "U" (at 5 2) (uuid "v") (path "/v") (property "Reference" "U801" (at 0 0)) (pad "8" smd rect (at 1 0) (net "unconnected-(U701-A6-Pad8)"))))"#;
    fn net_design(rest_ok: bool) -> ExportedDesign {
        let component = |reference: &str, path: &str, nets: &[(&str, &str)]| DesignComponent {
            reference: reference.into(),
            symbol_path: path.into(),
            value: "ignored".into(),
            footprint_id: "ignored".into(),
            dnp: false,
            pad_nets: nets
                .iter()
                .map(|(p, n)| (p.to_string(), n.to_string()))
                .collect(),
        };
        let second = if rest_ok {
            "unconnected-(U801-A6-Pad8)"
        } else {
            "unconnected-(U801-A7-Pad8)"
        };
        ExportedDesign {
            components: vec![
                component(
                    "U301",
                    "/u",
                    &[("1", "GND"), ("8", "unconnected-(U301-A6-Pad8)")],
                ),
                component("U801", "/v", &[("8", second)]),
            ],
            skipped: vec![],
            unassigned: vec![],
        }
    }
    #[test]
    fn generated_no_connect_names_follow_the_reference_and_nothing_else_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("board.kicad_pcb");
        std::fs::write(&path, NET_BOARD).unwrap();
        let dry =
            body(handle_with(&path, "n", &net_design(true), 1, true, None, &[], true).unwrap());
        assert_eq!(dry["status"], "ready");
        assert_eq!(
            dry["reference_field_placements"].as_array().unwrap().len(),
            2
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), NET_BOARD);
        let revision = dry["plan_revision"].as_str();
        let done =
            handle_with(&path, "n", &net_design(true), 1, false, revision, &[], true).unwrap();
        assert_eq!(body(done)["status"], "applied");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            NET_BOARD
                .replace("unconnected-(U601-A6-Pad8)", "unconnected-(U301-A6-Pad8)")
                .replace("unconnected-(U701-A6-Pad8)", "unconnected-(U801-A6-Pad8)")
        );
        let again =
            body(handle_with(&path, "n", &net_design(true), 1, true, None, &[], true).unwrap());
        assert_eq!(again["status"], "noop");
    }
    #[test]
    fn net_rename_refuses_unrelated_differences_and_does_nothing_by_default() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("board.kicad_pcb");
        std::fs::write(&path, NET_BOARD).unwrap();
        let refused =
            handle_with(&path, "n", &net_design(false), 1, true, None, &[], true).unwrap();
        assert_eq!(body(refused)["status"], "conflict");
        let off =
            body(handle_with(&path, "n", &net_design(true), 1, true, None, &[], false).unwrap());
        assert_eq!(off["status"], "noop");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), NET_BOARD);
    }

    #[test]
    fn generated_no_connect_names_can_swap_between_pads() {
        let board = NET_BOARD
            .replace("unconnected-(U601-A6-Pad8)", "unconnected-(U801-A6-Pad8)")
            .replace("unconnected-(U701-A6-Pad8)", "unconnected-(U301-A6-Pad8)");
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("board.kicad_pcb");
        std::fs::write(&path, &board).unwrap();
        let design = net_design(true);
        let dry = body(handle_with(&path, "n", &design, 1, true, None, &[], true).unwrap());
        assert_eq!(dry["status"], "ready");
        let revision = dry["plan_revision"].as_str();
        let done = handle_with(&path, "n", &design, 1, false, revision, &[], true).unwrap();
        assert_eq!(body(done)["status"], "applied");
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains(
            r#"(pad "8" smd rect (at 1 0) (net "unconnected-(U301-A6-Pad8)")) (future_token"#
        ));
        assert!(
            saved.contains(r#"(pad "8" smd rect (at 1 0) (net "unconnected-(U801-A6-Pad8)"))))"#)
        );
    }
}
