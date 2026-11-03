use crate::mcp::protocol::CallToolResult;
use crate::tool;
use crate::tools::{
    pcb_board::{attempt_ipc_write, BoardWrite},
    ToolContext, ToolDef,
};
use anyhow::{bail, Context, Result};
use konnect_ipc::gen::kiapi;
use prost::Message;
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone)]
struct LibraryFootprint {
    library_id: String,
    definition: kiapi::board::types::Footprint,
    attributes: kiapi::board::types::FootprintAttributes,
    datasheet: Option<String>,
    description_field: Option<String>,
    properties: Vec<kiapi::board::types::Field>,
    pads: Vec<konnect_ipc::IpcPadDefinition>,
    pad_overrides: Vec<PadOverrides>,
    graphics: Vec<konnect_ipc::IpcGraphicDefinition>,
    models: Vec<kiapi::board::types::Footprint3DModel>,
}

/// Optional messages retain the distinction between omitted and explicit zero.
#[derive(Debug, Clone, Default)]
struct PadOverrides {
    mask: Option<kiapi::common::types::Distance>,
    paste: Option<kiapi::common::types::Distance>,
    paste_ratio: Option<kiapi::common::types::Ratio>,
    clearance: Option<kiapi::common::types::Distance>,
    die_length: Option<kiapi::common::types::Distance>,
    zone: Option<i32>,
    thermal_width: Option<kiapi::common::types::Distance>,
    thermal_gap: Option<kiapi::common::types::Distance>,
    thermal_angle: Option<kiapi::common::types::Angle>,
}

impl PadOverrides {
    fn parse(pad: &konnect_sexp::SexpNode) -> Result<Self> {
        let mut settings = Self::default();
        let mut seen = BTreeSet::new();
        for child in pad.children().unwrap_or_default().iter().skip(4) {
            let Some(tag) = child.head() else { continue };
            if !matches!(
                tag,
                "solder_mask_margin"
                    | "solder_paste_margin"
                    | "solder_paste_margin_ratio"
                    | "clearance"
                    | "die_length"
                    | "zone_connect"
                    | "thermal_bridge_width"
                    | "thermal_gap"
                    | "thermal_bridge_angle"
            ) {
                continue;
            }
            if !seen.insert(tag) {
                bail!("duplicate pad clause '{tag}'");
            }
            if child.children().map_or(0, |children| children.len()) != 2 {
                bail!("pad clause '{tag}' requires exactly one numeric value");
            }
            let value = child
                .get(1)
                .and_then(|node| node.as_str()?.parse::<f64>().ok())
                .filter(|value| value.is_finite())
                .with_context(|| format!("pad clause '{tag}' requires a finite number"))?;
            let distance = || -> Result<kiapi::common::types::Distance> {
                let nm = value * 1_000_000.0;
                if nm < i64::MIN as f64 || nm >= i64::MAX as f64 {
                    bail!("pad clause '{tag}' exceeds the typed distance range");
                }
                if matches!(
                    tag,
                    "clearance" | "die_length" | "thermal_bridge_width" | "thermal_gap"
                ) && value < 0.0
                {
                    bail!("pad clause '{tag}' must be non-negative");
                }
                Ok(konnect_ipc::builders::distance(value))
            };
            match tag {
                "solder_mask_margin" => settings.mask = Some(distance()?),
                "solder_paste_margin" => settings.paste = Some(distance()?),
                "solder_paste_margin_ratio" => {
                    if !(-1.0..=1.0).contains(&value) {
                        bail!("pad clause '{tag}' must be between -1 and 1");
                    }
                    settings.paste_ratio = Some(kiapi::common::types::Ratio { value });
                }
                "clearance" => settings.clearance = Some(distance()?),
                "die_length" => settings.die_length = Some(distance()?),
                "thermal_bridge_width" => settings.thermal_width = Some(distance()?),
                "thermal_gap" => settings.thermal_gap = Some(distance()?),
                "thermal_bridge_angle" => {
                    settings.thermal_angle = Some(kiapi::common::types::Angle {
                        value_degrees: value,
                    })
                }
                "zone_connect" => {
                    // KiCad 10's file tokens are its ZONE_CONNECTION values (observed with
                    // pcbnew 10.0.6): 0 none, 1 thermal, 2 solid/full, 3 thermal for
                    // plated-through holes. "Inherited" is the absent token, never a number.
                    settings.zone = Some(match value {
                        0.0 => kiapi::board::types::ZoneConnectionStyle::ZcsNone as i32,
                        1.0 => kiapi::board::types::ZoneConnectionStyle::ZcsThermal as i32,
                        2.0 => kiapi::board::types::ZoneConnectionStyle::ZcsFull as i32,
                        3.0 => kiapi::board::types::ZoneConnectionStyle::ZcsPthThermal as i32,
                        _ => bail!("pad clause 'zone_connect' must be an integer from 0 to 3"),
                    })
                }
                _ => unreachable!(),
            }
        }
        Ok(settings)
    }

    fn apply(&self, pad: &mut kiapi::board::types::Pad) -> Result<()> {
        pad.copper_clearance_override = self.clearance.clone();
        pad.pad_to_die_length = self.die_length.clone();
        let stack = pad
            .pad_stack
            .as_mut()
            .context("typed pad has no pad stack")?;
        if self.mask.is_some() || self.paste.is_some() || self.paste_ratio.is_some() {
            for outer in [&mut stack.front_outer_layers, &mut stack.back_outer_layers] {
                let outer = outer.get_or_insert_with(Default::default);
                if self.mask.is_some() {
                    outer.solder_mask_settings = Some(kiapi::board::types::SolderMaskOverrides {
                        solder_mask_margin: self.mask.clone(),
                    });
                }
                if self.paste.is_some() || self.paste_ratio.is_some() {
                    outer.solder_paste_settings = Some(kiapi::board::types::SolderPasteOverrides {
                        solder_paste_margin: self.paste.clone(),
                        solder_paste_margin_ratio: self.paste_ratio.clone(),
                    });
                }
            }
        }
        if self.zone.is_some()
            || self.thermal_width.is_some()
            || self.thermal_gap.is_some()
            || self.thermal_angle.is_some()
        {
            stack.zone_settings = Some(kiapi::board::types::ZoneConnectionSettings {
                zone_connection: self
                    .zone
                    .unwrap_or(kiapi::board::types::ZoneConnectionStyle::ZcsInherited as i32),
                thermal_spokes: if self.thermal_width.is_some()
                    || self.thermal_gap.is_some()
                    || self.thermal_angle.is_some()
                {
                    Some(kiapi::board::types::ThermalSpokeSettings {
                        width: self.thermal_width.clone(),
                        gap: self.thermal_gap.clone(),
                        angle: self.thermal_angle.clone(),
                    })
                } else {
                    None
                },
            });
        }
        Ok(())
    }
}

#[derive(Debug)]
struct ParsedLibraryProperties {
    datasheet: Option<String>,
    description: Option<String>,
    custom: Vec<kiapi::board::types::Field>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
enum ChangedDomain {
    Pads,
    Graphics,
    Attributes,
    Metadata,
    Models,
}

#[derive(Debug)]
struct PreparedUpdate {
    item: prost_types::Any,
    changed_domains: BTreeSet<ChangedDomain>,
    preserved: PreservedState,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
struct UpdateFilters {
    references: Option<BTreeSet<String>>,
    library_ids: Option<BTreeSet<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArgumentError {
    field: String,
    reason: String,
}

impl std::fmt::Display for ArgumentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.field, self.reason)
    }
}

impl std::error::Error for ArgumentError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PlanStatus {
    Ready,
    Noop,
    Conflict,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
struct CountPair {
    planned: usize,
    applied: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
struct UpdateCoverage {
    selected: CountPair,
    changed: CountPair,
    unchanged: CountPair,
    skipped_unlinked: CountPair,
    conflicts: CountPair,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct PreservedState {
    position: bool,
    rotation: bool,
    layer: bool,
    locked: bool,
    kiid: bool,
    symbol_path: bool,
    pad_nets: bool,
    instance_overrides: bool,
}

impl PreservedState {
    /// Derived by comparing the rebuilt instance against the one read from
    /// the board — never asserted from policy. If a future change to
    /// `build_updated_instance` stops carrying one of these across, the
    /// response says so instead of echoing the intent.
    fn derive(
        current: &kiapi::board::types::FootprintInstance,
        updated: &kiapi::board::types::FootprintInstance,
        old_nets: &BTreeMap<String, kiapi::board::types::Net>,
    ) -> Self {
        let new_nets: BTreeMap<String, String> = updated
            .definition
            .iter()
            .flat_map(|definition| definition.items.iter())
            .filter(|item| item.type_url.ends_with("kiapi.board.types.Pad"))
            .filter_map(|item| kiapi::board::types::Pad::decode(item.value.as_slice()).ok())
            .filter_map(|pad| pad.net.map(|net| (pad.number, net.name)))
            .collect();
        Self {
            position: updated.position == current.position,
            rotation: updated.orientation == current.orientation,
            layer: updated.layer == current.layer,
            locked: updated.locked == current.locked,
            kiid: updated.id == current.id,
            symbol_path: updated.symbol_path == current.symbol_path
                && updated.symbol_sheet_name == current.symbol_sheet_name
                && updated.symbol_sheet_filename == current.symbol_sheet_filename,
            pad_nets: old_nets.iter().all(|(number, net)| {
                new_nets.get(number).map(String::as_str) == Some(net.name.as_str())
            }),
            instance_overrides: updated.overrides == current.overrides,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct PlannedUpdate {
    reference: String,
    library_id: String,
    changed_domains: BTreeSet<ChangedDomain>,
    preserved: PreservedState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct UpdateDiagnostic {
    code: String,
    message: String,
    reference: Option<String>,
}

#[derive(Debug)]
struct UpdatePlan {
    status: PlanStatus,
    plan_revision: String,
    coverage: UpdateCoverage,
    changes: Vec<PlannedUpdate>,
    diagnostics: Vec<UpdateDiagnostic>,
    prepared_items: Vec<prost_types::Any>,
}

pub(crate) fn tool() -> ToolDef {
    tool!(
        "update_footprints_from_library",
        "Plan or atomically apply KiCad's Update Footprints from Library operation to placed \
         footprints on the live board. Defaults to a non-mutating dry run; apply requires its \
         exact plan revision. Preserves placed-instance state and pad nets while refreshing \
         supported library-owned content.",
        json!({
            "type": "object",
            "properties": {
                "board": {
                    "type": "string",
                    "description": "Open .kicad_pcb path whose placed footprints will be refreshed"
                },
                "references": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional exact reference allowlist; omitted means all eligible footprints"
                },
                "library_ids": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional exact Library:Footprint allowlist"
                },
                "dry_run": {
                    "type": "boolean",
                    "default": true,
                    "description": "Plan and report changes without mutating the board"
                },
                "expected_plan_revision": {
                    "type": "string",
                    "description": "Required for apply; exact revision returned by the reviewed dry run"
                }
            },
            "required": ["board"],
            "additionalProperties": false
        }),
        |args, ctx| async move { handle_update_footprints_from_library(args, ctx).await }
    )
    .with_board_access(crate::tools::BoardAccess::LiveOnly)
}

pub(crate) async fn handle_update_footprints_from_library(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> Result<CallToolResult> {
    use kiapi::common::types::KiCadObjectType as ObjectType;

    let board_path = crate::tools::get_path(args, "board")?;
    let dry_run = args["dry_run"].as_bool().unwrap_or(true);
    let expected_revision = args["expected_plan_revision"].as_str().map(str::to_string);
    if !dry_run && expected_revision.is_none() {
        return Ok(CallToolResult::error_kind(
            crate::mcp::error::ToolErrorKind::InvalidArgument {
                field: "expected_plan_revision".to_string(),
                reason: "required when dry_run is false".to_string(),
            },
            "Apply requires the plan revision returned by a current dry run.",
        ));
    }
    if !board_path.is_file() {
        return Ok(CallToolResult::error_kind(
            crate::mcp::error::ToolErrorKind::FileNotFound {
                path: board_path.display().to_string(),
            },
            format!("Board file not found: {}", board_path.display()),
        ));
    }
    let filters = match parse_filters(args) {
        Ok(filters) => filters,
        Err(error) => {
            return Ok(CallToolResult::error_kind(
                crate::mcp::error::ToolErrorKind::InvalidArgument {
                    field: error.field.clone(),
                    reason: error.reason.clone(),
                },
                format!("Argument '{}' is invalid: {}", error.field, error.reason),
            ))
        }
    };

    let ipc_board_path = board_path.clone();
    let planning_board_path = board_path.clone();
    let operation = if dry_run {
        "footprint library update dry run"
    } else {
        "footprint library update apply"
    };
    let result = attempt_ipc_write(
        ctx,
        &board_path,
        operation,
        move |client| {
            let document = client.find_open_board(&ipc_board_path)?;
            let footprint_items =
                client.get_items_in(document.clone(), ObjectType::KotPcbFootprint)?;
            let nets = client.get_nets_in(document.clone())?;
            let net_codes = nets
                .into_iter()
                .map(|net| (net.name, net.netcode))
                .collect::<BTreeMap<_, _>>();
            let routed_nets = snapshot_routed_nets(client, document.clone())?;
            let mut plan = plan_updates(
                &planning_board_path,
                &footprint_items,
                &net_codes,
                &routed_nets,
                &filters,
            );

            if dry_run || plan.status == PlanStatus::Conflict {
                return Ok(plan_response(&plan, false));
            }
            if expected_revision.as_deref() != Some(plan.plan_revision.as_str()) {
                plan.status = PlanStatus::Conflict;
                plan.coverage.conflicts.planned += 1;
                plan.diagnostics.push(UpdateDiagnostic {
                    code: "stale_plan_revision".to_string(),
                    message: "The live board, selected libraries, or filters changed; rerun dry run and apply its new plan revision."
                        .to_string(),
                    reference: None,
                });
                plan.changes.clear();
                plan.prepared_items.clear();
                return Ok(plan_response(&plan, false));
            }
            if plan.status == PlanStatus::Noop {
                return Ok(plan_response(&plan, false));
            }

            let update_count = plan.prepared_items.len();
            let expected_items = std::mem::take(&mut plan.prepared_items);
            client.run_commit("Update footprints from libraries", |client| {
                // Acquire the native undo scope before the final stale check.
                let latest_items = client.get_items_in(document.clone(), ObjectType::KotPcbFootprint)?;
                let latest_nets = client.get_nets_in(document.clone())?.into_iter().map(|net| (net.name, net.netcode)).collect();
                let latest_routed = snapshot_routed_nets(client, document.clone())?;
                let latest = plan_updates(&planning_board_path, &latest_items, &latest_nets, &latest_routed, &filters);
                if latest.status == PlanStatus::Conflict || latest.plan_revision != plan.plan_revision {
                    bail!("stale footprint plan immediately before mutation; no update sent; inspect and replan");
                }
                client.update_items_in(document.clone(), expected_items.clone())?;
                let observed = client.get_items_in(document.clone(), ObjectType::KotPcbFootprint)?;
                verify_updated_items(&expected_items, &observed)
            })?;
            plan.coverage.selected.applied = plan.coverage.selected.planned;
            plan.coverage.changed.applied = update_count;
            plan.coverage.unchanged.applied = plan.coverage.unchanged.planned;
            plan.coverage.skipped_unlinked.applied = plan.coverage.skipped_unlinked.planned;
            Ok(plan_response(&plan, true))
        },
    )
    .await?;

    Ok(match result {
        BoardWrite::Ipc(response) => response,
        BoardWrite::File(reason) => preflight_conflict(format!(
            "{} update_footprints_from_library is live-IPC-only and never edits the board \
             file directly. Open the requested board in KiCad and retry.",
            reason.premise()
        )),
        BoardWrite::Refused(result) => result,
    })
}

fn verify_updated_items(
    expected: &[prost_types::Any],
    observed: &[prost_types::Any],
) -> Result<()> {
    let observed = observed
        .iter()
        .map(|item| kiapi::board::types::FootprintInstance::decode(item.value.as_slice()))
        .collect::<Result<Vec<_>, _>>()
        .context("invalid footprint readback")?;
    for item in expected {
        let expected = kiapi::board::types::FootprintInstance::decode(item.value.as_slice())?;
        let id = expected
            .id
            .as_ref()
            .context("updated footprint has no identity")?;
        let matches = observed
            .iter()
            .filter(|fp| fp.id.as_ref() == Some(id))
            .collect::<Vec<_>>();
        let [actual] = matches.as_slice() else {
            bail!(
                "readback must contain exactly one footprint with identity {}",
                id.value
            );
        };
        let differing_domains = changed_domains(&expected, actual)?;
        let instance_differs = !instance_state_matches(&expected, actual);
        if instance_differs || !differing_domains.is_empty() {
            bail!(
                "independent readback differs for footprint {} (instance state differs: {}; library-owned domains differing: {:?}; first difference: {}); native transaction must be dropped",
                field_text(&expected.reference_field),
                instance_differs,
                differing_domains,
                readback_difference_summary(&expected, actual)
            );
        }
        let nets = |fp: &kiapi::board::types::FootprintInstance| -> Result<Vec<(String, Vec<u8>)>> {
            let mut nets = fp
                .definition
                .as_ref()
                .context("readback has no definition")?
                .items
                .iter()
                .filter(|item| item.type_url.ends_with("kiapi.board.types.Pad"))
                .map(|item| {
                    let pad = kiapi::board::types::Pad::decode(item.value.as_slice())?;
                    Ok((
                        pad.number,
                        pad.net.map(|net| net.encode_to_vec()).unwrap_or_default(),
                    ))
                })
                .collect::<Result<Vec<_>>>()?;
            nets.sort();
            Ok(nets)
        };
        if nets(&expected)? != nets(actual)? {
            bail!(
                "pad-net readback differs for footprint {}",
                field_text(&expected.reference_field)
            );
        }
    }
    Ok(())
}

fn snapshot_routed_nets(
    client: &konnect_ipc::KiCadIpcClient,
    document: kiapi::common::types::DocumentSpecifier,
) -> Result<BTreeSet<String>> {
    use kiapi::common::types::KiCadObjectType as ObjectType;

    let mut routed = BTreeSet::new();
    for item in client.get_items_in(document.clone(), ObjectType::KotPcbTrace)? {
        if let Ok(track) = kiapi::board::types::Track::decode(item.value.as_slice()) {
            if let Some(net) = track.net.filter(|net| !net.name.is_empty()) {
                routed.insert(net.name);
            }
        }
    }
    for item in client.get_items_in(document.clone(), ObjectType::KotPcbArc)? {
        if let Ok(arc) = kiapi::board::types::Arc::decode(item.value.as_slice()) {
            if let Some(net) = arc.net.filter(|net| !net.name.is_empty()) {
                routed.insert(net.name);
            }
        }
    }
    for item in client.get_items_in(document.clone(), ObjectType::KotPcbVia)? {
        if let Ok(via) = kiapi::board::types::Via::decode(item.value.as_slice()) {
            if let Some(net) = via.net.filter(|net| !net.name.is_empty()) {
                routed.insert(net.name);
            }
        }
    }
    if !client
        .get_items_in(document.clone(), ObjectType::KotPcbZone)?
        .is_empty()
    {
        routed.extend(
            client
                .get_nets_in(document)?
                .into_iter()
                .filter(|net| !net.name.is_empty())
                .map(|net| net.name),
        );
    }
    Ok(routed)
}

fn plan_response(plan: &UpdatePlan, applied: bool) -> CallToolResult {
    CallToolResult::json(&json!({
        "status": if applied {
            "applied"
        } else {
            match plan.status {
                PlanStatus::Ready => "ready",
                PlanStatus::Noop => "noop",
                PlanStatus::Conflict => "conflict",
            }
        },
        "plan_revision": plan.plan_revision,
        "coverage": {
            "transport": "live_kicad_ipc",
            "atomicity": "single_kicad_undo_commit",
            "selected": plan.coverage.selected,
            "changed": plan.coverage.changed,
            "unchanged": plan.coverage.unchanged,
            "skipped_unlinked": plan.coverage.skipped_unlinked,
            "conflicts": plan.coverage.conflicts
        },
        "changes": plan.changes,
        "diagnostics": plan.diagnostics,
        "undo": if applied {
            Some("Ctrl-Z reverses the whole footprint library update.")
        } else {
            None
        }
    }))
}

fn preflight_conflict(message: impl Into<String>) -> CallToolResult {
    CallToolResult::json(&json!({
        "status": "conflict",
        "coverage": {
            "transport": "live_kicad_ipc",
            "atomicity": "single_kicad_undo_commit",
            "selected": CountPair::default(),
            "changed": CountPair::default(),
            "unchanged": CountPair::default(),
            "skipped_unlinked": CountPair::default(),
            "conflicts": CountPair { planned: 1, applied: 0 }
        },
        "changes": [],
        "diagnostics": [{
            "code": "preflight_conflict",
            "message": message.into()
        }],
        "undo": null
    }))
}

fn parse_filters(args: &serde_json::Value) -> std::result::Result<UpdateFilters, ArgumentError> {
    fn parse(
        args: &serde_json::Value,
        field: &str,
    ) -> std::result::Result<Option<BTreeSet<String>>, ArgumentError> {
        let Some(value) = args.get(field) else {
            return Ok(None);
        };
        let values = value.as_array().ok_or_else(|| ArgumentError {
            field: field.to_string(),
            reason: "must be an array of strings when supplied".to_string(),
        })?;
        let parsed = values
            .iter()
            .map(|value| {
                let value = value.as_str().ok_or_else(|| ArgumentError {
                    field: field.to_string(),
                    reason: "must contain only strings".to_string(),
                })?;
                if value.is_empty() {
                    return Err(ArgumentError {
                        field: field.to_string(),
                        reason: "must not contain empty strings".to_string(),
                    });
                }
                Ok(value.to_string())
            })
            .collect::<std::result::Result<BTreeSet<_>, _>>()
            .map(Some)?;
        if field == "library_ids" {
            if let Some(invalid) = parsed.as_ref().and_then(|values| {
                values.iter().find(|value| {
                    value.split_once(':').is_none_or(|(nickname, entry)| {
                        nickname.is_empty()
                            || entry.is_empty()
                            || value.contains('/')
                            || value.contains('\\')
                    })
                })
            }) {
                return Err(ArgumentError {
                    field: field.to_string(),
                    reason: format!(
                        "'{invalid}' must use a non-empty Library:Footprint identifier"
                    ),
                });
            }
        }
        Ok(parsed)
    }

    Ok(UpdateFilters {
        references: parse(args, "references")?,
        library_ids: parse(args, "library_ids")?,
    })
}

fn plan_updates(
    board_path: &Path,
    footprint_items: &[prost_types::Any],
    net_codes: &BTreeMap<String, i32>,
    routed_nets: &BTreeSet<String>,
    filters: &UpdateFilters,
) -> UpdatePlan {
    #[derive(Debug)]
    struct Candidate {
        reference: String,
        library_id: String,
        item: prost_types::Any,
        instance: kiapi::board::types::FootprintInstance,
    }

    let mut coverage = UpdateCoverage::default();
    let mut diagnostics = Vec::new();
    let mut candidates = Vec::new();
    let mut reference_counts = BTreeMap::<String, usize>::new();

    for item in footprint_items {
        let instance = match kiapi::board::types::FootprintInstance::decode(item.value.as_slice()) {
            Ok(instance) => instance,
            Err(error) => {
                diagnostics.push(UpdateDiagnostic {
                    code: "invalid_board_footprint".to_string(),
                    message: format!("KiCad returned an invalid footprint: {error}"),
                    reference: None,
                });
                continue;
            }
        };
        let reference = field_text(&instance.reference_field);
        *reference_counts.entry(reference.clone()).or_insert(0) += 1;
        let library_id = instance
            .definition
            .as_ref()
            .and_then(|definition| definition.id.as_ref())
            .map(|id| format!("{}:{}", id.library_nickname, id.entry_name))
            .unwrap_or_default();
        candidates.push(Candidate {
            reference,
            library_id,
            item: item.clone(),
            instance,
        });
    }

    if let Some(references) = filters.references.as_ref() {
        for reference in references {
            match reference_counts.get(reference).copied().unwrap_or(0) {
                0 => diagnostics.push(UpdateDiagnostic {
                    code: "reference_not_found".to_string(),
                    message: format!("footprint reference '{reference}' was not found"),
                    reference: Some(reference.clone()),
                }),
                count if count > 1 => diagnostics.push(UpdateDiagnostic {
                    code: "duplicate_reference".to_string(),
                    message: format!("footprint reference '{reference}' appears {count} times"),
                    reference: Some(reference.clone()),
                }),
                _ => {}
            }
        }
    } else {
        for (reference, count) in &reference_counts {
            if *count > 1 {
                diagnostics.push(UpdateDiagnostic {
                    code: "duplicate_reference".to_string(),
                    message: format!("footprint reference '{reference}' appears {count} times"),
                    reference: Some(reference.clone()),
                });
            }
        }
    }

    candidates.sort_by(|left, right| {
        left.reference
            .cmp(&right.reference)
            .then_with(|| left.library_id.cmp(&right.library_id))
    });
    let selected = candidates
        .into_iter()
        .filter(|candidate| {
            filters
                .references
                .as_ref()
                .is_none_or(|references| references.contains(&candidate.reference))
        })
        .filter(|candidate| {
            filters
                .library_ids
                .as_ref()
                .is_none_or(|ids| ids.contains(&candidate.library_id))
        })
        .collect::<Vec<_>>();

    coverage.skipped_unlinked.planned = selected
        .iter()
        .filter(|candidate| candidate.library_id.is_empty())
        .count();
    coverage.selected.planned = selected
        .iter()
        .filter(|candidate| !candidate.library_id.is_empty())
        .count();
    if filters.references.is_some() {
        for candidate in selected
            .iter()
            .filter(|candidate| candidate.library_id.is_empty())
        {
            diagnostics.push(UpdateDiagnostic {
                code: "unlinked_footprint".to_string(),
                message: format!(
                    "footprint '{}' has no resolvable Library:Footprint identifier",
                    candidate.reference
                ),
                reference: Some(candidate.reference.clone()),
            });
        }
    }

    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(filters).expect("filters serialize"));
    hasher.update(serde_json::to_vec(net_codes).expect("nets serialize"));
    hasher.update(serde_json::to_vec(routed_nets).expect("routed nets serialize"));
    let mut changes = Vec::new();
    let mut prepared_items = Vec::new();
    for candidate in selected {
        if candidate.library_id.is_empty() {
            continue;
        }
        let Some((nickname, entry)) = candidate.library_id.split_once(':') else {
            diagnostics.push(UpdateDiagnostic {
                code: "invalid_library_id".to_string(),
                message: format!(
                    "footprint '{}' has malformed library id '{}'",
                    candidate.reference, candidate.library_id
                ),
                reference: Some(candidate.reference),
            });
            continue;
        };
        if nickname.is_empty() || entry.is_empty() {
            diagnostics.push(UpdateDiagnostic {
                code: "invalid_library_id".to_string(),
                message: format!(
                    "footprint '{}' has malformed library id '{}'",
                    candidate.reference, candidate.library_id
                ),
                reference: Some(candidate.reference),
            });
            continue;
        }

        let library_path = match super::library::resolve_footprint_path(
            &candidate.library_id,
            board_path.parent(),
        ) {
            Ok(path) => path,
            Err(message) => {
                diagnostics.push(UpdateDiagnostic {
                    code: "footprint_library_resolution_failed".to_string(),
                    message,
                    reference: Some(candidate.reference),
                });
                continue;
            }
        };
        let source = match std::fs::read_to_string(&library_path) {
            Ok(source) => source,
            Err(error) => {
                diagnostics.push(UpdateDiagnostic {
                    code: "footprint_library_read_failed".to_string(),
                    message: format!("failed to read {}: {error}", library_path.display()),
                    reference: Some(candidate.reference),
                });
                continue;
            }
        };
        let library = match parse_library_footprint(&candidate.library_id, &source) {
            Ok(library) => library,
            Err(error) => {
                diagnostics.push(UpdateDiagnostic {
                    code: "unsupported_library_footprint".to_string(),
                    message: format!("{error:#}"),
                    reference: Some(candidate.reference),
                });
                continue;
            }
        };
        let prepared =
            match build_updated_instance(&candidate.instance, &library, net_codes, routed_nets) {
                Ok(prepared) => prepared,
                Err(error) => {
                    diagnostics.push(UpdateDiagnostic {
                        code: "footprint_update_conflict".to_string(),
                        message: format!("{error:#}"),
                        reference: Some(candidate.reference),
                    });
                    continue;
                }
            };

        hasher.update(candidate.item.type_url.as_bytes());
        hasher.update(&candidate.item.value);
        hasher.update(candidate.library_id.as_bytes());
        hasher.update(source.as_bytes());
        if prepared.changed_domains.is_empty() {
            coverage.unchanged.planned += 1;
        } else {
            coverage.changed.planned += 1;
            changes.push(PlannedUpdate {
                reference: candidate.reference,
                library_id: candidate.library_id,
                changed_domains: prepared.changed_domains,
                preserved: prepared.preserved,
            });
            prepared_items.push(prepared.item);
        }
    }

    if !diagnostics.is_empty() {
        coverage.conflicts.planned = diagnostics.len();
        changes.clear();
        prepared_items.clear();
        coverage.changed.planned = 0;
        return UpdatePlan {
            status: PlanStatus::Conflict,
            plan_revision: format!("{:x}", hasher.finalize()),
            coverage,
            changes,
            diagnostics,
            prepared_items,
        };
    }

    let status = if changes.is_empty() {
        PlanStatus::Noop
    } else {
        PlanStatus::Ready
    };
    UpdatePlan {
        status,
        plan_revision: format!("{:x}", hasher.finalize()),
        coverage,
        changes,
        diagnostics,
        prepared_items,
    }
}

fn parse_library_footprint(library_id: &str, source: &str) -> Result<LibraryFootprint> {
    let (library_nickname, entry_name) = library_id
        .split_once(':')
        .filter(|(nickname, entry)| !nickname.is_empty() && !entry.is_empty())
        .context("footprint identifier must use non-empty Library:Footprint syntax")?;
    let root = konnect_sexp::parse_sexp(source).context("invalid footprint S-expression")?;
    if root.head() != Some("footprint") {
        bail!("library source root must be a footprint");
    }

    validate_supported_children(&root)?;
    let properties = parse_library_properties(&root)?;
    let pads = super::pcb_components::extract_pad_definitions(source)?;
    // Custom properties travel as typed Field items below. Treating visible
    // properties as generic graphics as well would duplicate their text.
    let graphics = super::pcb_components::extract_graphic_definitions_without_properties(source)?;
    let models = super::library_footprint::models(&root)?;
    let attributes = super::library_footprint::attributes(&root)?;
    let definition = kiapi::board::types::Footprint {
        id: Some(kiapi::common::types::LibraryIdentifier {
            library_nickname: library_nickname.to_string(),
            entry_name: entry_name.to_string(),
        }),
        attributes: Some(super::library_footprint::description_and_keywords(&root)),
        ..Default::default()
    };

    Ok(LibraryFootprint {
        library_id: library_id.to_string(),
        definition,
        attributes,
        datasheet: properties.datasheet,
        description_field: properties.description,
        properties: properties.custom,
        pad_overrides: root
            .find_all("pad")
            .into_iter()
            .map(PadOverrides::parse)
            .collect::<Result<Vec<_>>>()?,
        pads,
        graphics,
        models,
    })
}

fn parse_library_properties(root: &konnect_sexp::SexpNode) -> Result<ParsedLibraryProperties> {
    let mut names = BTreeSet::new();
    let mut datasheet = None;
    let mut description = None;
    let mut custom = Vec::new();
    for property in root.find_all("property") {
        let name = property
            .get(1)
            .and_then(konnect_sexp::SexpNode::as_str)
            .context("property is missing its name")?;
        if !names.insert(name.to_string()) {
            bail!("property '{name}' appears more than once in the library footprint");
        }

        // Mandatory and custom properties share one lossless clause validator.
        // The mandatory values keep their existing first-class IPC fields; the
        // shared parser proves that none of their authored clauses would be
        // silently ignored without requiring a typed custom Field.
        let mandatory = matches!(name, "Reference" | "Value" | "Datasheet" | "Description");
        let parsed = parse_library_property(property, !mandatory)?;
        let value = property
            .get(2)
            .and_then(konnect_sexp::SexpNode::as_str)
            .with_context(|| format!("property '{name}' is missing its value"))?;
        match name {
            "Reference" | "Value" => {}
            "Datasheet" => datasheet = Some(value.to_string()),
            "Description" => description = Some(value.to_string()),
            _ => custom.push(parsed.context("custom property did not produce a typed field")?),
        }
    }

    Ok(ParsedLibraryProperties {
        datasheet,
        description,
        custom,
    })
}

/// Convert a footprint property into the typed `Field` shape carried by
/// KiCad's IPC model. Mandatory properties are validated through this same
/// path and then stored in their existing first-class fields. Unknown clauses
/// refuse here: accepting a property while dropping part of its authored
/// presentation would make the refresh lossy even when its value survived.
fn parse_library_property(
    property: &konnect_sexp::SexpNode,
    require_typed_field: bool,
) -> Result<Option<kiapi::board::types::Field>> {
    use kiapi::common::types::LockedState;

    let name = property
        .get(1)
        .and_then(konnect_sexp::SexpNode::as_str)
        .context("property is missing its name")?;
    let value = property
        .get(2)
        .and_then(konnect_sexp::SexpNode::as_str)
        .with_context(|| format!("property '{name}' is missing its value"))?;
    let mut position = None;
    let mut rotation = 0.0;
    let mut layer = None;
    let mut hidden = None;
    let mut knockout = None;
    let mut attributes = None;
    let mut identifier = None;

    for clause in property.children().unwrap_or_default().iter().skip(3) {
        let tag = clause
            .head()
            .with_context(|| format!("property '{name}' contains an unsupported atom"))?;
        match tag {
            "at" => {
                if position.is_some() {
                    bail!("property '{name}' contains duplicate 'at' clauses");
                }
                let count = clause.children().map_or(0, |children| children.len());
                if !matches!(count, 3 | 4) {
                    bail!("property '{name}' 'at' must contain x, y, and optional rotation");
                }
                let x = clause
                    .get_f64(1)
                    .with_context(|| format!("property '{name}' has an invalid X position"))?;
                let y = clause
                    .get_f64(2)
                    .with_context(|| format!("property '{name}' has an invalid Y position"))?;
                rotation = clause.get_f64(3).unwrap_or(0.0);
                if !x.is_finite() || !y.is_finite() || !rotation.is_finite() {
                    bail!("property '{name}' position and rotation must be finite");
                }
                position = Some(konnect_ipc::builders::vec2(x, y));
            }
            "layer" => {
                if layer.is_some() {
                    bail!("property '{name}' contains duplicate 'layer' clauses");
                }
                let layer_name = clause
                    .get(1)
                    .and_then(konnect_sexp::SexpNode::as_str)
                    .filter(|name| !name.is_empty())
                    .with_context(|| format!("property '{name}' has no layer name"))?;
                if clause.children().map_or(0, |children| children.len()) != 2 {
                    bail!("property '{name}' 'layer' must name exactly one layer");
                }
                layer = Some(
                    konnect_ipc::builders::try_layer_from_name(layer_name)
                        .with_context(|| format!("property '{name}' has an unsupported layer"))?
                        as i32,
                );
            }
            "hide" => {
                if hidden.is_some() {
                    bail!("property '{name}' contains duplicate 'hide' clauses");
                }
                hidden = Some(property_yes_no(clause, name, "hide")?);
            }
            "knockout" => {
                if knockout.is_some() {
                    bail!("property '{name}' contains duplicate 'knockout' clauses");
                }
                knockout = Some(property_yes_no(clause, name, "knockout")?);
            }
            "uuid" | "tstamp" => {
                if let Some(previous) = identifier {
                    bail!(
                        "property '{name}' contains multiple identifier clauses ('{previous}' and '{tag}')"
                    );
                }
                identifier = Some(tag);
                if clause.children().map_or(0, |children| children.len()) != 2
                    || clause
                        .get(1)
                        .and_then(konnect_sexp::SexpNode::as_str)
                        .is_none_or(str::is_empty)
                {
                    bail!("property '{name}' '{tag}' must contain exactly one identifier");
                }
            }
            "effects" => {
                if attributes.is_some() {
                    bail!("property '{name}' contains duplicate 'effects' clauses");
                }
                attributes = Some(parse_property_effects(clause, name)?);
            }
            unsupported => {
                bail!("property '{name}' clause '{unsupported}' is not supported losslessly")
            }
        }
    }

    if !require_typed_field {
        return Ok(None);
    }

    let position =
        position.with_context(|| format!("property '{name}' is missing its 'at' clause"))?;
    let layer =
        layer.with_context(|| format!("property '{name}' is missing its 'layer' clause"))?;
    let mut attributes =
        attributes.with_context(|| format!("property '{name}' is missing its 'effects' clause"))?;
    attributes.angle = Some(kiapi::common::types::Angle {
        value_degrees: rotation,
    });
    Ok(Some(kiapi::board::types::Field {
        id: None,
        name: name.to_string(),
        text: Some(kiapi::board::types::BoardText {
            // A library child's UUID is definition-local and cannot be reused
            // across placed instances. Let KiCad assign the board child ID.
            id: None,
            text: Some(kiapi::common::types::Text {
                position: Some(position),
                attributes: Some(attributes),
                text: value.to_string(),
                hyperlink: String::new(),
            }),
            layer,
            knockout: knockout.unwrap_or(false),
            locked: LockedState::LsUnlocked as i32,
            parent: None,
        }),
        visible: !hidden.unwrap_or(false),
    }))
}

fn property_yes_no(clause: &konnect_sexp::SexpNode, name: &str, tag: &str) -> Result<bool> {
    if clause.children().map_or(0, |children| children.len()) != 2 {
        bail!("property '{name}' '{tag}' must contain exactly one yes/no value");
    }
    match clause.get(1).and_then(konnect_sexp::SexpNode::as_str) {
        Some("yes") => Ok(true),
        Some("no") => Ok(false),
        _ => bail!("property '{name}' '{tag}' must be yes or no"),
    }
}

fn parse_property_effects(
    effects: &konnect_sexp::SexpNode,
    name: &str,
) -> Result<kiapi::common::types::TextAttributes> {
    use kiapi::common::types::{HorizontalAlignment, VerticalAlignment};

    let mut font = None;
    let mut horizontal = HorizontalAlignment::HaCenter;
    let mut vertical = VerticalAlignment::VaCenter;
    let mut mirrored = false;
    for clause in effects.children().unwrap_or_default().iter().skip(1) {
        let tag = clause
            .head()
            .with_context(|| format!("property '{name}' effects contain an unsupported atom"))?;
        match tag {
            "font" => {
                if font.replace(clause).is_some() {
                    bail!("property '{name}' contains duplicate font clauses");
                }
            }
            "justify" => {
                for value in clause.children().unwrap_or_default().iter().skip(1) {
                    match value.as_str().with_context(|| {
                        format!("property '{name}' justify contains a non-atom")
                    })? {
                        "left" if horizontal == HorizontalAlignment::HaCenter => {
                            horizontal = HorizontalAlignment::HaLeft
                        }
                        "right" if horizontal == HorizontalAlignment::HaCenter => {
                            horizontal = HorizontalAlignment::HaRight
                        }
                        "top" if vertical == VerticalAlignment::VaCenter => {
                            vertical = VerticalAlignment::VaTop
                        }
                        "bottom" if vertical == VerticalAlignment::VaCenter => {
                            vertical = VerticalAlignment::VaBottom
                        }
                        "mirror" if !mirrored => mirrored = true,
                        "left" | "right" => bail!(
                            "property '{name}' has conflicting horizontal justification"
                        ),
                        "top" | "bottom" => {
                            bail!("property '{name}' has conflicting vertical justification")
                        }
                        "mirror" => bail!("property '{name}' repeats mirrored justification"),
                        unsupported => bail!(
                            "property '{name}' justification '{unsupported}' is not supported losslessly"
                        ),
                    }
                }
            }
            unsupported => bail!(
                "property '{name}' effects clause '{unsupported}' is not supported losslessly"
            ),
        }
    }
    let font =
        font.with_context(|| format!("property '{name}' effects are missing the font clause"))?;

    let mut font_name = String::new();
    let mut size = None;
    let mut thickness = None;
    let mut bold = None;
    let mut italic = None;
    let mut line_spacing = None;
    for clause in font.children().unwrap_or_default().iter().skip(1) {
        let tag = clause
            .head()
            .with_context(|| format!("property '{name}' font contains an unsupported atom"))?;
        match tag {
            "face" => {
                if !font_name.is_empty() {
                    bail!("property '{name}' contains duplicate font face clauses");
                }
                font_name = clause
                    .get(1)
                    .and_then(konnect_sexp::SexpNode::as_str)
                    .filter(|face| !face.is_empty())
                    .with_context(|| format!("property '{name}' font face is invalid"))?
                    .to_string();
            }
            "size" => {
                if size.is_some() {
                    bail!("property '{name}' contains duplicate font size clauses");
                }
                if clause.children().map_or(0, |children| children.len()) != 3 {
                    bail!("property '{name}' font size must contain width and height");
                }
                let width = clause
                    .get_f64(1)
                    .with_context(|| format!("property '{name}' font width is invalid"))?;
                let height = clause
                    .get_f64(2)
                    .with_context(|| format!("property '{name}' font height is invalid"))?;
                if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
                    bail!("property '{name}' font size must be finite and positive");
                }
                size = Some((width, height));
            }
            "thickness" => {
                if thickness.is_some() {
                    bail!("property '{name}' contains duplicate font thickness clauses");
                }
                let value = clause
                    .get_f64(1)
                    .with_context(|| format!("property '{name}' font thickness is invalid"))?;
                if clause.children().map_or(0, |children| children.len()) != 2
                    || !value.is_finite()
                    || value <= 0.0
                {
                    bail!("property '{name}' font thickness must be finite and positive");
                }
                thickness = Some(value);
            }
            "bold" => {
                if bold.is_some() {
                    bail!("property '{name}' contains duplicate font 'bold' clauses");
                }
                bold = Some(property_yes_no(clause, name, "font bold")?);
            }
            "italic" => {
                if italic.is_some() {
                    bail!("property '{name}' contains duplicate font 'italic' clauses");
                }
                italic = Some(property_yes_no(clause, name, "font italic")?);
            }
            "line_spacing" => {
                if line_spacing.is_some() {
                    bail!("property '{name}' contains duplicate font 'line_spacing' clauses");
                }
                let value = clause
                    .get_f64(1)
                    .with_context(|| format!("property '{name}' line spacing is invalid"))?;
                if clause.children().map_or(0, |children| children.len()) != 2
                    || !value.is_finite()
                    || value <= 0.0
                {
                    bail!("property '{name}' line spacing must be finite and positive");
                }
                line_spacing = Some(value);
            }
            unsupported => {
                bail!("property '{name}' font clause '{unsupported}' is not supported losslessly")
            }
        }
    }
    let (width, height) =
        size.with_context(|| format!("property '{name}' font is missing its size"))?;
    Ok(kiapi::common::types::TextAttributes {
        font_name,
        horizontal_alignment: horizontal as i32,
        vertical_alignment: vertical as i32,
        angle: None,
        line_spacing: line_spacing.unwrap_or(1.0),
        stroke_width: Some(konnect_ipc::builders::distance(
            thickness.unwrap_or(width * 0.15),
        )),
        italic: italic.unwrap_or(false),
        bold: bold.unwrap_or(false),
        underlined: false,
        visible: true,
        mirrored,
        multiline: false,
        keep_upright: false,
        size: Some(konnect_ipc::builders::vec2(width, height)),
    })
}

fn validate_supported_children(root: &konnect_sexp::SexpNode) -> Result<()> {
    for child in root.children().unwrap_or_default().iter().skip(2) {
        let Some(tag) = child.head() else {
            continue;
        };
        match tag {
            "version" | "generator" | "generator_version" | "layer" | "descr" | "tags" | "attr"
            | "property" | "fp_text" | "fp_line" | "fp_rect" | "fp_circle" | "fp_arc"
            | "fp_poly" | "pad" | "model" => {}
            // KiCad 10 writes these two flags into every footprint it saves
            // (all 15,428 official library files carry them, every one at its
            // default). At the default the typed rebuild loses nothing;
            // a non-default value carries semantics it cannot represent.
            "embedded_fonts" | "duplicate_pad_numbers_are_jumpers" => {
                let value = child.get(1).and_then(konnect_sexp::SexpNode::as_str);
                if value != Some("no") {
                    bail!(
                        "footprint '{tag} {}' is not supported by typed library refresh",
                        value.unwrap_or("")
                    );
                }
            }
            unsupported => {
                bail!("footprint child '{unsupported}' is not supported by typed library refresh")
            }
        }
        match tag {
            "pad" => validate_pad(child)?,
            "fp_line" | "fp_rect" | "fp_circle" | "fp_arc" | "fp_poly" => {
                validate_graphic(child, tag)?
            }
            "fp_text" => {
                let kind = child
                    .get(1)
                    .and_then(konnect_sexp::SexpNode::as_str)
                    .context("fp_text is missing its kind")?;
                match kind {
                    "reference" | "value" => {}
                    "user" => validate_user_text(child)?,
                    _ => {
                        bail!(
                            "fp_text kind '{kind}' is not supported losslessly by typed library refresh"
                        );
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Accept only the `fp_text user` subset the typed IPC rebuild can reproduce.
/// Refusing an unknown clause is intentional: silently dropping even a visual
/// modifier would make a refreshed board differ from its library definition.
fn validate_user_text(text: &konnect_sexp::SexpNode) -> Result<()> {
    text.get(2)
        .and_then(konnect_sexp::SexpNode::as_str)
        .context("fp_text user is missing its text")?;

    let mut saw_at = false;
    let mut saw_layer = false;
    let mut saw_effects = false;
    let mut identifier = None;
    for clause in text.children().unwrap_or_default().iter().skip(3) {
        let tag = clause
            .head()
            .context("fp_text user contains an unsupported atom")?;
        match tag {
            "at" => {
                if saw_at {
                    bail!("fp_text user contains duplicate 'at' clauses");
                }
                saw_at = true;
                let count = clause.children().map_or(0, |children| children.len());
                if !matches!(count, 3 | 4) {
                    bail!("fp_text user 'at' must contain x, y, and optional rotation");
                }
                for index in 1..count {
                    let value = clause
                        .get_f64(index)
                        .with_context(|| format!("fp_text user 'at' value {index} is invalid"))?;
                    if !value.is_finite() {
                        bail!("fp_text user 'at' values must be finite");
                    }
                }
            }
            "layer" => {
                if saw_layer {
                    bail!("fp_text user contains duplicate 'layer' clauses");
                }
                saw_layer = true;
                if clause.children().map_or(0, |children| children.len()) != 2
                    || clause
                        .get(1)
                        .and_then(konnect_sexp::SexpNode::as_str)
                        .is_none_or(str::is_empty)
                {
                    bail!("fp_text user 'layer' must name exactly one layer");
                }
            }
            "uuid" | "tstamp" => {
                if let Some(previous) = identifier {
                    bail!(
                        "fp_text user contains multiple identifier clauses ('{previous}' and '{tag}')"
                    );
                }
                identifier = Some(tag);
                if clause.children().map_or(0, |children| children.len()) != 2
                    || clause
                        .get(1)
                        .and_then(konnect_sexp::SexpNode::as_str)
                        .is_none_or(str::is_empty)
                {
                    bail!("fp_text user '{tag}' must contain exactly one identifier");
                }
            }
            "effects" => {
                if saw_effects {
                    bail!("fp_text user contains duplicate 'effects' clauses");
                }
                saw_effects = true;
                validate_user_text_effects(clause)?;
            }
            unsupported => {
                bail!("fp_text user clause '{unsupported}' is not supported losslessly");
            }
        }
    }

    if !saw_at {
        bail!("fp_text user is missing its 'at' clause");
    }
    if !saw_layer {
        bail!("fp_text user is missing its 'layer' clause");
    }
    if !saw_effects {
        bail!("fp_text user is missing its 'effects' clause");
    }
    Ok(())
}

fn validate_user_text_effects(effects: &konnect_sexp::SexpNode) -> Result<()> {
    let mut font = None;
    for clause in effects.children().unwrap_or_default().iter().skip(1) {
        let tag = clause
            .head()
            .context("fp_text user effects contain an unsupported atom")?;
        if tag != "font" {
            bail!("fp_text user effects clause '{tag}' is not supported losslessly");
        }
        if font.replace(clause).is_some() {
            bail!("fp_text user contains duplicate font clauses");
        }
    }
    let font = font.context("fp_text user effects are missing the font clause")?;

    let mut size = None;
    let mut thickness = None;
    for clause in font.children().unwrap_or_default().iter().skip(1) {
        let tag = clause
            .head()
            .context("fp_text user font contains an unsupported atom")?;
        match tag {
            "size" => {
                if size.is_some() {
                    bail!("fp_text user contains duplicate font size clauses");
                }
                if clause.children().map_or(0, |children| children.len()) != 3 {
                    bail!("fp_text user font size must contain width and height");
                }
                let width = clause
                    .get_f64(1)
                    .context("fp_text user font width is invalid")?;
                let height = clause
                    .get_f64(2)
                    .context("fp_text user font height is invalid")?;
                if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
                    bail!("fp_text user font size must be finite and positive");
                }
                if width != height {
                    bail!("fp_text user non-square font size is not supported losslessly");
                }
                size = Some(width);
            }
            "thickness" => {
                if thickness.is_some() {
                    bail!("fp_text user contains duplicate font thickness clauses");
                }
                if clause.children().map_or(0, |children| children.len()) != 2 {
                    bail!("fp_text user font thickness must contain exactly one value");
                }
                let value = clause
                    .get_f64(1)
                    .context("fp_text user font thickness is invalid")?;
                if !value.is_finite() || value <= 0.0 {
                    bail!("fp_text user font thickness must be finite and positive");
                }
                thickness = Some(value);
            }
            unsupported => {
                bail!("fp_text user font clause '{unsupported}' is not supported losslessly");
            }
        }
    }
    size.context("fp_text user font is missing its size")?;
    thickness.context("fp_text user font is missing its thickness")?;
    Ok(())
}

fn validate_graphic(graphic: &konnect_sexp::SexpNode, kind: &str) -> Result<()> {
    let allowed: &[&str] = match kind {
        "fp_line" => &["start", "end", "stroke", "layer", "uuid", "tstamp"],
        "fp_rect" => &["start", "end", "stroke", "fill", "layer", "uuid", "tstamp"],
        "fp_circle" => &["center", "end", "stroke", "fill", "layer", "uuid", "tstamp"],
        "fp_arc" => &["start", "mid", "end", "stroke", "layer", "uuid", "tstamp"],
        "fp_poly" => &["pts", "stroke", "fill", "layer", "uuid", "tstamp"],
        _ => bail!("unsupported footprint graphic '{kind}'"),
    };
    for child in graphic.children().unwrap_or_default().iter().skip(1) {
        let Some(tag) = child.head() else {
            bail!("{kind} contains an unsupported atom");
        };
        if !allowed.contains(&tag) {
            bail!("{kind} clause '{tag}' is not supported by typed library refresh");
        }
        match tag {
            "stroke" => {
                for stroke_child in child.children().unwrap_or_default().iter().skip(1) {
                    let stroke_tag = stroke_child
                        .head()
                        .context("graphic stroke contains an unsupported atom")?;
                    match stroke_tag {
                        "width" => {}
                        "type" => {
                            let stroke_type = stroke_child
                                .get(1)
                                .and_then(konnect_sexp::SexpNode::as_str)
                                .context("graphic stroke type is missing")?;
                            if !matches!(stroke_type, "solid" | "default") {
                                bail!(
                                    "graphic stroke type '{stroke_type}' is not supported losslessly"
                                );
                            }
                        }
                        unsupported => bail!(
                            "graphic stroke clause '{unsupported}' is not supported losslessly"
                        ),
                    }
                }
            }
            "fill" => {
                let fill = child
                    .get(1)
                    .and_then(konnect_sexp::SexpNode::as_str)
                    .context("graphic fill is missing")?;
                if !matches!(fill, "none" | "no" | "solid" | "yes") {
                    bail!("graphic fill '{fill}' is not supported losslessly");
                }
            }
            "pts"
                if child
                    .children()
                    .unwrap_or_default()
                    .iter()
                    .skip(1)
                    .any(|point| point.head() != Some("xy")) =>
            {
                bail!("fp_poly contains a non-xy point");
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_pad(pad: &konnect_sexp::SexpNode) -> Result<()> {
    let shape = pad
        .get(3)
        .and_then(konnect_sexp::SexpNode::as_str)
        .context("footprint pad is missing its shape")?;
    if !matches!(shape, "circle" | "rect" | "oval" | "roundrect") {
        bail!("pad shape '{shape}' is not supported by typed library refresh");
    }
    for child in pad.children().unwrap_or_default().iter().skip(4) {
        let Some(tag) = child.head() else {
            continue;
        };
        if !matches!(
            tag,
            "at" | "size"
                | "layers"
                | "drill"
                | "roundrect_rratio"
                | "uuid"
                | "tstamp"
                | "remove_unused_layers"
                | "solder_mask_margin"
                | "solder_paste_margin"
                | "solder_paste_margin_ratio"
                | "clearance"
                | "die_length"
                | "zone_connect"
                | "thermal_bridge_width"
                | "thermal_gap"
                | "thermal_bridge_angle"
        ) {
            bail!("pad clause '{tag}' is not supported by typed library refresh");
        }
        if tag == "remove_unused_layers" {
            // The typed rebuild always keeps unused layers (`UlrKeep`), so a
            // pad that actually removes them cannot be rebuilt losslessly.
            let value = child.get(1).and_then(konnect_sexp::SexpNode::as_str);
            if value != Some("no") {
                bail!("pad 'remove_unused_layers yes' is not supported by typed library refresh");
            }
        }
        if tag == "drill" {
            for nested in child.children().unwrap_or_default().iter().skip(1) {
                if nested.head().is_some() {
                    bail!("nested drill clauses are not supported by typed library refresh");
                }
            }
        }
    }
    Ok(())
}

fn build_updated_instance(
    current: &kiapi::board::types::FootprintInstance,
    library: &LibraryFootprint,
    net_codes: &BTreeMap<String, i32>,
    routed_nets: &BTreeSet<String>,
) -> Result<PreparedUpdate> {
    let current_definition = current
        .definition
        .as_ref()
        .context("board footprint has no embedded definition")?;
    let current_id = current_definition
        .id
        .as_ref()
        .map(|id| format!("{}:{}", id.library_nickname, id.entry_name))
        .unwrap_or_default();
    if current_id != library.library_id {
        bail!(
            "board footprint library id '{current_id}' does not match '{}'",
            library.library_id
        );
    }

    let mut old_nets = BTreeMap::<String, kiapi::board::types::Net>::new();
    for item in &current_definition.items {
        if !item.type_url.ends_with("kiapi.board.types.Pad") {
            continue;
        }
        let pad = kiapi::board::types::Pad::decode(item.value.as_slice())
            .context("board footprint contains an invalid pad")?;
        let Some(net) = pad.net.filter(|net| !net.name.is_empty()) else {
            continue;
        };
        if let Some(existing) = old_nets.insert(pad.number.clone(), net.clone()) {
            if existing.name != net.name {
                bail!(
                    "logical pad {} carries multiple nets ('{}' and '{}')",
                    pad.number,
                    existing.name,
                    net.name
                );
            }
        }
    }
    let new_numbers = library
        .pads
        .iter()
        .map(|pad| pad.number.as_str())
        .collect::<BTreeSet<_>>();
    for (number, net) in &old_nets {
        if !new_numbers.contains(number.as_str()) {
            let routed = routed_nets.contains(&net.name);
            bail!(
                "library update removes connected pad {number} on net '{}'{}",
                net.name,
                if routed { " with routed copper" } else { "" }
            );
        }
    }

    let position = current.position.as_ref().cloned().unwrap_or_default();
    let rotation = current
        .orientation
        .as_ref()
        .map(|angle| angle.value_degrees)
        .unwrap_or(0.0);
    let is_back = current.layer == kiapi::board::types::BoardLayer::BlBCu as i32;
    let layer = if is_back { "B.Cu" } else { "F.Cu" };
    let (pads, graphics) = if is_back {
        (
            library
                .pads
                .iter()
                .map(mirror_pad)
                .collect::<Result<Vec<_>>>()?,
            library
                .graphics
                .iter()
                .map(mirror_graphic)
                .collect::<Result<Vec<_>>>()?,
        )
    } else {
        (library.pads.clone(), library.graphics.clone())
    };
    let packed = konnect_ipc::KiCadIpcClient::build_footprint_item(
        &library.library_id,
        &field_text(&current.reference_field),
        &field_text(&current.value_field),
        &pads,
        &graphics,
        &Default::default(),
        konnect_ipc::builders::nm_to_mm(position.x_nm),
        konnect_ipc::builders::nm_to_mm(position.y_nm),
        rotation,
        layer,
    )?;
    let built = kiapi::board::types::FootprintInstance::decode(packed.value.as_slice())
        .context("typed footprint builder returned an invalid item")?;

    let mut updated = current.clone();
    let mut definition = built
        .definition
        .context("typed footprint builder returned no definition")?;
    definition.attributes = library.definition.attributes.clone();
    definition.reference_field = current_definition.reference_field.clone();
    definition.value_field = current_definition.value_field.clone();
    definition.datasheet_field = current_definition.datasheet_field.clone();
    definition.description_field = current_definition.description_field.clone();
    apply_field_value(
        &mut definition.datasheet_field,
        library.datasheet.as_deref(),
    );
    apply_field_value(
        &mut definition.description_field,
        library.description_field.as_deref(),
    );
    definition.items.extend(
        library.models.iter().map(|model| {
            konnect_ipc::builders::pack_any(model, "kiapi.board.types.Footprint3DModel")
        }),
    );
    let mut pad_index = 0;
    for item in &mut definition.items {
        if item.type_url.ends_with("kiapi.board.types.Pad") {
            let mut pad = kiapi::board::types::Pad::decode(item.value.as_slice())?;
            library
                .pad_overrides
                .get(pad_index)
                .context("library pad override count differs from typed pads")?
                .apply(&mut pad)?;
            pad_index += 1;
            pad.net = old_nets
                .get(&pad.number)
                .map(|old| kiapi::board::types::Net {
                    code: net_codes
                        .get(&old.name)
                        .copied()
                        .or_else(|| old.code.as_ref().map(|code| code.value))
                        .map(|value| kiapi::board::types::NetCode { value }),
                    name: old.name.clone(),
                });
            *item = konnect_ipc::builders::pack_any(&pad, "kiapi.board.types.Pad");
        } else if is_back && item.type_url.ends_with("kiapi.board.types.BoardText") {
            let mut text = kiapi::board::types::BoardText::decode(item.value.as_slice())?;
            if is_side_specific_layer(text.layer) {
                if let Some(attributes) =
                    text.text.as_mut().and_then(|text| text.attributes.as_mut())
                {
                    attributes.mirrored = true;
                }
            }
            *item = konnect_ipc::builders::pack_any(&text, "kiapi.board.types.BoardText");
        }
    }
    merge_custom_properties(
        &mut definition,
        current_definition,
        &library.properties,
        &position,
        rotation,
        is_back,
    )?;
    updated.definition = Some(definition);
    apply_field_value(&mut updated.datasheet_field, library.datasheet.as_deref());
    apply_field_value(
        &mut updated.description_field,
        library.description_field.as_deref(),
    );
    let mut attributes = library.attributes.clone();
    if let Some(current_attributes) = current.attributes.as_ref() {
        attributes.not_in_schematic = current_attributes.not_in_schematic;
        attributes.do_not_populate = current_attributes.do_not_populate;
    }
    updated.attributes = Some(attributes);

    let changed_domains = changed_domains(current, &updated)?;
    let preserved = PreservedState::derive(current, &updated, &old_nets);
    Ok(PreparedUpdate {
        item: konnect_ipc::builders::pack_any(&updated, "kiapi.board.types.FootprintInstance"),
        changed_domains,
        preserved,
    })
}

fn apply_field_value(field: &mut Option<kiapi::board::types::Field>, value: Option<&str>) {
    let Some(value) = value else {
        return;
    };
    field
        .get_or_insert_with(Default::default)
        .text
        .get_or_insert_with(Default::default)
        .text
        .get_or_insert_with(Default::default)
        .text = value.to_string();
}

fn merge_custom_properties(
    updated: &mut kiapi::board::types::Footprint,
    current: &kiapi::board::types::Footprint,
    library_properties: &[kiapi::board::types::Field],
    footprint_position: &kiapi::common::types::Vector2,
    footprint_rotation: f64,
    is_back: bool,
) -> Result<()> {
    let library_names = library_properties
        .iter()
        .map(|field| field.name.as_str())
        .collect::<BTreeSet<_>>();
    let mut current_names = BTreeSet::new();
    for item in current
        .items
        .iter()
        .filter(|item| item.type_url.ends_with("kiapi.board.types.Field"))
    {
        let field = kiapi::board::types::Field::decode(item.value.as_slice())
            .context("board footprint contains an invalid custom property")?;
        if field.name.is_empty() {
            bail!("board footprint contains a custom property without a name");
        }
        if !current_names.insert(field.name.clone()) {
            bail!(
                "board footprint contains more than one custom property named '{}'",
                field.name
            );
        }
        if !library_names.contains(field.name.as_str()) {
            // A field that exists only on the placed instance belongs to that
            // instance. Preserve its complete typed representation verbatim.
            updated.items.push(item.clone());
        }
    }
    for property in library_properties {
        let property =
            transform_library_property(property, footprint_position, footprint_rotation, is_back)?;
        updated.items.push(konnect_ipc::builders::pack_any(
            &property,
            "kiapi.board.types.Field",
        ));
    }
    Ok(())
}

fn transform_library_property(
    property: &kiapi::board::types::Field,
    footprint_position: &kiapi::common::types::Vector2,
    footprint_rotation: f64,
    is_back: bool,
) -> Result<kiapi::board::types::Field> {
    let mut property = property.clone();
    property.id = None;
    let board_text = property
        .text
        .as_mut()
        .with_context(|| format!("property '{}' has no board text", property.name))?;
    board_text.id = None;
    let text = board_text
        .text
        .as_mut()
        .with_context(|| format!("property '{}' has no text value", property.name))?;
    let local_position = text
        .position
        .as_ref()
        .with_context(|| format!("property '{}' has no position", property.name))?;
    let local_x = konnect_ipc::builders::nm_to_mm(local_position.x_nm);
    let mut local_y = konnect_ipc::builders::nm_to_mm(local_position.y_nm);
    if is_back {
        local_y = -local_y;
    }
    let (board_x, board_y) = konnect_sexp::geometry::transform_pad(
        local_x,
        local_y,
        konnect_ipc::builders::nm_to_mm(footprint_position.x_nm),
        konnect_ipc::builders::nm_to_mm(footprint_position.y_nm),
        footprint_rotation,
    );
    text.position = Some(konnect_ipc::builders::vec2(board_x, board_y));

    let attributes = text
        .attributes
        .as_mut()
        .with_context(|| format!("property '{}' has no text attributes", property.name))?;
    let local_angle = attributes
        .angle
        .as_ref()
        .map(|angle| angle.value_degrees)
        .unwrap_or(0.0);
    let local_angle = if is_back {
        180.0 - local_angle
    } else {
        local_angle
    };
    attributes.angle = Some(kiapi::common::types::Angle {
        value_degrees: readable_property_angle(local_angle + footprint_rotation),
    });
    if is_back {
        attributes.mirrored = !attributes.mirrored;
        let layer = kiapi::board::types::BoardLayer::try_from(board_text.layer)
            .with_context(|| format!("property '{}' has an invalid layer", property.name))?;
        let layer_name = konnect_ipc::builders::layer_name(layer)
            .with_context(|| format!("property '{}' has an unnamed layer", property.name))?;
        board_text.layer =
            konnect_ipc::builders::try_layer_from_name(&flip_layer_name(layer_name)?)? as i32;
    }
    Ok(property)
}

fn readable_property_angle(degrees: f64) -> f64 {
    let mut angle = degrees.rem_euclid(360.0);
    if angle > 90.0 && angle <= 270.0 {
        angle -= 180.0;
    }
    angle
}

fn mirror_pad(pad: &konnect_ipc::IpcPadDefinition) -> Result<konnect_ipc::IpcPadDefinition> {
    let mut mirrored = pad.clone();
    mirrored.y = -mirrored.y;
    mirrored.rotation = -mirrored.rotation;
    mirrored.layers = mirrored
        .layers
        .iter()
        .map(|layer| flip_layer_name(layer))
        .collect::<Result<_>>()?;
    Ok(mirrored)
}

fn mirror_graphic(
    graphic: &konnect_ipc::IpcGraphicDefinition,
) -> Result<konnect_ipc::IpcGraphicDefinition> {
    use konnect_ipc::IpcGraphicDefinition as Graphic;

    let point = |(x, y): (f64, f64)| (x, -y);
    Ok(match graphic {
        Graphic::Line {
            start,
            end,
            layer,
            width,
        } => Graphic::Line {
            start: point(*start),
            end: point(*end),
            layer: flip_layer_name(layer)?,
            width: *width,
        },
        Graphic::Rect {
            start,
            end,
            layer,
            width,
            filled,
        } => Graphic::Rect {
            start: point(*start),
            end: point(*end),
            layer: flip_layer_name(layer)?,
            width: *width,
            filled: *filled,
        },
        Graphic::Circle {
            center,
            end,
            layer,
            width,
            filled,
        } => Graphic::Circle {
            center: point(*center),
            end: point(*end),
            layer: flip_layer_name(layer)?,
            width: *width,
            filled: *filled,
        },
        Graphic::Arc {
            start,
            mid,
            end,
            layer,
            width,
        } => Graphic::Arc {
            start: point(*end),
            mid: point(*mid),
            end: point(*start),
            layer: flip_layer_name(layer)?,
            width: *width,
        },
        Graphic::Poly {
            points,
            layer,
            width,
            filled,
        } => Graphic::Poly {
            points: points.iter().copied().map(point).collect(),
            layer: flip_layer_name(layer)?,
            width: *width,
            filled: *filled,
        },
        Graphic::Text {
            text,
            position,
            rotation,
            layer,
            size,
            stroke_width_mm,
        } => Graphic::Text {
            text: text.clone(),
            position: point(*position),
            rotation: 180.0 - rotation,
            layer: flip_layer_name(layer)?,
            size: *size,
            stroke_width_mm: *stroke_width_mm,
        },
    })
}

fn flip_layer_name(layer: &str) -> Result<String> {
    let flipped = match layer {
        "F.Cu" => "B.Cu",
        "B.Cu" => "F.Cu",
        "F.Adhes" => "B.Adhes",
        "B.Adhes" => "F.Adhes",
        "F.Paste" => "B.Paste",
        "B.Paste" => "F.Paste",
        "F.SilkS" | "F.Silkscreen" => "B.SilkS",
        "B.SilkS" | "B.Silkscreen" => "F.SilkS",
        "F.Mask" => "B.Mask",
        "B.Mask" => "F.Mask",
        "F.CrtYd" | "F.Courtyard" => "B.CrtYd",
        "B.CrtYd" | "B.Courtyard" => "F.CrtYd",
        "F.Fab" => "B.Fab",
        "B.Fab" => "F.Fab",
        "*.Cu" | "*.Mask" | "*.Paste" => layer,
        other if other.starts_with("F.") || other.starts_with("B.") => {
            bail!("unsupported side-specific footprint layer '{other}'")
        }
        other => other,
    };
    Ok(flipped.to_string())
}

fn is_side_specific_layer(layer: i32) -> bool {
    use kiapi::board::types::BoardLayer;

    matches!(
        BoardLayer::try_from(layer).ok(),
        Some(
            BoardLayer::BlFCu
                | BoardLayer::BlBCu
                | BoardLayer::BlFAdhes
                | BoardLayer::BlBAdhes
                | BoardLayer::BlFPaste
                | BoardLayer::BlBPaste
                | BoardLayer::BlFSilkS
                | BoardLayer::BlBSilkS
                | BoardLayer::BlFMask
                | BoardLayer::BlBMask
                | BoardLayer::BlFCrtYd
                | BoardLayer::BlBCrtYd
                | BoardLayer::BlFFab
                | BoardLayer::BlBFab
        )
    )
}

/// The placed-instance state that a refresh must leave as KiCad reports it: everything
/// except the library-owned definition. KiCad reports an unset mounting style as
/// `FmsUnspecified` after an update where the pre-update state carried `FmsUnknown`;
/// both mean "not set" (as in `library_owned_attributes`), so they are compared as one.
fn comparable_instance(
    instance: &kiapi::board::types::FootprintInstance,
) -> kiapi::board::types::FootprintInstance {
    use kiapi::board::types::FootprintMountingStyle as Style;
    let mut copy = instance.clone();
    copy.definition = None;
    if let Some(attributes) = copy.attributes.as_mut() {
        if attributes.mounting_style == Style::FmsUnknown as i32 {
            attributes.mounting_style = Style::FmsUnspecified as i32;
        }
    }
    copy
}

fn instance_state_matches(
    expected: &kiapi::board::types::FootprintInstance,
    actual: &kiapi::board::types::FootprintInstance,
) -> bool {
    comparable_instance(expected) == comparable_instance(actual)
}

/// First point at which two readbacks differ, as short `Debug` excerpts, so a refused
/// transaction says what KiCad stored differently instead of only that something differs.
fn readback_difference_summary(
    expected: &kiapi::board::types::FootprintInstance,
    actual: &kiapi::board::types::FootprintInstance,
) -> String {
    fn excerpt(expected: &str, actual: &str) -> String {
        let e: Vec<char> = expected.chars().collect();
        let a: Vec<char> = actual.chars().collect();
        let at = e.iter().zip(&a).take_while(|(x, y)| x == y).count();
        let start = at.saturating_sub(60);
        let take = |chars: &[char]| chars.iter().skip(start).take(220).collect::<String>();
        format!("expected `{}` / actual `{}`", take(&e), take(&a))
    }
    if !instance_state_matches(expected, actual) {
        let (e, a) = (comparable_instance(expected), comparable_instance(actual));
        return format!(
            "instance: {}",
            excerpt(&format!("{e:?}"), &format!("{a:?}"))
        );
    }
    let pads = |fp: &kiapi::board::types::FootprintInstance| {
        fp.definition
            .as_ref()
            .and_then(|definition| normalized_items(definition, "Pad").ok())
            .unwrap_or_default()
            .iter()
            .filter_map(|bytes| kiapi::board::types::Pad::decode(bytes.as_slice()).ok())
            .collect::<Vec<_>>()
    };
    let (expected_pads, actual_pads) = (pads(expected), pads(actual));
    for (index, (e, a)) in expected_pads.iter().zip(&actual_pads).enumerate() {
        if e != a {
            return format!("pad #{index}: {}", pad_field_differences(e, a).join("; "));
        }
    }
    format!(
        "pad count expected {} / actual {}",
        expected_pads.len(),
        actual_pads.len()
    )
}

/// Every differing top-level pad / padstack field, with short values.
fn pad_field_differences(
    expected: &kiapi::board::types::Pad,
    actual: &kiapi::board::types::Pad,
) -> Vec<String> {
    fn short(value: &impl std::fmt::Debug) -> String {
        let text = format!("{value:?}");
        if text.chars().count() > 160 {
            format!("{}...", text.chars().take(160).collect::<String>())
        } else {
            text
        }
    }
    let mut out = Vec::new();
    macro_rules! compare {
        ($e:expr, $a:expr, $name:expr) => {
            if $e != $a {
                out.push(format!(
                    "{}: expected {} / actual {}",
                    $name,
                    short(&$e),
                    short(&$a)
                ));
            }
        };
    }
    compare!(expected.locked, actual.locked, "locked");
    compare!(expected.number, actual.number, "number");
    compare!(expected.r#type, actual.r#type, "type");
    compare!(expected.position, actual.position, "position");
    compare!(
        expected.copper_clearance_override,
        actual.copper_clearance_override,
        "copper_clearance_override"
    );
    match (&expected.pad_stack, &actual.pad_stack) {
        (Some(e), Some(a)) => {
            compare!(e.r#type, a.r#type, "stack.type");
            compare!(e.layers, a.layers, "stack.layers");
            compare!(e.drill, a.drill, "stack.drill");
            compare!(
                e.unconnected_layer_removal,
                a.unconnected_layer_removal,
                "stack.unconnected_layer_removal"
            );
            compare!(e.copper_layers, a.copper_layers, "stack.copper_layers");
            compare!(e.angle, a.angle, "stack.angle");
            compare!(
                e.front_outer_layers,
                a.front_outer_layers,
                "stack.front_outer_layers"
            );
            compare!(
                e.back_outer_layers,
                a.back_outer_layers,
                "stack.back_outer_layers"
            );
            compare!(e.zone_settings, a.zone_settings, "stack.zone_settings");
            compare!(
                e.secondary_drill,
                a.secondary_drill,
                "stack.secondary_drill"
            );
            compare!(e.tertiary_drill, a.tertiary_drill, "stack.tertiary_drill");
            compare!(
                e.front_post_machining,
                a.front_post_machining,
                "stack.front_post_machining"
            );
            compare!(
                e.back_post_machining,
                a.back_post_machining,
                "stack.back_post_machining"
            );
        }
        (e, a) => compare!(e.is_some(), a.is_some(), "stack present"),
    }
    out
}

fn changed_domains(
    current: &kiapi::board::types::FootprintInstance,
    updated: &kiapi::board::types::FootprintInstance,
) -> Result<BTreeSet<ChangedDomain>> {
    let mut changed = BTreeSet::new();
    let current_definition = current
        .definition
        .as_ref()
        .context("missing current definition")?;
    let updated_definition = updated
        .definition
        .as_ref()
        .context("missing updated definition")?;

    let current_pads = normalized_items(current_definition, "Pad")?;
    let updated_pads = normalized_items(updated_definition, "Pad")?;
    if current_pads != updated_pads {
        changed.insert(ChangedDomain::Pads);
    }
    let current_graphics = normalized_graphics(current_definition)?;
    let updated_graphics = normalized_graphics(updated_definition)?;
    if current_graphics != updated_graphics {
        changed.insert(ChangedDomain::Graphics);
    }
    if konnect_ipc::KiCadIpcClient::footprint_3d_models(current)?
        != konnect_ipc::KiCadIpcClient::footprint_3d_models(updated)?
    {
        changed.insert(ChangedDomain::Models);
    }
    if current_definition.attributes != updated_definition.attributes
        || field_text(&current_definition.datasheet_field)
            != field_text(&updated_definition.datasheet_field)
        || field_text(&current_definition.description_field)
            != field_text(&updated_definition.description_field)
        || normalized_items(current_definition, "Field")?
            != normalized_items(updated_definition, "Field")?
    {
        changed.insert(ChangedDomain::Metadata);
    }
    if library_owned_attributes(current.attributes.as_ref())
        != library_owned_attributes(updated.attributes.as_ref())
    {
        changed.insert(ChangedDomain::Attributes);
    }
    Ok(changed)
}

fn normalized_items(
    definition: &kiapi::board::types::Footprint,
    suffix: &str,
) -> Result<Vec<Vec<u8>>> {
    let mut items = definition
        .items
        .iter()
        .filter(|item| item.type_url.ends_with(suffix))
        .map(|item| {
            if suffix == "Pad" {
                let mut pad = kiapi::board::types::Pad::decode(item.value.as_slice())?;
                pad.id = None;
                pad.parent = None;
                pad.net = None;
                pad.pad_to_die_length = None;
                pad.symbol_pin = None;
                pad.pad_to_die_delay = None;
                if let Some(stack) = pad.pad_stack.as_mut() {
                    stack.layers.sort_unstable();
                    // A drill without a diameter is no hole, whatever layers it names: KiCad
                    // 10.0.6 reports an SMD pad's absent drill as a 0 x 0 drill from F.Cu
                    // to B.Cu, where the state read before the update carries none.
                    if stack.drill.as_ref().is_some_and(|drill| {
                        drill
                            .diameter
                            .as_ref()
                            .is_none_or(|diameter| diameter.x_nm == 0 && diameter.y_nm == 0)
                    }) {
                        stack.drill = None;
                    }
                    // Parameters that do not apply to a layer's shape (documented as such in
                    // the API) are KiCad's own defaults when read back: 0.25 / 0.2 corner and
                    // chamfer ratios and a circle anchor on a plain rectangle. They carry no
                    // meaning, so they are compared as one value.
                    for layer in stack.copper_layers.iter_mut() {
                        use kiapi::board::types::PadStackShape as Shape;
                        let shape = layer.shape;
                        if shape != Shape::PssRoundrect as i32
                            && shape != Shape::PssChamferedrect as i32
                        {
                            layer.corner_rounding_ratio = 0.0;
                        }
                        if shape != Shape::PssChamferedrect as i32 {
                            layer.chamfer_ratio = 0.0;
                            layer.chamfered_corners = None;
                        }
                        if shape != Shape::PssCustom as i32 {
                            layer.custom_anchor_shape = Shape::PssUnknown as i32;
                            layer.custom_shapes.clear();
                        }
                        if shape != Shape::PssTrapezoid as i32 {
                            layer.trapezoid_delta = None;
                        }
                    }
                    if stack.zone_settings.as_ref().is_some_and(|settings| {
                        settings.zone_connection
                            == kiapi::board::types::ZoneConnectionStyle::ZcsInherited as i32
                            && settings.thermal_spokes.as_ref().is_none_or(|spokes| {
                                spokes.width.is_none()
                                    && spokes.gap.is_none()
                                    && spokes
                                        .angle
                                        .as_ref()
                                        .is_none_or(|angle| angle.value_degrees == 90.0)
                            })
                    }) {
                        stack.zone_settings = None;
                    }
                }
                Ok(pad.encode_to_vec())
            } else if suffix == "Field" {
                let mut field = kiapi::board::types::Field::decode(item.value.as_slice())?;
                field.id = None;
                if let Some(text) = field.text.as_mut() {
                    text.id = None;
                }
                Ok(field.encode_to_vec())
            } else {
                Ok(item.value.clone())
            }
        })
        .collect::<Result<Vec<_>>>()?;
    items.sort();
    Ok(items)
}

fn normalized_graphics(definition: &kiapi::board::types::Footprint) -> Result<Vec<Vec<u8>>> {
    let mut graphics = definition
        .items
        .iter()
        .filter_map(|item| {
            if item.type_url.ends_with("BoardGraphicShape") {
                Some(
                    kiapi::board::types::BoardGraphicShape::decode(item.value.as_slice()).map(
                        |mut shape| {
                            shape.id = None;
                            shape.net = None;
                            shape.locked = kiapi::common::types::LockedState::LsUnlocked as i32;
                            if let Some(graphic) = shape.shape.as_mut() {
                                if let Some(stroke) = graphic
                                    .attributes
                                    .as_mut()
                                    .and_then(|attributes| attributes.stroke.as_mut())
                                {
                                    if matches!(
                                        stroke.style(),
                                        kiapi::common::types::StrokeLineStyle::SlsUnknown
                                            | kiapi::common::types::StrokeLineStyle::SlsDefault
                                            | kiapi::common::types::StrokeLineStyle::SlsSolid
                                    ) {
                                        stroke.style =
                                            kiapi::common::types::StrokeLineStyle::SlsSolid as i32;
                                    }
                                    stroke.color = None;
                                }
                                if let Some(
                                    kiapi::common::types::graphic_shape::Geometry::Polygon(polyset),
                                ) = graphic.geometry.as_mut()
                                {
                                    normalize_polyset(polyset);
                                }
                                if let Some(
                                    kiapi::common::types::graphic_shape::Geometry::Rectangle(
                                        rectangle,
                                    ),
                                ) = graphic.geometry.as_mut()
                                {
                                    if rectangle
                                        .corner_radius
                                        .as_ref()
                                        .is_some_and(|radius| radius.value_nm == 0)
                                    {
                                        rectangle.corner_radius = None;
                                    }
                                }
                            }
                            shape.encode_to_vec()
                        },
                    ),
                )
            } else if item.type_url.ends_with("BoardText") {
                Some(
                    kiapi::board::types::BoardText::decode(item.value.as_slice()).map(
                        |mut text| {
                            text.id = None;
                            text.encode_to_vec()
                        },
                    ),
                )
            } else {
                None
            }
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    graphics.sort();
    Ok(graphics)
}

fn normalize_polyset(polyset: &mut kiapi::common::types::PolySet) {
    for polygon in &mut polyset.polygons {
        if let Some(outline) = polygon.outline.as_mut() {
            normalize_polyline(outline);
        }
        for hole in &mut polygon.holes {
            normalize_polyline(hole);
        }
        polygon.holes.sort_by_key(prost::Message::encode_to_vec);
    }
    polyset.polygons.sort_by_key(prost::Message::encode_to_vec);
}

fn normalize_polyline(polyline: &mut kiapi::common::types::PolyLine) {
    if !polyline.closed || polyline.nodes.len() < 2 {
        return;
    }
    let candidates = [
        polyline.nodes.clone(),
        polyline.nodes.iter().cloned().rev().collect(),
    ];
    let mut best: Option<(Vec<u8>, Vec<kiapi::common::types::PolyLineNode>)> = None;
    for nodes in candidates {
        for offset in 0..nodes.len() {
            let rotated = nodes[offset..]
                .iter()
                .chain(nodes[..offset].iter())
                .cloned()
                .collect::<Vec<_>>();
            let encoded = rotated
                .iter()
                .flat_map(prost::Message::encode_to_vec)
                .collect::<Vec<_>>();
            if best
                .as_ref()
                .is_none_or(|(best_encoded, _)| encoded < *best_encoded)
            {
                best = Some((encoded, rotated));
            }
        }
    }
    if let Some((_, nodes)) = best {
        polyline.nodes = nodes;
    }
}

fn library_owned_attributes(
    attributes: Option<&kiapi::board::types::FootprintAttributes>,
) -> Option<(bool, bool, bool, bool, i32, bool)> {
    attributes.map(|attributes| {
        let mounting_style = match attributes.mounting_style() {
            kiapi::board::types::FootprintMountingStyle::FmsUnknown
            | kiapi::board::types::FootprintMountingStyle::FmsUnspecified => {
                kiapi::board::types::FootprintMountingStyle::FmsUnspecified as i32
            }
            style => style as i32,
        };
        (
            attributes.exclude_from_position_files,
            attributes.exclude_from_bill_of_materials,
            attributes.exempt_from_courtyard_requirement,
            attributes.allow_soldermask_bridges,
            mounting_style,
            attributes.not_in_schematic,
        )
    })
}

fn field_text(field: &Option<kiapi::board::types::Field>) -> String {
    field
        .as_ref()
        .and_then(|field| field.text.as_ref())
        .and_then(|text| text.text.as_ref())
        .map(|text| text.text.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #[test]
    fn ailab48_zone_connect_tokens_map_to_kicads_own_meaning() {
        use kiapi::board::types::ZoneConnectionStyle as Style;
        // Verified natively: KiCad 10.0.6 reads token 2 as FULL and its saved board
        // reported THERMAL after the previous (shifted) mapping was applied.
        for (token, expected) in [
            (0, Style::ZcsNone),
            (1, Style::ZcsThermal),
            (2, Style::ZcsFull),
            (3, Style::ZcsPthThermal),
        ] {
            let pad = konnect_sexp::parse_sexp(&format!("(pad 1 smd rect (zone_connect {token}))"))
                .unwrap();
            let parsed = PadOverrides::parse(&pad).unwrap();
            assert_eq!(parsed.zone, Some(expected as i32), "token {token}");
        }
        let absent =
            PadOverrides::parse(&konnect_sexp::parse_sexp("(pad 1 smd rect)").unwrap()).unwrap();
        assert_eq!(
            absent.zone, None,
            "no token means inherited: nothing is overridden"
        );
    }

    #[test]
    fn ailab48_shape_irrelevant_pad_parameters_do_not_count_as_a_difference() {
        use kiapi::board::types::{
            BoardLayer, Footprint, Pad, PadStack, PadStackLayer, PadStackShape, PadType,
        };
        use kiapi::common::types::Vector2;
        use prost::Message;
        let definition =
            |shape: PadStackShape, corner: f64, chamfer: f64, anchor: PadStackShape| {
                let layer = PadStackLayer {
                    layer: BoardLayer::BlFCu as i32,
                    shape: shape as i32,
                    size: Some(Vector2 {
                        x_nm: 1_000_000,
                        y_nm: 1_200_000,
                    }),
                    corner_rounding_ratio: corner,
                    chamfer_ratio: chamfer,
                    custom_anchor_shape: anchor as i32,
                    ..Default::default()
                };
                let pad = Pad {
                    r#type: PadType::PtSmd as i32,
                    pad_stack: Some(PadStack {
                        copper_layers: vec![layer],
                        ..Default::default()
                    }),
                    ..Default::default()
                };
                Footprint {
                    items: vec![prost_types::Any {
                        type_url: "type.googleapis.com/kiapi.board.types.Pad".into(),
                        value: pad.encode_to_vec(),
                    }],
                    ..Default::default()
                }
            };
        let pads = |d: &Footprint| super::normalized_items(d, "Pad").unwrap();
        // Observed natively on KiCad 10.0.6: a plain rectangle reads back with 0.25 / 0.2 / circle.
        let rect_expected = definition(
            PadStackShape::PssRectangle,
            0.0,
            0.0,
            PadStackShape::PssRectangle,
        );
        let rect_native = definition(
            PadStackShape::PssRectangle,
            0.25,
            0.2,
            PadStackShape::PssCircle,
        );
        assert_eq!(pads(&rect_expected), pads(&rect_native));
        // Where the parameter applies, a different value is a real difference.
        let round_a = definition(
            PadStackShape::PssRoundrect,
            0.25,
            0.0,
            PadStackShape::PssRectangle,
        );
        let round_b = definition(
            PadStackShape::PssRoundrect,
            0.1,
            0.0,
            PadStackShape::PssRectangle,
        );
        assert_ne!(pads(&round_a), pads(&round_b));
        let chamfer_a = definition(
            PadStackShape::PssChamferedrect,
            0.1,
            0.2,
            PadStackShape::PssRectangle,
        );
        let chamfer_b = definition(
            PadStackShape::PssChamferedrect,
            0.1,
            0.3,
            PadStackShape::PssRectangle,
        );
        assert_ne!(pads(&chamfer_a), pads(&chamfer_b));
        // A different shape is still a difference.
        assert_ne!(pads(&rect_expected), pads(&round_a));
    }

    #[test]
    fn ailab48_zero_diameter_drill_is_no_drill_in_pad_comparison() {
        use kiapi::board::types::{BoardLayer, DrillProperties, Footprint, Pad, PadStack, PadType};
        use kiapi::common::types::Vector2;
        use prost::Message;
        let definition = |drill: Option<DrillProperties>| {
            let pad = Pad {
                r#type: PadType::PtSmd as i32,
                pad_stack: Some(PadStack {
                    drill,
                    ..Default::default()
                }),
                ..Default::default()
            };
            Footprint {
                items: vec![prost_types::Any {
                    type_url: "type.googleapis.com/kiapi.board.types.Pad".into(),
                    value: pad.encode_to_vec(),
                }],
                ..Default::default()
            }
        };
        let drill = |diameter: i64| DrillProperties {
            start_layer: BoardLayer::BlFCu as i32,
            end_layer: BoardLayer::BlBCu as i32,
            diameter: Some(Vector2 {
                x_nm: diameter,
                y_nm: diameter,
            }),
            ..Default::default()
        };
        // Native KiCad: absent drill is reported as a 0 x 0 drill from F.Cu to B.Cu.
        assert_eq!(
            super::normalized_items(&definition(None), "Pad").unwrap(),
            super::normalized_items(&definition(Some(drill(0))), "Pad").unwrap()
        );
        // A real hole, or a different hole, is still a difference.
        assert_ne!(
            super::normalized_items(&definition(None), "Pad").unwrap(),
            super::normalized_items(&definition(Some(drill(800_000))), "Pad").unwrap()
        );
        assert_ne!(
            super::normalized_items(&definition(Some(drill(800_000))), "Pad").unwrap(),
            super::normalized_items(&definition(Some(drill(900_000))), "Pad").unwrap()
        );
    }

    #[test]
    fn ailab48_unset_mounting_style_is_not_a_readback_difference() {
        use kiapi::board::types::{FootprintAttributes, FootprintInstance, FootprintMountingStyle};
        // Observed natively on KiCad 10.0.6: after an update the instance reports
        // FmsUnspecified where the state read before it carried FmsUnknown.
        let with_style = |style: FootprintMountingStyle, locked: bool| FootprintInstance {
            attributes: Some(FootprintAttributes {
                mounting_style: style as i32,
                ..Default::default()
            }),
            locked: if locked {
                kiapi::common::types::LockedState::LsLocked as i32
            } else {
                kiapi::common::types::LockedState::LsUnlocked as i32
            },
            ..Default::default()
        };
        let unknown = with_style(FootprintMountingStyle::FmsUnknown, false);
        let unspecified = with_style(FootprintMountingStyle::FmsUnspecified, false);
        assert!(super::instance_state_matches(&unknown, &unspecified));
        // A real mounting-style change, or any other instance state change, still differs.
        let smd = with_style(FootprintMountingStyle::FmsSmd, false);
        assert!(!super::instance_state_matches(&unknown, &smd));
        let locked = with_style(FootprintMountingStyle::FmsUnknown, true);
        assert!(!super::instance_state_matches(&unknown, &locked));
        let summary = super::readback_difference_summary(&unknown, &locked);
        assert!(summary.starts_with("instance:"), "{summary}");
    }

    use super::*;
    use konnect_ipc::builders;
    use konnect_ipc::gen::kiapi;
    use prost::Message;
    use std::collections::{BTreeMap, BTreeSet};

    /// The serialization KiCad 10 actually writes — produced by running this
    /// module's hand-written fixture through `kicad-cli fp upgrade` — parses,
    /// and the two universal flags KiCad stamps into every saved footprint
    /// (`embedded_fonts no`, `duplicate_pad_numbers_are_jumpers no`, plus
    /// `remove_unused_layers no` on pads) do not refuse it. The hand-written
    /// fixture alone cannot catch this class: it shares this module's own
    /// assumptions about the format (see the fixtures-from-KiCad rule in
    /// CONTRIBUTING).
    #[test]
    fn a_footprint_kicad_saved_is_accepted_not_refused() {
        let source = KICAD_LIBRARY_FOOTPRINT;
        let library = parse_library_footprint("Konnect:Socket", source)
            .expect("KiCad's own serialization of the fixture must parse");
        assert_eq!(library.pads.len(), 4, "two '1' variants plus '2' and '3'");
        assert_eq!(
            library.graphics.len(),
            3,
            "one fp_line, one fp_poly, and KiCad's standard fab reference text"
        );
        assert!(library.graphics.iter().any(|graphic| {
            matches!(
                graphic,
                konnect_ipc::IpcGraphicDefinition::Text {
                    text,
                    position,
                    rotation,
                    layer,
                    size,
                    stroke_width_mm,
                } if text == "${REFERENCE}"
                    && *position == (0.0, 1.5)
                    && *rotation == 0.0
                    && layer == "F.Fab"
                    && *size == 1.0
                    && *stroke_width_mm == 0.15
            )
        }));
        assert_eq!(library.datasheet.as_deref(), Some("new-datasheet.pdf"));
        assert_eq!(
            library
                .properties
                .iter()
                .map(|property| (property.name.as_str(), field_text_value(property)))
                .collect::<Vec<_>>(),
            vec![
                ("KiLib_Generator", "konnect_test_generator"),
                ("AssemblyVendor", "Example Assembly"),
            ]
        );
        assert!(library.properties.iter().all(|property| !property.visible));
        assert_eq!(library.models.len(), 1);

        // The same flags at a non-default value carry semantics the typed
        // rebuild cannot represent, so they refuse rather than silently drop.
        for (from, to) in [
            ("(embedded_fonts no)", "(embedded_fonts yes)"),
            ("(remove_unused_layers no)", "(remove_unused_layers yes)"),
        ] {
            let flipped = source.replace(from, to);
            assert_ne!(flipped, source, "fixture must contain {from}");
            let error = parse_library_footprint("Konnect:Socket", &flipped)
                .expect_err("a non-default flag must refuse")
                .to_string();
            assert!(
                error.contains("not supported"),
                "refusal names the unsupported clause: {error}"
            );
        }
    }

    #[test]
    fn stock_kicad_generator_property_is_preserved_as_a_typed_field() {
        // Unmodified KiCad 10.0.5 standard-library output from
        // Capacitor_SMD.pretty/C_0603_1608Metric.kicad_mod. This is the stock
        // footprint that exposed #373; keeping its real generator-authored
        // shape prevents a synthetic fixture from agreeing with this parser.
        let source = include_str!("../../tests/fixtures/c_0603_1608metric_kicad10.kicad_mod");
        let library = parse_library_footprint("Capacitor_SMD:C_0603_1608Metric", source)
            .expect("the stock KiCad generator footprint must parse losslessly");

        assert_eq!(library.pads.len(), 2);
        let generator = library
            .properties
            .iter()
            .find(|property| property.name == "KiLib_Generator")
            .expect("KiCad's generator property must remain a typed Field");
        assert_eq!(field_text_value(generator), "SMD_2terminal_chip_molded");
        assert!(!generator.visible);
        assert_eq!(library.models.len(), 1);
    }

    #[test]
    fn visible_property_is_a_field_only_and_clause_order_does_not_change_its_angle() {
        let source = KICAD_LIBRARY_FOOTPRINT.replace(
            "\t\t(at 0.5 0.75 15)\n\t\t(layer \"F.Fab\")\n\t\t(hide yes)",
            "\t\t(layer \"F.Fab\")\n\t\t(hide no)\n\t\t(at 0.5 0.75 15)",
        );
        assert_ne!(source, KICAD_LIBRARY_FOOTPRINT);

        let library = parse_library_footprint("Konnect:Socket", &source).unwrap();
        let property = library
            .properties
            .iter()
            .find(|property| property.name == "AssemblyVendor")
            .unwrap();
        assert!(property.visible);
        assert_eq!(
            property
                .text
                .as_ref()
                .unwrap()
                .text
                .as_ref()
                .unwrap()
                .attributes
                .as_ref()
                .unwrap()
                .angle
                .as_ref()
                .unwrap()
                .value_degrees,
            15.0
        );
        assert!(!library.graphics.iter().any(|graphic| matches!(
            graphic,
            konnect_ipc::IpcGraphicDefinition::Text { text, .. }
                if text == "Example Assembly"
        )));
    }

    /// `preserved` must be a comparison of the rebuilt instance against the
    /// board's, never a policy constant: when the two instances genuinely
    /// diverge, the flags have to say so.
    #[test]
    fn preserved_flags_come_from_comparison_not_policy() {
        let pad = |number: &str, net: Option<&str>| {
            let item = kiapi::board::types::Pad {
                number: number.to_string(),
                net: net.map(|name| kiapi::board::types::Net {
                    code: Some(kiapi::board::types::NetCode { value: 3 }),
                    name: name.to_string(),
                }),
                ..Default::default()
            };
            builders::pack_any(&item, "kiapi.board.types.Pad")
        };
        let current = kiapi::board::types::FootprintInstance {
            position: Some(builders::vec2(10.0, 20.0)),
            orientation: Some(kiapi::common::types::Angle {
                value_degrees: 90.0,
            }),
            layer: kiapi::board::types::BoardLayer::BlFCu as i32,
            definition: Some(kiapi::board::types::Footprint {
                items: vec![pad("1", Some("GND"))],
                ..Default::default()
            }),
            ..Default::default()
        };
        let old_nets = BTreeMap::from([(
            "1".to_string(),
            kiapi::board::types::Net {
                code: Some(kiapi::board::types::NetCode { value: 3 }),
                name: "GND".to_string(),
            },
        )]);

        let identical = PreservedState::derive(&current, &current.clone(), &old_nets);
        assert!(
            identical.position
                && identical.rotation
                && identical.layer
                && identical.locked
                && identical.kiid
                && identical.symbol_path
                && identical.pad_nets
                && identical.instance_overrides,
            "an untouched instance preserves everything: {identical:?}"
        );

        let mut moved = current.clone();
        moved.position = Some(builders::vec2(99.0, 99.0));
        moved.definition.as_mut().unwrap().items = vec![pad("1", None)];
        let diverged = PreservedState::derive(&current, &moved, &old_nets);
        assert!(
            !diverged.position,
            "a moved instance must report position unpreserved"
        );
        assert!(
            !diverged.pad_nets,
            "a dropped pad net must report pad_nets unpreserved"
        );
        assert!(
            diverged.rotation && diverged.layer && diverged.kiid,
            "untouched fields stay preserved: {diverged:?}"
        );
    }

    const KICAD_LIBRARY_FOOTPRINT: &str =
        include_str!("../../tests/fixtures/socket_kicad10.kicad_mod");

    const LIBRARY_FOOTPRINT: &str = r#"
(footprint "Socket"
  (version 20240108)
  (generator "konnect")
  (layer "F.Cu")
  (descr "updated description")
  (tags "keyboard socket")
  (attr smd exclude_from_pos_files)
  (fp_line (start -1 -2) (end 3 4)
    (stroke (width 0.12) (type solid))
    (layer "F.SilkS"))
  (fp_poly (pts (xy -2 -1) (xy 2 -1) (xy 2 1) (xy -2 1))
    (stroke (width 0.05) (type solid))
    (fill none)
    (layer "B.CrtYd"))
  (pad "1" smd roundrect (at -2 0 15) (size 2 1)
    (layers "B.Cu" "B.Paste" "B.Mask")
    (roundrect_rratio 0.2))
  (pad "1" smd rect (at -1 0) (size 1 1) (layers "B.Cu"))
  (pad "2" thru_hole circle (at 2 0) (size 3 3)
    (layers "*.Cu" "*.Mask") (drill 1))
  (pad "3" smd rect (at 0 3) (size 1 1) (layers "F.Cu"))
  (fp_text reference "REF**" (at 0 -4 0) (layer "F.SilkS")
    (effects (font (size 1 1) (thickness 0.15))))
  (fp_text value "Socket" (at 0 4 0) (layer "F.Fab")
    (effects (font (size 1 1) (thickness 0.15))))
  (fp_text user "${REFERENCE}" (at 0 1.5 0) (layer "F.Fab")
    (uuid "f96c2efe-5925-4f74-81d2-f89a56f57e13")
    (effects (font (size 0.8 0.8) (thickness 0.11))))
  (property "Datasheet" "new-datasheet.pdf" (at 0 0) (layer "F.Fab") (hide yes))
  (property "Description" "new field description" (at 0 0) (layer "F.Fab") (hide yes))
  (model "../models/Socket.step"
    (offset (xyz 1 2 3))
    (scale (xyz -1 1 1))
    (rotate (xyz 90 0 45)))
)
"#;

    fn field(name: &str, value: &str, x: f64, y: f64, visible: bool) -> kiapi::board::types::Field {
        kiapi::board::types::Field {
            name: name.to_string(),
            visible,
            text: Some(kiapi::board::types::BoardText {
                text: Some(kiapi::common::types::Text {
                    position: Some(builders::vec2(x, y)),
                    attributes: Some(kiapi::common::types::TextAttributes {
                        size: Some(builders::vec2(1.25, 1.25)),
                        angle: Some(kiapi::common::types::Angle {
                            value_degrees: 17.0,
                        }),
                        mirrored: true,
                        ..Default::default()
                    }),
                    text: value.to_string(),
                    ..Default::default()
                }),
                layer: kiapi::board::types::BoardLayer::BlBSilkS as i32,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn current_pad(number: &str, net_name: &str, net_code: i32) -> prost_types::Any {
        builders::pack_any(
            &kiapi::board::types::Pad {
                id: Some(kiapi::common::types::Kiid {
                    value: format!("old-pad-{number}-{net_code}"),
                }),
                number: number.to_string(),
                net: Some(kiapi::board::types::Net {
                    code: Some(kiapi::board::types::NetCode { value: net_code }),
                    name: net_name.to_string(),
                }),
                position: Some(builders::vec2(100.0, 50.0)),
                ..Default::default()
            },
            "kiapi.board.types.Pad",
        )
    }

    fn current_instance(
        layer: kiapi::board::types::BoardLayer,
    ) -> kiapi::board::types::FootprintInstance {
        let reference = field("Reference", "SW1", 101.0, 48.0, true);
        let value = field("Value", "Socket Value", 99.0, 52.0, false);
        kiapi::board::types::FootprintInstance {
            id: Some(kiapi::common::types::Kiid {
                value: "instance-kiid".to_string(),
            }),
            position: Some(builders::vec2(100.0, 50.0)),
            orientation: Some(kiapi::common::types::Angle {
                value_degrees: 37.0,
            }),
            layer: layer as i32,
            locked: kiapi::common::types::LockedState::LsLocked as i32,
            definition: Some(kiapi::board::types::Footprint {
                id: Some(kiapi::common::types::LibraryIdentifier {
                    library_nickname: "Test".to_string(),
                    entry_name: "Socket".to_string(),
                }),
                reference_field: Some(reference.clone()),
                value_field: Some(value.clone()),
                items: vec![current_pad("1", "ROW1", 11), current_pad("2", "COL1", 12)],
                ..Default::default()
            }),
            reference_field: Some(reference),
            value_field: Some(value),
            datasheet_field: Some(field("Datasheet", "placed-datasheet", 0.0, 0.0, false)),
            description_field: Some(field("Description", "placed-description", 0.0, 0.0, false)),
            attributes: Some(kiapi::board::types::FootprintAttributes {
                not_in_schematic: false,
                exclude_from_bill_of_materials: true,
                do_not_populate: true,
                ..Default::default()
            }),
            overrides: Some(kiapi::board::types::FootprintDesignRuleOverrides {
                copper_clearance: Some(builders::distance(0.3)),
                ..Default::default()
            }),
            symbol_path: Some(kiapi::common::types::SheetPath {
                path: vec![kiapi::common::types::Kiid {
                    value: "sheet-kiid".to_string(),
                }],
                path_human_readable: "/Keyboard".to_string(),
            }),
            symbol_sheet_name: "Keyboard".to_string(),
            symbol_sheet_filename: "keyboard.kicad_sch".to_string(),
            symbol_footprint_filters: "Test:*".to_string(),
            parent: None,
        }
    }

    fn decoded_pads(
        instance: &kiapi::board::types::FootprintInstance,
    ) -> Vec<kiapi::board::types::Pad> {
        instance
            .definition
            .as_ref()
            .unwrap()
            .items
            .iter()
            .filter(|item| item.type_url.ends_with("kiapi.board.types.Pad"))
            .map(|item| kiapi::board::types::Pad::decode(item.value.as_slice()).unwrap())
            .collect()
    }

    fn decoded_custom_fields(
        instance: &kiapi::board::types::FootprintInstance,
    ) -> Vec<kiapi::board::types::Field> {
        instance
            .definition
            .as_ref()
            .unwrap()
            .items
            .iter()
            .filter(|item| item.type_url.ends_with("kiapi.board.types.Field"))
            .map(|item| {
                kiapi::board::types::Field::decode(item.value.as_slice())
                    .expect("custom field must decode")
            })
            .collect()
    }

    fn field_text_value(field: &kiapi::board::types::Field) -> &str {
        field
            .text
            .as_ref()
            .and_then(|text| text.text.as_ref())
            .map(|text| text.text.as_str())
            .unwrap_or_default()
    }

    #[test]
    fn parses_supported_library_definition_without_dropping_domains() {
        let library = parse_library_footprint("Test:Socket", LIBRARY_FOOTPRINT).unwrap();

        assert_eq!(library.library_id, "Test:Socket");
        assert_eq!(
            library.definition.attributes.as_ref().unwrap().description,
            "updated description"
        );
        assert_eq!(
            library.definition.attributes.as_ref().unwrap().keywords,
            "keyboard socket"
        );
        assert_eq!(library.pads.len(), 4);
        assert_eq!(library.graphics.len(), 3);
        assert!(library.graphics.iter().any(|graphic| matches!(
            graphic,
            konnect_ipc::IpcGraphicDefinition::Text {
                text,
                size,
                stroke_width_mm,
                ..
            } if text == "${REFERENCE}" && *size == 0.8 && *stroke_width_mm == 0.11
        )));
        assert_eq!(library.models.len(), 1);
        let model = &library.models[0];
        assert_eq!(model.filename, "../models/Socket.step");
        assert_eq!(model.offset.as_ref().unwrap().x_nm, 1.0);
        assert_eq!(model.scale.as_ref().unwrap().x_nm, -1.0);
        assert_eq!(model.rotation.as_ref().unwrap().z_nm, 45.0);
    }

    #[test]
    fn merge_preserves_instance_state_and_nets_by_logical_pad_number() {
        let current = current_instance(kiapi::board::types::BoardLayer::BlBCu);
        let library = parse_library_footprint("Test:Socket", KICAD_LIBRARY_FOOTPRINT).unwrap();
        let prepared = build_updated_instance(
            &current,
            &library,
            &BTreeMap::from([("ROW1".to_string(), 11), ("COL1".to_string(), 12)]),
            &BTreeSet::from(["ROW1".to_string(), "COL1".to_string()]),
        )
        .unwrap();
        let updated =
            kiapi::board::types::FootprintInstance::decode(prepared.item.value.as_slice()).unwrap();

        assert_eq!(updated.id, current.id);
        assert_eq!(updated.position, current.position);
        assert_eq!(updated.orientation, current.orientation);
        assert_eq!(updated.layer, current.layer);
        assert_eq!(updated.locked, current.locked);
        assert_eq!(updated.reference_field, current.reference_field);
        assert_eq!(updated.value_field, current.value_field);
        assert_eq!(field_text(&updated.datasheet_field), "new-datasheet.pdf");
        assert_eq!(
            updated
                .datasheet_field
                .as_ref()
                .and_then(|field| field.text.as_ref())
                .and_then(|text| text.text.as_ref())
                .and_then(|text| text.attributes.as_ref()),
            current
                .datasheet_field
                .as_ref()
                .and_then(|field| field.text.as_ref())
                .and_then(|text| text.text.as_ref())
                .and_then(|text| text.attributes.as_ref())
        );
        assert_eq!(
            field_text(&updated.description_field),
            "new field description"
        );
        let attributes = updated.attributes.as_ref().unwrap();
        let current_attributes = current.attributes.as_ref().unwrap();
        assert_eq!(
            attributes.not_in_schematic,
            current_attributes.not_in_schematic
        );
        assert_eq!(
            attributes.do_not_populate,
            current_attributes.do_not_populate
        );
        assert!(attributes.exclude_from_position_files);
        assert!(!attributes.exclude_from_bill_of_materials);
        assert_eq!(
            attributes.mounting_style,
            kiapi::board::types::FootprintMountingStyle::FmsSmd as i32
        );
        assert_eq!(updated.overrides, current.overrides);
        assert_eq!(updated.symbol_path, current.symbol_path);
        assert_eq!(updated.symbol_sheet_name, current.symbol_sheet_name);
        assert_eq!(updated.symbol_sheet_filename, current.symbol_sheet_filename);
        assert_eq!(
            updated.symbol_footprint_filters,
            current.symbol_footprint_filters
        );

        let properties = decoded_custom_fields(&updated);
        assert_eq!(
            properties
                .iter()
                .map(|property| (property.name.as_str(), field_text_value(property)))
                .collect::<Vec<_>>(),
            vec![
                ("KiLib_Generator", "konnect_test_generator"),
                ("AssemblyVendor", "Example Assembly"),
            ]
        );
        let assembly = properties
            .iter()
            .find(|property| property.name == "AssemblyVendor")
            .unwrap();
        let assembly_text = assembly.text.as_ref().unwrap();
        assert_eq!(
            assembly_text.layer,
            kiapi::board::types::BoardLayer::BlBFab as i32
        );
        let text = assembly_text.text.as_ref().unwrap();
        let expected = konnect_sexp::geometry::transform_pad(0.5, -0.75, 100.0, 50.0, 37.0);
        let actual = text.position.as_ref().unwrap();
        let actual = (
            builders::nm_to_mm(actual.x_nm),
            builders::nm_to_mm(actual.y_nm),
        );
        assert!((actual.0 - expected.0).abs() <= 0.000_001);
        assert!((actual.1 - expected.1).abs() <= 0.000_001);
        let attributes = text.attributes.as_ref().unwrap();
        assert!(attributes.mirrored);
        assert_eq!(
            attributes.angle.as_ref().unwrap().value_degrees,
            readable_property_angle(180.0 - 15.0 + 37.0)
        );

        let pads = decoded_pads(&updated);
        assert_eq!(pads.iter().filter(|pad| pad.number == "1").count(), 2);
        assert!(pads.iter().filter(|pad| pad.number == "1").all(|pad| pad
            .net
            .as_ref()
            .map(|net| net.name.as_str())
            == Some("ROW1")));
        assert_eq!(
            pads.iter()
                .find(|pad| pad.number == "2")
                .and_then(|pad| pad.net.as_ref())
                .map(|net| net.name.as_str()),
            Some("COL1")
        );
        assert!(pads
            .iter()
            .find(|pad| pad.number == "3")
            .unwrap()
            .net
            .is_none());
        let flipped_pad = pads
            .iter()
            .find(|pad| pad.number == "3")
            .expect("new pad 3");
        let flipped_stack = flipped_pad.pad_stack.as_ref().expect("pad stack");
        assert_eq!(
            flipped_stack.layers,
            vec![kiapi::board::types::BoardLayer::BlBCu as i32],
            "a front-copper library pad moves to back copper with a B.Cu instance"
        );
        let expected = konnect_sexp::geometry::transform_pad(0.0, -3.0, 100.0, 50.0, 37.0);
        let actual = flipped_pad.position.as_ref().expect("pad position");
        let actual = (
            builders::nm_to_mm(actual.x_nm),
            builders::nm_to_mm(actual.y_nm),
        );
        assert!((actual.0 - expected.0).abs() <= 0.000_001);
        assert!((actual.1 - expected.1).abs() <= 0.000_001);
        assert!(prepared.changed_domains.contains(&ChangedDomain::Pads));
        assert!(prepared.changed_domains.contains(&ChangedDomain::Graphics));
        assert!(prepared.changed_domains.contains(&ChangedDomain::Metadata));
        assert!(prepared.changed_domains.contains(&ChangedDomain::Models));
    }

    #[test]
    fn merge_preserves_instance_only_properties_and_refreshes_library_properties() {
        let mut current = current_instance(kiapi::board::types::BoardLayer::BlFCu);
        let instance_only = field("InstanceNote", "keep this", 105.0, 55.0, false);
        let stale_library_property = field("AssemblyVendor", "old library value", 99.0, 49.0, true);
        current.definition.as_mut().unwrap().items.extend([
            builders::pack_any(&instance_only, "kiapi.board.types.Field"),
            builders::pack_any(&stale_library_property, "kiapi.board.types.Field"),
        ]);
        let library = parse_library_footprint("Test:Socket", KICAD_LIBRARY_FOOTPRINT).unwrap();

        let prepared = build_updated_instance(
            &current,
            &library,
            &BTreeMap::from([("ROW1".to_string(), 11), ("COL1".to_string(), 12)]),
            &BTreeSet::new(),
        )
        .unwrap();
        let updated =
            kiapi::board::types::FootprintInstance::decode(prepared.item.value.as_slice()).unwrap();
        let properties = decoded_custom_fields(&updated);

        assert_eq!(
            properties
                .iter()
                .map(|property| property.name.as_str())
                .collect::<Vec<_>>(),
            vec!["InstanceNote", "KiLib_Generator", "AssemblyVendor"]
        );
        assert_eq!(
            properties
                .iter()
                .find(|property| property.name == "InstanceNote")
                .unwrap(),
            &instance_only
        );
        assert_eq!(
            field_text_value(
                properties
                    .iter()
                    .find(|property| property.name == "AssemblyVendor")
                    .unwrap()
            ),
            "Example Assembly"
        );
        assert!(prepared.changed_domains.contains(&ChangedDomain::Metadata));
    }

    #[test]
    fn merge_conflicts_when_library_removes_a_connected_logical_pad() {
        let current = current_instance(kiapi::board::types::BoardLayer::BlFCu);
        let without_pad_two = LIBRARY_FOOTPRINT.replace(
            "  (pad \"2\" thru_hole circle (at 2 0) (size 3 3)\n    (layers \"*.Cu\" \"*.Mask\") (drill 1))\n",
            "",
        );
        let library = parse_library_footprint("Test:Socket", &without_pad_two).unwrap();

        let error = build_updated_instance(
            &current,
            &library,
            &BTreeMap::from([("ROW1".to_string(), 11), ("COL1".to_string(), 12)]),
            &BTreeSet::from(["COL1".to_string()]),
        )
        .unwrap_err();

        assert!(error.to_string().contains("connected pad 2"), "{error:#}");
    }

    #[test]
    fn parser_rejects_unsupported_children_before_any_update_can_be_built() {
        let unsupported = LIBRARY_FOOTPRINT.replace(
            "  (model \"../models/Socket.step\"",
            "  (zone (net 0) (layers \"F.Cu\"))\n  (model \"../models/Socket.step\"",
        );

        let error = parse_library_footprint("Test:Socket", &unsupported).unwrap_err();

        assert!(error.to_string().contains("zone"), "{error:#}");
    }

    #[test]
    fn parser_names_unrepresentable_or_ambiguous_custom_properties() {
        let unsupported = KICAD_LIBRARY_FOOTPRINT.replace(
            "\t(property \"AssemblyVendor\" \"Example Assembly\"\n\t\t(at",
            "\t(property \"AssemblyVendor\" \"Example Assembly\"\n\t\t(unlocked yes)\n\t\t(at",
        );
        assert_ne!(unsupported, KICAD_LIBRARY_FOOTPRINT);
        let error = parse_library_footprint("Test:Socket", &unsupported).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("AssemblyVendor"), "{message}");
        assert!(message.contains("unlocked"), "{message}");

        let duplicate = KICAD_LIBRARY_FOOTPRINT.replace(
            "\"KiLib_Generator\" \"konnect_test_generator\"",
            "\"AssemblyVendor\" \"konnect_test_generator\"",
        );
        assert_ne!(duplicate, KICAD_LIBRARY_FOOTPRINT);
        let error = parse_library_footprint("Test:Socket", &duplicate).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("AssemblyVendor"), "{message}");
        assert!(message.contains("more than once"), "{message}");
    }

    #[test]
    fn mandatory_properties_are_validated_and_duplicate_names_refuse() {
        let mut accepted = Vec::new();
        for (name, value) in [
            ("Reference", "REF**"),
            ("Value", "Socket"),
            ("Datasheet", "new-datasheet.pdf"),
            ("Description", "new field description"),
        ] {
            let authored = format!("\t(property \"{name}\" \"{value}\"\n\t\t(at");
            let unsupported = KICAD_LIBRARY_FOOTPRINT.replace(
                &authored,
                &format!("\t(property \"{name}\" \"{value}\"\n\t\t(unlocked yes)\n\t\t(at"),
            );
            assert_ne!(unsupported, KICAD_LIBRARY_FOOTPRINT);
            match parse_library_footprint("Test:Socket", &unsupported) {
                Ok(_) => accepted.push(format!("{name} unsupported clause")),
                Err(error) => {
                    let message = error.to_string();
                    assert!(message.contains(name), "{message}");
                    assert!(message.contains("unlocked"), "{message}");
                }
            }

            let duplicate = KICAD_LIBRARY_FOOTPRINT.replace(
                "\"KiLib_Generator\" \"konnect_test_generator\"",
                &format!("\"{name}\" \"konnect_test_generator\""),
            );
            assert_ne!(duplicate, KICAD_LIBRARY_FOOTPRINT);
            match parse_library_footprint("Test:Socket", &duplicate) {
                Ok(_) => accepted.push(format!("{name} duplicate name")),
                Err(error) => {
                    let message = error.to_string();
                    assert!(message.contains(name), "{message}");
                    assert!(message.contains("more than once"), "{message}");
                }
            }
        }
        assert!(
            accepted.is_empty(),
            "parser accepted mandatory-property cases that must refuse: {}",
            accepted.join(", ")
        );
    }

    #[test]
    fn repeated_scalar_property_clauses_refuse() {
        let mut accepted = Vec::new();
        let vendor_prefix = "\t(property \"AssemblyVendor\" \"Example Assembly\"\n\t\t(at 0.5 0.75 15)\n\t\t(layer \"F.Fab\")\n\t\t(hide yes)";
        for (clause, replacement) in [
            ("hide", format!("{vendor_prefix}\n\t\t(hide no)")),
            (
                "knockout",
                format!("{vendor_prefix}\n\t\t(knockout yes)\n\t\t(knockout no)"),
            ),
        ] {
            let repeated = KICAD_LIBRARY_FOOTPRINT.replace(vendor_prefix, &replacement);
            assert_ne!(repeated, KICAD_LIBRARY_FOOTPRINT);
            match parse_library_footprint("Test:Socket", &repeated) {
                Ok(_) => accepted.push(clause),
                Err(error) => {
                    let message = error.to_string();
                    assert!(message.contains("AssemblyVendor"), "{message}");
                    assert!(message.contains(clause), "{message}");
                }
            }
        }

        let vendor_font =
            "\t\t\t\t(size 1 1)\n\t\t\t\t(thickness 0.15)\n\t\t\t)\n\t\t)\n\t)\n\t(attr";
        for (clause, repeated_clauses) in [
            ("bold", "\t\t\t\t(bold yes)\n\t\t\t\t(bold no)\n"),
            ("italic", "\t\t\t\t(italic yes)\n\t\t\t\t(italic no)\n"),
            (
                "line_spacing",
                "\t\t\t\t(line_spacing 1.1)\n\t\t\t\t(line_spacing 1.2)\n",
            ),
        ] {
            let replacement =
                vendor_font.replace("\t\t\t)\n", &format!("{repeated_clauses}\t\t\t)\n"));
            let repeated = KICAD_LIBRARY_FOOTPRINT.replace(vendor_font, &replacement);
            assert_ne!(repeated, KICAD_LIBRARY_FOOTPRINT);
            match parse_library_footprint("Test:Socket", &repeated) {
                Ok(_) => accepted.push(clause),
                Err(error) => {
                    let message = error.to_string();
                    assert!(message.contains("AssemblyVendor"), "{message}");
                    assert!(message.contains(clause), "{message}");
                }
            }
        }
        assert!(
            accepted.is_empty(),
            "parser accepted repeated scalar clauses that must refuse: {}",
            accepted.join(", ")
        );
    }

    #[test]
    fn refresh_refuses_an_unrepresentable_layer_before_building_the_update() {
        let unsupported = LIBRARY_FOOTPRINT.replace("(layer \"F.SilkS\"))", "(layer \"In99.Cu\"))");
        let library = parse_library_footprint("Test:Socket", &unsupported).unwrap();
        let current = current_instance(kiapi::board::types::BoardLayer::BlFCu);

        let error = build_updated_instance(
            &current,
            &library,
            &BTreeMap::from([("ROW1".to_string(), 11), ("COL1".to_string(), 12)]),
            &BTreeSet::new(),
        )
        .unwrap_err();

        let message = format!("{error:#}");
        assert!(message.contains("In99.Cu"), "{message}");
        assert!(message.contains("request was not sent"), "{message}");
    }

    #[test]
    fn parser_rejects_lossy_graphic_and_pad_clauses() {
        let dashed = LIBRARY_FOOTPRINT.replace(
            "(stroke (width 0.12) (type solid))",
            "(stroke (width 0.12) (type dash))",
        );
        let error = parse_library_footprint("Test:Socket", &dashed).unwrap_err();
        assert!(error.to_string().contains("stroke type"), "{error:#}");
    }

    #[test]
    fn seventy_two_instances_include_sixty_six_formerly_rejected_pad_settings() {
        for index in 0..72 {
            let source = if index < 66 {
                LIBRARY_FOOTPRINT.replace(
                    "(roundrect_rratio 0.2))",
                    "(roundrect_rratio 0.2) (solder_mask_margin 0.05) (thermal_bridge_angle 45))",
                )
            } else {
                LIBRARY_FOOTPRINT.to_string()
            };
            let library = parse_library_footprint("Test:Socket", &source).unwrap();
            let current = current_instance(kiapi::board::types::BoardLayer::BlFCu);
            let prepared =
                build_updated_instance(&current, &library, &BTreeMap::new(), &BTreeSet::new())
                    .unwrap();
            let updated =
                kiapi::board::types::FootprintInstance::decode(prepared.item.value.as_slice())
                    .unwrap();
            let repeated =
                build_updated_instance(&updated, &library, &BTreeMap::new(), &BTreeSet::new())
                    .unwrap();
            assert!(repeated.changed_domains.is_empty(), "instance {index}");
        }
    }

    #[test]
    fn all_supported_pad_overrides_preserve_exact_values_on_both_sides() {
        let source = LIBRARY_FOOTPRINT.replace("(roundrect_rratio 0.2))",
            "(roundrect_rratio 0.2) (solder_mask_margin 0.075) (solder_paste_margin -0.02) (solder_paste_margin_ratio -0.15) (clearance 0.12) (die_length 0.8) (zone_connect 2) (thermal_bridge_width 0.3) (thermal_gap 0.25) (thermal_bridge_angle 37.5))");
        let library = parse_library_footprint("Test:Socket", &source).unwrap();
        for side in [
            kiapi::board::types::BoardLayer::BlFCu,
            kiapi::board::types::BoardLayer::BlBCu,
        ] {
            for rotation in [0.0, 37.0, 90.0, 180.0, 270.0] {
                let mut current = current_instance(side);
                current.orientation = Some(kiapi::common::types::Angle {
                    value_degrees: rotation,
                });
                let prepared =
                    build_updated_instance(&current, &library, &BTreeMap::new(), &BTreeSet::new())
                        .unwrap();
                let updated =
                    kiapi::board::types::FootprintInstance::decode(prepared.item.value.as_slice())
                        .unwrap();
                let pad = decoded_pads(&updated).remove(0);
                assert_eq!(pad.copper_clearance_override.unwrap().value_nm, 120_000);
                assert_eq!(pad.pad_to_die_length.unwrap().value_nm, 800_000);
                let stack = pad.pad_stack.unwrap();
                for outer in [
                    stack.front_outer_layers.unwrap(),
                    stack.back_outer_layers.unwrap(),
                ] {
                    assert_eq!(
                        outer
                            .solder_mask_settings
                            .unwrap()
                            .solder_mask_margin
                            .unwrap()
                            .value_nm,
                        75_000
                    );
                    let paste = outer.solder_paste_settings.unwrap();
                    assert_eq!(paste.solder_paste_margin.unwrap().value_nm, -20_000);
                    assert_eq!(paste.solder_paste_margin_ratio.unwrap().value, -0.15);
                }
                let zone = stack.zone_settings.unwrap();
                assert_eq!(
                    zone.zone_connection,
                    kiapi::board::types::ZoneConnectionStyle::ZcsFull as i32,
                    "(zone_connect 2) is KiCad's solid/full connection"
                );
                let thermal = zone.thermal_spokes.unwrap();
                assert_eq!(thermal.width.unwrap().value_nm, 300_000);
                assert_eq!(thermal.gap.unwrap().value_nm, 250_000);
                assert_eq!(thermal.angle.unwrap().value_degrees, 37.5);
                assert!(build_updated_instance(
                    &updated,
                    &library,
                    &BTreeMap::new(),
                    &BTreeSet::new()
                )
                .unwrap()
                .changed_domains
                .is_empty());
            }
        }
    }

    #[test]
    fn explicit_zero_and_omitted_pad_overrides_remain_distinct() {
        let omitted = konnect_sexp::parse_sexp("(pad 1 smd rect)").unwrap();
        let explicit = konnect_sexp::parse_sexp("(pad 1 smd rect (solder_mask_margin 0) (solder_paste_margin 0) (solder_paste_margin_ratio 0) (thermal_bridge_angle 0) (thermal_gap 0) (thermal_bridge_width 0) (zone_connect 0) (clearance 0) (die_length 0))").unwrap();
        let absent = PadOverrides::parse(&omitted).unwrap();
        let zero = PadOverrides::parse(&explicit).unwrap();
        assert!(
            absent.mask.is_none()
                && absent.paste.is_none()
                && absent.paste_ratio.is_none()
                && absent.thermal_angle.is_none()
        );
        assert_eq!(zero.mask.unwrap().value_nm, 0);
        assert_eq!(zero.paste.unwrap().value_nm, 0);
        assert_eq!(zero.paste_ratio.unwrap().value, 0.0);
        assert_eq!(zero.thermal_angle.unwrap().value_degrees, 0.0);
        assert_eq!(zero.thermal_gap.unwrap().value_nm, 0);
        assert_eq!(zero.thermal_width.unwrap().value_nm, 0);
        assert_eq!(zero.clearance.unwrap().value_nm, 0);
        assert_eq!(zero.die_length.unwrap().value_nm, 0);
        assert_eq!(
            zero.zone,
            Some(kiapi::board::types::ZoneConnectionStyle::ZcsNone as i32),
            "(zone_connect 0) is KiCad's 'none', not inherited"
        );
    }

    #[test]
    fn malformed_pad_overrides_refuse_precisely() {
        for (clause, field) in [
            ("(solder_mask_margin nan)", "solder_mask_margin"),
            ("(solder_mask_margin 0 1)", "solder_mask_margin"),
            ("(solder_mask_margin 1e99)", "solder_mask_margin"),
            (
                "(solder_mask_margin 0) (solder_mask_margin 0.1)",
                "solder_mask_margin",
            ),
            ("(thermal_bridge_angle inf)", "thermal_bridge_angle"),
            ("(thermal_gap -1)", "thermal_gap"),
            ("(thermal_bridge_width -1)", "thermal_bridge_width"),
            ("(zone_connect 1.5)", "zone_connect"),
            ("(zone_connect 4)", "zone_connect"),
            ("(solder_paste_margin_ratio 2)", "solder_paste_margin_ratio"),
        ] {
            let source = LIBRARY_FOOTPRINT.replace(
                "(roundrect_rratio 0.2))",
                &format!("(roundrect_rratio 0.2) {clause})"),
            );
            let error = parse_library_footprint("Test:Socket", &source).unwrap_err();
            assert!(error.to_string().contains(field), "{error:#}");
        }
    }

    #[test]
    fn thermal_bridge_angle_is_representable() {
        let source = LIBRARY_FOOTPRINT.replace(
            "(roundrect_rratio 0.2))",
            "(roundrect_rratio 0.2) (thermal_bridge_angle 45))",
        );
        parse_library_footprint("Test:Socket", &source).unwrap();
    }

    #[test]
    fn pad_local_mask_and_thermal_settings_survive_refresh() {
        let source = LIBRARY_FOOTPRINT.replace(
            "(roundrect_rratio 0.2))",
            "(roundrect_rratio 0.2) (solder_mask_margin 0) (thermal_bridge_angle 37.5))",
        );
        let library = parse_library_footprint("Test:Socket", &source).unwrap();
        let current = current_instance(kiapi::board::types::BoardLayer::BlFCu);
        let prepared =
            build_updated_instance(&current, &library, &BTreeMap::new(), &BTreeSet::new()).unwrap();
        let updated =
            kiapi::board::types::FootprintInstance::decode(prepared.item.value.as_slice()).unwrap();
        let pads = decoded_pads(&updated);
        let stack = pads[0].pad_stack.as_ref().unwrap();
        assert_eq!(
            stack
                .front_outer_layers
                .as_ref()
                .unwrap()
                .solder_mask_settings
                .as_ref()
                .unwrap()
                .solder_mask_margin
                .as_ref()
                .unwrap()
                .value_nm,
            0
        );
        assert_eq!(
            stack
                .zone_settings
                .as_ref()
                .unwrap()
                .thermal_spokes
                .as_ref()
                .unwrap()
                .angle
                .as_ref()
                .unwrap()
                .value_degrees,
            37.5
        );
    }

    #[test]
    fn parser_rejects_lossy_user_text_variants() {
        for (from, to, expected) in [
            (
                "(at 0 1.5 0) (layer \"F.Fab\")",
                "(at 0 1.5 0) (unlocked yes) (layer \"F.Fab\")",
                "unlocked",
            ),
            (
                "(effects (font (size 0.8 0.8) (thickness 0.11)))",
                "(effects (font (size 0.8 0.7) (thickness 0.11)))",
                "non-square",
            ),
            (
                "(effects (font (size 0.8 0.8) (thickness 0.11)))",
                "(effects (font (size 0.8 0.8) (thickness 0.11)) (justify left))",
                "justify",
            ),
            (
                "(effects (font (size 0.8 0.8) (thickness 0.11)))",
                "(effects (font (size 0.8 0.8) (thickness 0.11) (italic yes)))",
                "italic",
            ),
            (
                "(effects (font (size 0.8 0.8) (thickness 0.11)))",
                "(effects (font (size 0.8 0.8)))",
                "missing its thickness",
            ),
            (
                "(uuid \"f96c2efe-5925-4f74-81d2-f89a56f57e13\")",
                "(uuid \"f96c2efe-5925-4f74-81d2-f89a56f57e13\") (uuid \"duplicate\")",
                "multiple identifier clauses",
            ),
            (
                "(uuid \"f96c2efe-5925-4f74-81d2-f89a56f57e13\")",
                "(uuid \"f96c2efe-5925-4f74-81d2-f89a56f57e13\") (tstamp \"legacy-too\")",
                "multiple identifier clauses",
            ),
        ] {
            let unsupported = LIBRARY_FOOTPRINT.replacen(from, to, 1);
            let error = parse_library_footprint("Test:Socket", &unsupported).unwrap_err();
            assert!(error.to_string().contains(expected), "{error:#}");
        }
    }

    #[test]
    fn rebuilding_an_applied_instance_is_a_noop_at_every_rotation_and_side() {
        let library = parse_library_footprint("Test:Socket", KICAD_LIBRARY_FOOTPRINT).unwrap();
        let net_codes = BTreeMap::from([("ROW1".to_string(), 11), ("COL1".to_string(), 12)]);
        let routed = BTreeSet::from(["ROW1".to_string(), "COL1".to_string()]);

        for layer in [
            kiapi::board::types::BoardLayer::BlFCu,
            kiapi::board::types::BoardLayer::BlBCu,
        ] {
            for rotation in [0.0, 90.0, 180.0, 270.0, 37.0] {
                let mut current = current_instance(layer);
                current.orientation = Some(kiapi::common::types::Angle {
                    value_degrees: rotation,
                });
                let first =
                    build_updated_instance(&current, &library, &net_codes, &routed).unwrap();
                let applied =
                    kiapi::board::types::FootprintInstance::decode(first.item.value.as_slice())
                        .unwrap();

                let second =
                    build_updated_instance(&applied, &library, &net_codes, &routed).unwrap();

                assert!(
                    second.changed_domains.is_empty(),
                    "{layer:?} at {rotation} degrees changed again: {:?}",
                    second.changed_domains
                );
            }
        }
    }

    #[test]
    fn kicad_roundtrip_defaults_do_not_create_false_pad_or_graphic_changes() {
        let library = parse_library_footprint("Test:Socket", LIBRARY_FOOTPRINT).unwrap();
        let current = current_instance(kiapi::board::types::BoardLayer::BlFCu);
        let prepared = build_updated_instance(
            &current,
            &library,
            &BTreeMap::from([("ROW1".to_string(), 11), ("COL1".to_string(), 12)]),
            &BTreeSet::new(),
        )
        .unwrap();
        let mut prepared =
            kiapi::board::types::FootprintInstance::decode(prepared.item.value.as_slice()).unwrap();
        let mut roundtripped = prepared.clone();
        roundtripped.definition.as_mut().unwrap().items.reverse();
        roundtripped.attributes.as_mut().unwrap().mounting_style =
            kiapi::board::types::FootprintMountingStyle::FmsUnspecified as i32;
        prepared.attributes.as_mut().unwrap().mounting_style =
            kiapi::board::types::FootprintMountingStyle::FmsUnknown as i32;

        for item in &mut roundtripped.definition.as_mut().unwrap().items {
            if item.type_url.ends_with("kiapi.board.types.Pad") {
                let mut pad = kiapi::board::types::Pad::decode(item.value.as_slice()).unwrap();
                pad.id = Some(kiapi::common::types::Kiid {
                    value: format!("kicad-pad-{}", pad.number),
                });
                pad.net = Some(kiapi::board::types::Net::default());
                pad.pad_to_die_length = Some(kiapi::common::types::Distance { value_nm: 0 });
                pad.symbol_pin = Some(kiapi::board::types::SymbolPinInfo::default());
                pad.pad_to_die_delay = Some(kiapi::common::types::Time { value_as: 0 });
                let stack = pad.pad_stack.as_mut().unwrap();
                stack.layers.reverse();
                if stack.drill.is_none() {
                    stack.drill = Some(kiapi::board::types::DrillProperties {
                        start_layer: kiapi::board::types::BoardLayer::BlUndefined as i32,
                        end_layer: kiapi::board::types::BoardLayer::BlUndefined as i32,
                        diameter: Some(builders::vec2(0.0, 0.0)),
                        shape: kiapi::board::types::DrillShape::DsUndefined as i32,
                        ..Default::default()
                    });
                }
                stack.zone_settings = Some(kiapi::board::types::ZoneConnectionSettings {
                    zone_connection: kiapi::board::types::ZoneConnectionStyle::ZcsInherited as i32,
                    thermal_spokes: Some(kiapi::board::types::ThermalSpokeSettings {
                        angle: Some(kiapi::common::types::Angle {
                            value_degrees: 90.0,
                        }),
                        ..Default::default()
                    }),
                });
                *item = builders::pack_any(&pad, "kiapi.board.types.Pad");
            } else if item
                .type_url
                .ends_with("kiapi.board.types.BoardGraphicShape")
            {
                let mut graphic =
                    kiapi::board::types::BoardGraphicShape::decode(item.value.as_slice()).unwrap();
                graphic.id = Some(kiapi::common::types::Kiid {
                    value: "kicad-graphic".to_string(),
                });
                graphic.net = Some(kiapi::board::types::Net::default());
                graphic
                    .shape
                    .as_mut()
                    .unwrap()
                    .attributes
                    .as_mut()
                    .unwrap()
                    .stroke
                    .as_mut()
                    .unwrap()
                    .style = kiapi::common::types::StrokeLineStyle::SlsSolid as i32;
                if let Some(kiapi::common::types::graphic_shape::Geometry::Polygon(polyset)) =
                    graphic
                        .shape
                        .as_mut()
                        .and_then(|shape| shape.geometry.as_mut())
                {
                    if let Some(outline) = polyset
                        .polygons
                        .first_mut()
                        .and_then(|polygon| polygon.outline.as_mut())
                    {
                        outline.nodes.rotate_left(1);
                        outline.nodes.reverse();
                    }
                }
                if let Some(kiapi::common::types::graphic_shape::Geometry::Rectangle(rectangle)) =
                    graphic
                        .shape
                        .as_mut()
                        .and_then(|shape| shape.geometry.as_mut())
                {
                    rectangle.corner_radius = Some(kiapi::common::types::Distance { value_nm: 0 });
                }
                *item = builders::pack_any(&graphic, "kiapi.board.types.BoardGraphicShape");
            }
        }

        let roundtripped_pads = normalized_items(roundtripped.definition.as_ref().unwrap(), "Pad")
            .unwrap()
            .into_iter()
            .map(|bytes| kiapi::board::types::Pad::decode(bytes.as_slice()).unwrap())
            .collect::<Vec<_>>();
        let prepared_pads = normalized_items(prepared.definition.as_ref().unwrap(), "Pad")
            .unwrap()
            .into_iter()
            .map(|bytes| kiapi::board::types::Pad::decode(bytes.as_slice()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(roundtripped_pads, prepared_pads);
        assert!(changed_domains(&roundtripped, &prepared)
            .unwrap()
            .is_empty());
    }

    fn plan_item(reference: &str, library_id: Option<&str>, kiid: &str) -> prost_types::Any {
        let mut instance = current_instance(kiapi::board::types::BoardLayer::BlFCu);
        instance.id = Some(kiapi::common::types::Kiid {
            value: kiid.to_string(),
        });
        instance.reference_field = Some(field("Reference", reference, 101.0, 48.0, true));
        if let Some(definition) = instance.definition.as_mut() {
            definition.reference_field = instance.reference_field.clone();
            definition.id = library_id.map(|library_id| {
                let (nickname, entry) = library_id.split_once(':').unwrap();
                kiapi::common::types::LibraryIdentifier {
                    library_nickname: nickname.to_string(),
                    entry_name: entry.to_string(),
                }
            });
        }
        builders::pack_any(&instance, "kiapi.board.types.FootprintInstance")
    }

    fn plan_fixture() -> (tempfile::TempDir, std::path::PathBuf, Vec<prost_types::Any>) {
        let temp = tempfile::tempdir().unwrap();
        let board = temp.path().join("board.kicad_pcb");
        std::fs::write(&board, "(kicad_pcb (version 20240108))").unwrap();
        let library_dir = temp.path().join("Test.pretty");
        std::fs::create_dir(&library_dir).unwrap();
        std::fs::write(
            library_dir.join("Socket.kicad_mod"),
            KICAD_LIBRARY_FOOTPRINT,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("fp-lib-table"),
            "(fp_lib_table (lib (name \"Test\") (type \"KiCad\") (uri \"${KIPRJMOD}/Test.pretty\") (options \"\") (descr \"\")))",
        )
        .unwrap();
        let items = vec![
            plan_item("SW2", Some("Test:Socket"), "kiid-2"),
            plan_item("TP1", None, "kiid-3"),
            plan_item("SW1", Some("Test:Socket"), "kiid-1"),
        ];
        (temp, board, items)
    }

    #[test]
    fn planner_refuses_an_unrepresentable_property_before_preparing_any_update() {
        let (temp, board, items) = plan_fixture();
        let unsupported = KICAD_LIBRARY_FOOTPRINT.replace(
            "\t(property \"AssemblyVendor\" \"Example Assembly\"\n\t\t(at",
            "\t(property \"AssemblyVendor\" \"Example Assembly\"\n\t\t(unlocked yes)\n\t\t(at",
        );
        std::fs::write(
            temp.path().join("Test.pretty/Socket.kicad_mod"),
            unsupported,
        )
        .unwrap();

        let plan = plan_updates(
            &board,
            &items,
            &BTreeMap::from([("ROW1".to_string(), 11), ("COL1".to_string(), 12)]),
            &BTreeSet::new(),
            &UpdateFilters::default(),
        );

        assert_eq!(plan.status, PlanStatus::Conflict);
        assert!(plan.prepared_items.is_empty());
        assert!(plan.changes.is_empty());
        assert!(plan.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "unsupported_library_footprint"
                && diagnostic.message.contains("AssemblyVendor")
                && diagnostic.message.contains("unlocked")
        }));
    }

    #[test]
    fn filters_are_normalized_and_supplied_empty_arrays_select_nothing() {
        let filters = parse_filters(&serde_json::json!({
            "references": ["SW2", "SW1", "SW2"],
            "library_ids": ["Test:Socket", "Test:Socket"]
        }))
        .unwrap();
        assert_eq!(
            filters.references,
            Some(BTreeSet::from(["SW1".to_string(), "SW2".to_string()]))
        );
        assert_eq!(
            filters.library_ids,
            Some(BTreeSet::from(["Test:Socket".to_string()]))
        );

        let (_, board, items) = plan_fixture();
        let empty = plan_updates(
            &board,
            &items,
            &BTreeMap::from([("ROW1".to_string(), 11), ("COL1".to_string(), 12)]),
            &BTreeSet::new(),
            &parse_filters(&serde_json::json!({ "references": [] })).unwrap(),
        );
        assert_eq!(empty.status, PlanStatus::Noop);
        assert_eq!(empty.coverage.selected.planned, 0);
        assert!(empty.changes.is_empty());

        let malformed =
            parse_filters(&serde_json::json!({ "library_ids": ["not-a-library-id"] })).unwrap_err();
        assert_eq!(malformed.field, "library_ids");
        assert!(malformed.reason.contains("Library:Footprint"));
    }

    #[test]
    fn planner_selects_deterministically_and_intersects_filters() {
        let (_temp, board, items) = plan_fixture();
        let net_codes = BTreeMap::from([("ROW1".to_string(), 11), ("COL1".to_string(), 12)]);
        let all = plan_updates(
            &board,
            &items,
            &net_codes,
            &BTreeSet::new(),
            &UpdateFilters::default(),
        );
        assert_eq!(all.status, PlanStatus::Ready);
        assert_eq!(all.coverage.selected.planned, 2);
        assert_eq!(all.coverage.changed.planned, 2);
        assert_eq!(all.coverage.skipped_unlinked.planned, 1);
        assert_eq!(
            all.changes
                .iter()
                .map(|change| change.reference.as_str())
                .collect::<Vec<_>>(),
            vec!["SW1", "SW2"]
        );

        let intersection = plan_updates(
            &board,
            &items,
            &net_codes,
            &BTreeSet::new(),
            &parse_filters(&serde_json::json!({
                "references": ["SW2"],
                "library_ids": ["Other:Socket"]
            }))
            .unwrap(),
        );
        assert_eq!(intersection.status, PlanStatus::Noop);
        assert_eq!(intersection.coverage.selected.planned, 0);
    }

    #[test]
    fn planner_conflicts_on_missing_or_duplicate_selected_references() {
        let (_temp, board, mut items) = plan_fixture();
        let net_codes = BTreeMap::from([("ROW1".to_string(), 11), ("COL1".to_string(), 12)]);
        let missing = plan_updates(
            &board,
            &items,
            &net_codes,
            &BTreeSet::new(),
            &parse_filters(&serde_json::json!({ "references": ["SW404"] })).unwrap(),
        );
        assert_eq!(missing.status, PlanStatus::Conflict);
        assert!(missing
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "reference_not_found"));

        items.push(plan_item("SW1", Some("Test:Socket"), "kiid-duplicate"));
        let duplicate = plan_updates(
            &board,
            &items,
            &net_codes,
            &BTreeSet::new(),
            &parse_filters(&serde_json::json!({ "references": ["SW1"] })).unwrap(),
        );
        assert_eq!(duplicate.status, PlanStatus::Conflict);
        assert!(duplicate
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "duplicate_reference"));
        assert!(duplicate.prepared_items.is_empty());
    }

    #[test]
    fn explicitly_selected_unlinked_footprint_is_a_conflict() {
        let (_temp, board, items) = plan_fixture();
        let plan = plan_updates(
            &board,
            &items,
            &BTreeMap::from([("ROW1".to_string(), 11), ("COL1".to_string(), 12)]),
            &BTreeSet::new(),
            &parse_filters(&serde_json::json!({ "references": ["TP1"] })).unwrap(),
        );

        assert_eq!(plan.status, PlanStatus::Conflict);
        assert!(plan
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "unlinked_footprint"));
        assert!(plan.prepared_items.is_empty());
    }

    #[test]
    fn planner_returns_noop_after_the_prepared_update_is_applied() {
        let (_temp, board, items) = plan_fixture();
        let net_codes = BTreeMap::from([("ROW1".to_string(), 11), ("COL1".to_string(), 12)]);
        let first = plan_updates(
            &board,
            &[items[2].clone()],
            &net_codes,
            &BTreeSet::new(),
            &UpdateFilters::default(),
        );
        assert_eq!(first.status, PlanStatus::Ready);

        let second = plan_updates(
            &board,
            &first.prepared_items,
            &net_codes,
            &BTreeSet::new(),
            &UpdateFilters::default(),
        );

        assert_eq!(second.status, PlanStatus::Noop);
        assert_eq!(second.coverage.selected.planned, 1);
        assert_eq!(second.coverage.changed.planned, 0);
        assert_eq!(second.coverage.unchanged.planned, 1);
        assert!(second.prepared_items.is_empty());
    }

    #[test]
    fn datasheet_only_library_change_is_reported_as_metadata() {
        let base = parse_library_footprint("Test:Socket", LIBRARY_FOOTPRINT).unwrap();
        let current = current_instance(kiapi::board::types::BoardLayer::BlFCu);
        let net_codes = BTreeMap::from([("ROW1".to_string(), 11), ("COL1".to_string(), 12)]);
        let routed = BTreeSet::new();
        let applied = build_updated_instance(&current, &base, &net_codes, &routed).unwrap();
        let applied =
            kiapi::board::types::FootprintInstance::decode(applied.item.value.as_slice()).unwrap();
        let changed_source =
            LIBRARY_FOOTPRINT.replace("new-datasheet.pdf", "replacement-datasheet.pdf");
        let changed = parse_library_footprint("Test:Socket", &changed_source).unwrap();

        let prepared = build_updated_instance(&applied, &changed, &net_codes, &routed).unwrap();

        assert_eq!(
            prepared.changed_domains,
            BTreeSet::from([ChangedDomain::Metadata])
        );
    }

    #[test]
    fn plan_revision_tracks_selected_board_library_and_filter_inputs_only() {
        let (_temp, board, mut items) = plan_fixture();
        let net_codes = BTreeMap::from([("ROW1".to_string(), 11), ("COL1".to_string(), 12)]);
        let filters = parse_filters(&serde_json::json!({ "references": ["SW1"] })).unwrap();
        let first = plan_updates(&board, &items, &net_codes, &BTreeSet::new(), &filters);
        let identical = plan_updates(&board, &items, &net_codes, &BTreeSet::new(), &filters);
        assert_eq!(first.plan_revision, identical.plan_revision);

        items[0] = plan_item("SW2", Some("Test:Socket"), "unselected-changed");
        let unrelated = plan_updates(&board, &items, &net_codes, &BTreeSet::new(), &filters);
        assert_eq!(first.plan_revision, unrelated.plan_revision);

        items[2] = plan_item("SW1", Some("Test:Socket"), "selected-changed");
        let board_changed = plan_updates(&board, &items, &net_codes, &BTreeSet::new(), &filters);
        assert_ne!(first.plan_revision, board_changed.plan_revision);

        std::fs::write(
            board.parent().unwrap().join("Test.pretty/Socket.kicad_mod"),
            LIBRARY_FOOTPRINT.replace("updated description", "library changed"),
        )
        .unwrap();
        let library_changed = plan_updates(&board, &items, &net_codes, &BTreeSet::new(), &filters);
        assert_ne!(board_changed.plan_revision, library_changed.plan_revision);

        let different_filter = plan_updates(
            &board,
            &items,
            &net_codes,
            &BTreeSet::new(),
            &parse_filters(&serde_json::json!({ "references": ["SW2"] })).unwrap(),
        );
        assert_ne!(
            library_changed.plan_revision,
            different_filter.plan_revision
        );
    }

    fn test_context(ipc_address: &str) -> crate::tools::ToolContext {
        crate::tools::ToolContext::new(
            crate::tools::ServerConfig {
                kicad_cli: String::new(),
                kicad_binary: String::new(),
                ipc_address: ipc_address.to_string(),
                project_dir: None,
                jlcpcb_db_path: None,
                auto_load_toolsets: false,
                eager_toolsets: false,
            },
            std::sync::Arc::new(crate::router::ToolRouter::new()),
        )
    }

    fn result_json(result: &crate::mcp::protocol::CallToolResult) -> serde_json::Value {
        let text = match result.content.first() {
            Some(crate::mcp::protocol::ToolContent::Text { text }) => text,
            other => panic!("expected text result, got {other:?}"),
        };
        serde_json::from_str(text).expect("tool result must be JSON")
    }

    #[test]
    fn tool_schema_defaults_to_dry_run_and_requires_only_the_board() {
        let definition = tool();

        assert_eq!(definition.name, "update_footprints_from_library");
        assert_eq!(
            definition.input_schema["required"],
            serde_json::json!(["board"])
        );
        assert_eq!(
            definition.input_schema["properties"]["dry_run"]["default"],
            true
        );
        assert_eq!(
            definition.input_schema["properties"]["references"]["items"]["type"],
            "string"
        );
        assert_eq!(
            definition.input_schema["properties"]["library_ids"]["items"]["type"],
            "string"
        );
    }

    #[tokio::test]
    async fn apply_without_a_reviewed_revision_is_rejected_before_ipc() {
        let temp = tempfile::tempdir().unwrap();
        let board = temp.path().join("board.kicad_pcb");
        std::fs::write(&board, "(kicad_pcb (version 20240108))").unwrap();

        let result = handle_update_footprints_from_library(
            &serde_json::json!({ "board": board, "dry_run": false }),
            &test_context(""),
        )
        .await
        .unwrap();

        assert!(result.is_error);
        assert_eq!(
            crate::mcp::error::extract_error_kind(&result).as_deref(),
            Some("invalid_argument")
        );
        let text = match result.content.first().unwrap() {
            crate::mcp::protocol::ToolContent::Text { text } => text,
            _ => unreachable!(),
        };
        assert!(text.contains("expected_plan_revision"));
    }

    #[tokio::test]
    async fn unreachable_ipc_returns_a_non_mutating_conflict_response() {
        let temp = tempfile::tempdir().unwrap();
        let board = temp.path().join("board.kicad_pcb");
        let before = "(kicad_pcb (version 20240108))";
        std::fs::write(&board, before).unwrap();

        let result = handle_update_footprints_from_library(
            &serde_json::json!({ "board": board }),
            &test_context(""),
        )
        .await
        .unwrap();

        assert!(!result.is_error);
        let response = result_json(&result);
        assert_eq!(response["status"], "conflict");
        assert_eq!(response["coverage"]["transport"], "live_kicad_ipc");
        assert!(response["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("live-IPC-only"));
        assert_eq!(std::fs::read_to_string(board).unwrap(), before);
    }

    #[derive(Default)]
    struct MutationCapture {
        begin_count: usize,
        footprint_reads: usize,
        final_items: Vec<prost_types::Any>,
        update_batches: Vec<usize>,
        commit_actions: Vec<kiapi::common::commands::CommitAction>,
    }

    struct PlannerMock {
        url: String,
        capture: std::sync::Arc<std::sync::Mutex<MutationCapture>>,
        _thread: std::thread::JoinHandle<()>,
    }

    fn api_ok() -> kiapi::common::ApiResponse {
        kiapi::common::ApiResponse {
            status: Some(kiapi::common::ApiResponseStatus {
                status: kiapi::common::ApiStatusCode::AsOk as i32,
                error_message: String::new(),
            }),
            header: None,
            message: None,
        }
    }

    fn api_reply(message: prost_types::Any) -> kiapi::common::ApiResponse {
        kiapi::common::ApiResponse {
            message: Some(message),
            ..api_ok()
        }
    }

    fn spawn_planner_mock(
        board: &Path,
        footprint: prost_types::Any,
        fail_update: bool,
    ) -> PlannerMock {
        spawn_planner_mock_batch(board, vec![footprint], fail_update, false)
    }

    fn spawn_planner_mock_batch(
        board: &Path,
        footprints: Vec<prost_types::Any>,
        fail_update: bool,
        corrupt_readback: bool,
    ) -> PlannerMock {
        use nng::options::Options;

        static NEXT_MOCK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let url = format!(
            "inproc://footprint-update-{}",
            NEXT_MOCK.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let socket = nng::Socket::new(nng::Protocol::Rep0).unwrap();
        socket
            .set_opt::<nng::options::RecvTimeout>(Some(std::time::Duration::from_secs(20)))
            .unwrap();
        socket.listen(&url).unwrap();

        let board_name = board.to_string_lossy().to_string();
        let capture = std::sync::Arc::new(std::sync::Mutex::new(MutationCapture::default()));
        let captured = capture.clone();
        let thread = std::thread::spawn(move || {
            let mut footprints = footprints;
            let mut before_commit = None;
            captured.lock().unwrap().final_items = footprints.clone();
            while let Ok(message) = socket.recv() {
                let request = kiapi::common::ApiRequest::decode(message.as_slice()).unwrap();
                let message = request.message.expect("request message");
                let response = if message.type_url.ends_with("GetOpenDocuments") {
                    api_reply(builders::pack_any(
                        &kiapi::common::commands::GetOpenDocumentsResponse {
                            documents: vec![kiapi::common::types::DocumentSpecifier {
                                r#type: kiapi::common::types::DocumentType::DoctypePcb as i32,
                                identifier: Some(
                                    kiapi::common::types::document_specifier::Identifier::BoardFilename(
                                        board_name.clone(),
                                    ),
                                ),
                                project: None,
                            }],
                        },
                        "kiapi.common.commands.GetOpenDocumentsResponse",
                    ))
                } else if message.type_url.ends_with("GetItems") {
                    let command =
                        kiapi::common::commands::GetItems::decode(message.value.as_slice())
                            .unwrap();
                    let items = if command.types
                        == vec![kiapi::common::types::KiCadObjectType::KotPcbFootprint as i32]
                    {
                        captured.lock().unwrap().footprint_reads += 1;
                        footprints.clone()
                    } else {
                        Vec::new()
                    };
                    api_reply(builders::pack_any(
                        &kiapi::common::commands::GetItemsResponse {
                            header: None,
                            status: kiapi::common::types::ItemRequestStatus::IrsOk as i32,
                            items,
                        },
                        "kiapi.common.commands.GetItemsResponse",
                    ))
                } else if message.type_url.ends_with("GetNets") {
                    api_reply(builders::pack_any(
                        &kiapi::board::commands::NetsResponse {
                            nets: vec![builders::net("ROW1", 11), builders::net("COL1", 12)],
                        },
                        "kiapi.board.commands.NetsResponse",
                    ))
                } else if message.type_url.ends_with("BeginCommit") {
                    captured.lock().unwrap().begin_count += 1;
                    before_commit = Some(footprints.clone());
                    api_reply(builders::pack_any(
                        &kiapi::common::commands::BeginCommitResponse {
                            id: Some(kiapi::common::types::Kiid {
                                value: "commit-1".to_string(),
                            }),
                        },
                        "kiapi.common.commands.BeginCommitResponse",
                    ))
                } else if message.type_url.ends_with("UpdateItems") {
                    let update =
                        kiapi::common::commands::UpdateItems::decode(message.value.as_slice())
                            .unwrap();
                    captured
                        .lock()
                        .unwrap()
                        .update_batches
                        .push(update.items.len());
                    if !fail_update && !corrupt_readback {
                        for updated in &update.items {
                            let fp = kiapi::board::types::FootprintInstance::decode(
                                updated.value.as_slice(),
                            )
                            .unwrap();
                            let old = footprints
                                .iter_mut()
                                .find(|old| {
                                    kiapi::board::types::FootprintInstance::decode(
                                        old.value.as_slice(),
                                    )
                                    .unwrap()
                                    .id == fp.id
                                })
                                .unwrap();
                            *old = updated.clone();
                        }
                    }
                    captured.lock().unwrap().final_items = footprints.clone();
                    let updated_items = update
                        .items
                        .into_iter()
                        .map(|item| kiapi::common::commands::ItemUpdateResult {
                            status: Some(kiapi::common::commands::ItemStatus {
                                code: if fail_update {
                                    kiapi::common::commands::ItemStatusCode::IscInvalidData as i32
                                } else {
                                    kiapi::common::commands::ItemStatusCode::IscOk as i32
                                },
                                error_message: if fail_update {
                                    "mock update failure".to_string()
                                } else {
                                    String::new()
                                },
                            }),
                            item: Some(item),
                        })
                        .collect();
                    api_reply(builders::pack_any(
                        &kiapi::common::commands::UpdateItemsResponse {
                            header: None,
                            status: kiapi::common::types::ItemRequestStatus::IrsOk as i32,
                            updated_items,
                        },
                        "kiapi.common.commands.UpdateItemsResponse",
                    ))
                } else if message.type_url.ends_with("EndCommit") {
                    let end = kiapi::common::commands::EndCommit::decode(message.value.as_slice())
                        .unwrap();
                    captured.lock().unwrap().commit_actions.push(end.action());
                    if end.action() == kiapi::common::commands::CommitAction::CmaDrop {
                        footprints = before_commit.take().unwrap();
                        captured.lock().unwrap().final_items = footprints.clone();
                    }
                    api_reply(builders::pack_any(
                        &kiapi::common::commands::EndCommitResponse {},
                        "kiapi.common.commands.EndCommitResponse",
                    ))
                } else {
                    panic!("unexpected mock request {}", message.type_url);
                };
                let response = nng::Message::from(response.encode_to_vec().as_slice());
                if socket.send(response).is_err() {
                    break;
                }
            }
        });
        PlannerMock {
            url,
            capture,
            _thread: thread,
        }
    }

    #[tokio::test]
    async fn dry_run_and_stale_revision_never_open_a_commit_or_send_updates() {
        let (_temp, board, items) = plan_fixture();
        let mock = spawn_planner_mock(&board, items[2].clone(), false);
        let context = test_context(&mock.url);

        let dry_run =
            handle_update_footprints_from_library(&serde_json::json!({ "board": board }), &context)
                .await
                .unwrap();
        assert_eq!(result_json(&dry_run)["status"], "ready");

        let stale = handle_update_footprints_from_library(
            &serde_json::json!({
                "board": board,
                "dry_run": false,
                "expected_plan_revision": "stale"
            }),
            &context,
        )
        .await
        .unwrap();
        let stale = result_json(&stale);
        assert_eq!(stale["status"], "conflict");
        assert_eq!(stale["diagnostics"][0]["code"], "stale_plan_revision");

        let capture = mock.capture.lock().unwrap();
        assert_eq!(capture.begin_count, 0);
        assert!(capture.update_batches.is_empty());
        assert!(capture.commit_actions.is_empty());
    }

    #[tokio::test]
    async fn apply_sends_one_batch_in_one_commit() {
        let (_temp, board, items) = plan_fixture();
        let mock = spawn_planner_mock(&board, items[2].clone(), false);
        let context = test_context(&mock.url);
        let dry_run =
            handle_update_footprints_from_library(&serde_json::json!({ "board": board }), &context)
                .await
                .unwrap();
        let revision = result_json(&dry_run)["plan_revision"]
            .as_str()
            .unwrap()
            .to_string();

        let applied = handle_update_footprints_from_library(
            &serde_json::json!({
                "board": board,
                "dry_run": false,
                "expected_plan_revision": revision
            }),
            &context,
        )
        .await
        .unwrap();

        let applied = result_json(&applied);
        assert_eq!(applied["status"], "applied");
        assert_eq!(applied["coverage"]["changed"]["applied"], 1);
        let capture = mock.capture.lock().unwrap();
        assert_eq!(capture.begin_count, 1);
        assert_eq!(capture.update_batches, vec![1]);
        assert_eq!(
            capture.commit_actions,
            vec![kiapi::common::commands::CommitAction::CmaCommit]
        );
    }

    #[tokio::test]
    async fn readback_mismatch_drops_the_commit_and_restores_exact_input() {
        let (_temp, board, items) = plan_fixture();
        let before = vec![items[2].clone()];
        let mock = spawn_planner_mock_batch(&board, before.clone(), false, true);
        let context = test_context(&mock.url);
        let dry = handle_update_footprints_from_library(&json!({"board": board}), &context)
            .await
            .unwrap();
        let revision = result_json(&dry)["plan_revision"]
            .as_str()
            .unwrap()
            .to_string();
        let result = handle_update_footprints_from_library(
            &json!({"board": board, "dry_run": false, "expected_plan_revision": revision}),
            &context,
        )
        .await
        .unwrap();
        assert!(result.is_error);
        let capture = mock.capture.lock().unwrap();
        assert_eq!(
            capture.commit_actions,
            vec![kiapi::common::commands::CommitAction::CmaDrop]
        );
        assert_eq!(capture.final_items, before);
    }

    #[tokio::test]
    async fn served_refresh_applies_description_and_models_with_independent_readback() {
        use crate::mcp::handler::McpHandler;
        use crate::tools::ServerConfig;
        let (_temp, board, items) = plan_fixture();
        let mock = spawn_planner_mock(&board, items[2].clone(), false);
        let handler = McpHandler::new(ServerConfig {
            kicad_cli: String::new(),
            kicad_binary: String::new(),
            ipc_address: mock.url.clone(),
            project_dir: None,
            jlcpcb_db_path: None,
            auto_load_toolsets: false,
            eager_toolsets: true,
        })
        .await
        .unwrap();
        async fn call(handler: &McpHandler, args: serde_json::Value) -> serde_json::Value {
            let response = handler.handle_message(json!({"jsonrpc":"2.0", "id":48, "method":"tools/call", "params":{"name":"update_footprints_from_library", "arguments":args}})).await.unwrap();
            let result = response.result.unwrap();
            assert_ne!(result["isError"], true, "{result}");
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
        }
        let dry = call(&handler, json!({"board":board})).await;
        assert_eq!(dry["status"], "ready");
        let applied = call(
            &handler,
            json!({"board":board, "dry_run":false, "expected_plan_revision":dry["plan_revision"]}),
        )
        .await;
        assert_eq!(applied["status"], "applied");
        let capture = mock.capture.lock().unwrap();
        assert_eq!(
            capture.footprint_reads, 4,
            "dry run, apply plan, in-commit stale check, independent readback"
        );
        assert_eq!(capture.update_batches, vec![1]);
        assert_eq!(
            capture.commit_actions,
            vec![kiapi::common::commands::CommitAction::CmaCommit]
        );
        let after =
            kiapi::board::types::FootprintInstance::decode(capture.final_items[0].value.as_slice())
                .unwrap();
        assert_eq!(
            after
                .definition
                .as_ref()
                .unwrap()
                .attributes
                .as_ref()
                .unwrap()
                .description,
            "updated description"
        );
        let library = parse_library_footprint("Test:Socket", KICAD_LIBRARY_FOOTPRINT).unwrap();
        let expected_models = library
            .models
            .iter()
            .map(|model| builders::pack_any(model, "kiapi.board.types.Footprint3DModel"))
            .collect::<Vec<_>>();
        let actual_models = after
            .definition
            .as_ref()
            .unwrap()
            .items
            .iter()
            .filter(|item| item.type_url.ends_with("Footprint3DModel"))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(actual_models, expected_models);
    }

    #[tokio::test]
    async fn failed_update_drops_the_single_commit() {
        let (_temp, board, items) = plan_fixture();
        let mock = spawn_planner_mock(&board, items[2].clone(), true);
        let context = test_context(&mock.url);
        let dry_run =
            handle_update_footprints_from_library(&serde_json::json!({ "board": board }), &context)
                .await
                .unwrap();
        let revision = result_json(&dry_run)["plan_revision"]
            .as_str()
            .unwrap()
            .to_string();

        let failed = handle_update_footprints_from_library(
            &serde_json::json!({
                "board": board,
                "dry_run": false,
                "expected_plan_revision": revision
            }),
            &context,
        )
        .await
        .unwrap();

        assert!(failed.is_error);
        let capture = mock.capture.lock().unwrap();
        assert_eq!(capture.begin_count, 1);
        assert_eq!(capture.update_batches, vec![1]);
        assert_eq!(
            capture.commit_actions,
            vec![kiapi::common::commands::CommitAction::CmaDrop]
        );
    }
}
