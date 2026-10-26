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
        spans.push(FootprintSpan {
            id: id.clone(),
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

pub(super) fn handle(
    board: &Path,
    netlist: &str,
    design: &ExportedDesign,
    hierarchy: usize,
    dry_run: bool,
    revision: Option<&str>,
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
    let mut hash = Sha256::new();
    hash.update(plan.plan_revision.as_bytes());
    hash.update(before.as_bytes());
    plan.plan_revision = format!("{:x}", hash.finalize());
    let status = if plan.status == PlanStatus::Conflict {
        "conflict"
    } else if plan.changes.is_empty() {
        "noop"
    } else {
        "ready"
    };
    if dry_run || status == "conflict" {
        return response(&plan, status, hierarchy, false);
    }
    if revision != Some(plan.plan_revision.as_str()) {
        return Ok(conflict_result(
            "Saved board or schematic changed; rerun the reference-only dry run".into(),
        ));
    }
    if status == "noop" {
        return response(&plan, "noop", hierarchy, false);
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
    let replacement = apply_edits(before.clone(), edits);
    let (expected, _) = snapshot(&replacement)?;
    verify_reference_state(&state, &expected, &plan.changes)?;
    write_atomic_if_unchanged(board, &before, &replacement)?;
    let saved = read_consistent(board)?;
    anyhow::ensure!(
        saved == replacement,
        "Reference migration was written but readback differs; inspect before retrying"
    );
    let (observed, _) = snapshot(&saved)?;
    verify_reference_state(&state, &observed, &plan.changes)?;
    plan.counts.updated.applied = plan.counts.updated.planned;
    plan.counts.board_only_preserved.applied = plan.counts.board_only_preserved.planned;
    response(&plan, "applied", hierarchy, true)
}

fn response(
    plan: &SyncPlan,
    status: &str,
    hierarchy: usize,
    applied: bool,
) -> Result<CallToolResult> {
    let result = sync_response(plan, status, hierarchy, applied);
    let ToolContent::Text { text } = &result.content[0] else {
        bail!("Expected JSON sync response")
    };
    let mut value: serde_json::Value = serde_json::from_str(text)?;
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
        let dry = body(handle(&path, "netlist", &design(), 1, true, None).unwrap());
        let mut invalid = design();
        invalid.components[0].reference = "R?".into();
        assert_eq!(
            body(handle(&path, "netlist", &invalid, 1, true, None).unwrap())["status"],
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
        let dry = body(handle(&path, "netlist", &design(), 1, true, None).unwrap());
        std::fs::write(&path, format!("{BOARD}\n")).unwrap();
        assert_eq!(
            body(
                handle(
                    &path,
                    "netlist",
                    &design(),
                    1,
                    false,
                    dry["plan_revision"].as_str()
                )
                .unwrap()
            )["status"],
            "conflict"
        );
        let lock = kicad_editor_lock_path(&path).unwrap();
        std::fs::write(lock, "test").unwrap();
        assert_eq!(
            body(handle(&path, "netlist", &design(), 1, true, None).unwrap())["status"],
            "conflict"
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), format!("{BOARD}\n"));
    }
}
