//! Planning and readback for atomic page/reference migrations.
use super::*;
use serde::Deserialize;

type Pending = HashMap<PathBuf, (PathBuf, String, String)>;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReferenceChange {
    sheet_instance_path: String,
    old_reference: String,
    new_reference: String,
}

pub(super) struct Options {
    pub pages: HashMap<String, String>,
    pub changes: Vec<ReferenceChange>,
    pub dry_run: bool,
    pub strict: bool,
    explicit_order: bool,
}

fn valid_path(path: &str) -> bool {
    path.starts_with('/')
        && path[1..]
            .split('/')
            .all(|part| uuid::Uuid::parse_str(part).is_ok())
}

fn valid_reference(reference: &str) -> bool {
    let Some(start) = reference.find(|c: char| c.is_ascii_digit()) else {
        return false;
    };
    start > 0
        && reference[..start]
            .chars()
            .all(|c| c.is_ascii_alphabetic() || c == '#')
        && reference[start..].chars().all(|c| c.is_ascii_digit())
}

impl Options {
    pub fn parse(args: &Value) -> anyhow::Result<Self> {
        let mut pages = HashMap::new();
        if let Some(order) = args.get("sheet_order") {
            let order: Vec<String> = serde_json::from_value(order.clone())
                .map_err(|e| anyhow::anyhow!("Invalid sheet_order: {e}"))?;
            for (index, path) in order.into_iter().enumerate() {
                anyhow::ensure!(valid_path(&path), "Invalid sheet_order UUID path: {path}");
                anyhow::ensure!(
                    pages
                        .insert(path.clone(), (index + 2).to_string())
                        .is_none(),
                    "Duplicate sheet_order path: {path}"
                );
            }
        }
        let changes: Vec<ReferenceChange> = match args.get("reference_changes") {
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|e| anyhow::anyhow!("Invalid reference_changes: {e}"))?,
            None => Vec::new(),
        };
        let mut targets = HashSet::new();
        for change in &changes {
            anyhow::ensure!(
                valid_path(&change.sheet_instance_path),
                "Invalid reference_changes instance path"
            );
            anyhow::ensure!(
                valid_reference(&change.old_reference) && valid_reference(&change.new_reference),
                "reference_changes requires annotated reference designators"
            );
            anyhow::ensure!(
                targets.insert((
                    change.sheet_instance_path.clone(),
                    change.old_reference.clone()
                )),
                "Duplicate reference_changes target"
            );
        }
        let dry_run = match args.get("dry_run") {
            Some(value) => value
                .as_bool()
                .ok_or_else(|| anyhow::anyhow!("dry_run must be boolean"))?,
            None => false,
        };
        Ok(Self {
            pages,
            changes,
            dry_run,
            strict: args.get("sheet_order").is_some()
                || args.get("reference_changes").is_some()
                || dry_run,
            explicit_order: args.get("sheet_order").is_some(),
        })
    }

    pub fn validate_order(&self, pages: &[Value]) -> anyhow::Result<()> {
        if self.explicit_order {
            let observed: HashSet<_> = pages
                .iter()
                .map(|p| p["sheet_instance_path"].as_str().unwrap())
                .collect();
            anyhow::ensure!(observed.len() == self.pages.len() && self.pages.keys().all(|p|observed.contains(p.as_str())),
                "sheet_order must contain every non-root instance exactly once, without unknown paths");
        }
        Ok(())
    }
}

#[derive(Clone)]
struct Member {
    file: PathBuf,
    uuid: String,
    path: String,
    reference: String,
    unit: u32,
    lib_id: String,
    value: String,
}

pub(super) fn remap_references(
    pending: &mut Pending,
    project: &str,
    root_prefix: &str,
    pages: &[Value],
    changes: &[ReferenceChange],
) -> anyhow::Result<Vec<Value>> {
    if changes.is_empty() {
        return Ok(Vec::new());
    }
    // The root plus every traversed child establishes which saved records belong
    // to this hierarchy. A shared file can also carry other roots/projects.
    let mut live: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for (key, (path, _, _)) in pending.iter() {
        if cse::Schematic::from_source(path, &pending[key].2)?
            .uuid
            .as_ref()
            .is_some_and(|uuid| format!("/{uuid}") == root_prefix)
        {
            live.entry(key.clone())
                .or_default()
                .push(root_prefix.to_owned());
        }
    }
    for page in pages {
        let path = PathBuf::from(page["document_path"].as_str().unwrap()).canonicalize()?;
        live.entry(path)
            .or_default()
            .push(page["sheet_instance_path"].as_str().unwrap().to_owned());
    }
    let mut members = Vec::new();
    for (file, paths) in &live {
        let (path, _, source) = pending
            .get(file)
            .ok_or_else(|| anyhow::anyhow!("Missing sheet source {}", file.display()))?;
        let sch = cse::Schematic::from_source(path, source)?;
        for symbol in sch.symbols.iter() {
            let instances = symbol.instances();
            for instance_path in paths {
                let matches: Vec<_> = instances
                    .iter()
                    .filter(|i| {
                        i.project.as_deref() == Some(project)
                            && i.path.as_deref() == Some(instance_path)
                    })
                    .collect();
                anyhow::ensure!(
                    matches.len() == 1,
                    "Missing or duplicate saved symbol instance for {} at {}",
                    symbol.uuid,
                    instance_path
                );
                let instance = matches[0];
                let reference = instance
                    .reference
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Missing instance reference"))?;
                anyhow::ensure!(
                    valid_reference(reference) && instance.unit == Some(symbol.unit),
                    "Incomplete or inconsistent symbol instance {}",
                    symbol.uuid
                );
                members.push(Member {
                    file: file.clone(),
                    uuid: symbol.uuid.clone(),
                    path: instance_path.clone(),
                    reference: reference.clone(),
                    unit: symbol.unit,
                    lib_id: symbol.lib_id.clone(),
                    value: symbol.value_str().unwrap_or("").to_owned(),
                });
            }
        }
    }
    let mut mapping = HashMap::new();
    for change in changes {
        let targets: Vec<_> = members
            .iter()
            .filter(|m| m.path == change.sheet_instance_path && m.reference == change.old_reference)
            .collect();
        anyhow::ensure!(
            !targets.is_empty(),
            "Stale or unknown reference_changes target {} at {}",
            change.old_reference,
            change.sheet_instance_path
        );
        validate_group(&targets)?;
        mapping.insert(
            (
                change.sheet_instance_path.as_str(),
                change.old_reference.as_str(),
            ),
            change.new_reference.as_str(),
        );
    }
    // Validate the entire final project namespace, not the intermediate sequence:
    // swaps are legal, collisions with untouched components are not.
    let mut final_groups: HashMap<&str, Vec<&Member>> = HashMap::new();
    for member in &members {
        let reference = mapping
            .get(&(member.path.as_str(), member.reference.as_str()))
            .copied()
            .unwrap_or(member.reference.as_str());
        final_groups.entry(reference).or_default().push(member);
    }
    for (reference, group) in final_groups {
        validate_group(&group)
            .map_err(|e| anyhow::anyhow!("Reference collision {reference}: {e}"))?;
    }
    let mut result = Vec::new();
    for (file, (path, _, source)) in pending.iter_mut() {
        let mut sch = cse::Schematic::from_source(&*path, &*source)?;
        let mut ids = Vec::new();
        for symbol in sch.symbols.iter_mut() {
            let symbol_uuid = symbol.uuid.clone();
            let old_property = symbol.reference().map(str::to_owned);
            let mut property_target = None;
            for member in members
                .iter()
                .filter(|m| &m.file == file && m.uuid == symbol_uuid)
            {
                if let Some(new) = mapping.get(&(member.path.as_str(), member.reference.as_str())) {
                    symbol.set_instance_path(project, &member.path, new, member.unit);
                    if old_property.as_deref() == Some(member.reference.as_str()) {
                        property_target = Some(*new);
                    }
                    ids.push(ItemId::new(symbol.uuid.clone())?);
                    result.push(json!({"document_path":path,"symbol_uuid":symbol.uuid,"sheet_instance_path":member.path,
                        "old_reference":member.reference,"new_reference":new,"unit":member.unit}));
                }
            }
            if let Some(new) = property_target {
                symbol.set_reference(new);
            }
        }
        if !ids.is_empty() {
            ids.sort_by_key(|a| a.to_string());
            ids.dedup();
            let command = SchematicCommand::replace_items_from_document(
                source,
                &sch.to_source(),
                ids,
                "Migrate hierarchy instance references",
            )?;
            *source = prepare_command(path, source, &command)?.0;
        }
    }
    result.sort_by_key(|r| {
        (
            r["sheet_instance_path"].as_str().unwrap().to_owned(),
            r["symbol_uuid"].as_str().unwrap().to_owned(),
        )
    });
    Ok(result)
}

fn validate_group(group: &[&Member]) -> anyhow::Result<()> {
    let first = group[0];
    let mut units = HashSet::new();
    for member in group {
        anyhow::ensure!(
            member.path == first.path
                && member.file == first.file
                && member.reference == first.reference
                && member.lib_id == first.lib_id
                && member.value == first.value
                && units.insert(member.unit),
            "not distinct units of one component"
        );
    }
    Ok(())
}

pub(super) fn readback(
    pages: &[Value],
    references: &[Value],
    project: &str,
) -> anyhow::Result<(Vec<Value>, Vec<Value>)> {
    let mut observed_pages = Vec::new();
    for page in pages {
        let sch = cse::Schematic::load(page["parent_document"].as_str().unwrap())?;
        let sheet = sch
            .sheets
            .iter()
            .find(|s| Some(s.uuid.as_str()) == page["sheet_uuid"].as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing saved sheet"))?;
        let full = page["sheet_instance_path"].as_str().unwrap();
        let parent = full.rsplit_once('/').unwrap().0;
        let matches: Vec<_> = sheet
            .instances
            .iter()
            .filter(|i| i.project_name == project && i.path == parent)
            .collect();
        anyhow::ensure!(
            matches.len() == 1 && Some(matches[0].page.as_str()) == page["page"].as_str(),
            "Saved page differs from plan"
        );
        let mut result = page.clone();
        result["page"] = json!(matches[0].page);
        observed_pages.push(result);
    }
    let mut observed_references = Vec::new();
    for reference in references {
        let sch = cse::Schematic::load(reference["document_path"].as_str().unwrap())?;
        let symbol = sch
            .symbols
            .iter()
            .find(|s| Some(s.uuid.as_str()) == reference["symbol_uuid"].as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing saved symbol"))?;
        let matches: Vec<_> = symbol
            .instances()
            .into_iter()
            .filter(|i| {
                i.project.as_deref() == Some(project)
                    && i.path.as_deref() == reference["sheet_instance_path"].as_str()
            })
            .collect();
        anyhow::ensure!(
            matches.len() == 1
                && matches[0].reference.as_deref() == reference["new_reference"].as_str()
                && matches[0].unit.map(u64::from) == reference["unit"].as_u64(),
            "Saved reference differs from plan"
        );
        let mut result = reference.clone();
        result["new_reference"] = json!(matches[0].reference);
        observed_references.push(result);
    }
    Ok((observed_pages, observed_references))
}
