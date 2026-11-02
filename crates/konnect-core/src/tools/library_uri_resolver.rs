//! Library URI expansion with explicit sources for deterministic, read-only tests.

use std::path::{Path, PathBuf};

#[cfg(any(target_os = "windows", target_os = "macos"))]
const KICAD_DOCUMENTS_SUBDIR: &str = "KiCad";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const KICAD_DOCUMENTS_SUBDIR: &str = "kicad";

pub(super) fn expand_with(
    uri: &str,
    kiprjmod: Option<&Path>,
    process_var: impl Fn(&str) -> Option<std::ffi::OsString>,
    configured: &std::collections::HashMap<String, String>,
    documents_dir: Option<&Path>,
    install_dirs: impl Fn(&str) -> Vec<PathBuf>,
) -> Option<PathBuf> {
    let Some(rest) = uri.strip_prefix("${") else {
        return (!uri.is_empty()).then(|| PathBuf::from(uri));
    };
    let close = rest.find('}')?;
    let var = &rest[..close];
    let tail = rest[close + 1..].trim_start_matches(['/', '\\']);

    // ${KIPRJMOD} is the project directory — resolved from the table's own
    // location, not the environment: KiCad sets it per open project at
    // runtime, so an exported value (if any) may belong to a different
    // project than the table being read. Project-scoped registrations are
    // the default for register_footprint_library, so this is the common
    // case for user-registered libraries, not an edge.
    if var == "KIPRJMOD" {
        let p = kiprjmod?.join(tail);
        return p.exists().then_some(p);
    }

    // KiCad selects a built-in's effective source before resolving its target.
    // An explicit override must never fall through to a same-name default
    // library merely because that override's target is absent.
    // See KiCad 10.0.6 COMMON_SETTINGS::InitializeEnvironment and
    // PATHS::GetDefault3rdPartyPath. Keep other variables' legacy behavior.
    let version = match var {
        "KICAD10_3RD_PARTY" => Some("10.0"),
        "KICAD9_3RD_PARTY" => Some("9.0"),
        "KICAD8_3RD_PARTY" => Some("8.0"),
        _ => None,
    };
    if let Some(version) = version {
        let base = process_var(var)
            // KiCad InitializeEnvironment ignores empty exported values.
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| configured.get(var).map(PathBuf::from))
            .or_else(|| {
                documents_dir.map(|dir| {
                    dir.join(KICAD_DOCUMENTS_SUBDIR)
                        .join(version)
                        .join("3rdparty")
                })
            })?;
        let path = base.join(tail);
        return path.exists().then_some(path);
    }

    // var_os, not var: `var` treats a non-Unicode value as absent, which would
    // send a perfectly good ${KICAD*_DIR} down the install-root guess path.
    if let Some(base) = process_var(var) {
        let p = PathBuf::from(base).join(tail);
        if p.exists() {
            return Some(p);
        }
    }

    // User path variables (Preferences → Configure Paths) are stored in
    // kicad_common.json and are NOT process environment variables, so the
    // var_os lookup above can never see them. They are the normal way to write
    // a portable lib-table — `${MY_PARTS}/house.kicad_sym` — and without this
    // every such entry resolved to None and the library was invisible.
    // Credit to @JYPochez (#172) for finding this.
    if let Some(base) = configured.get(var) {
        let p = PathBuf::from(base).join(tail);
        if p.exists() {
            return Some(p);
        }
    }

    // e.g. KICAD10_FOOTPRINT_DIR -> "footprints"
    let kind = if var.ends_with("_FOOTPRINT_DIR") {
        "footprints"
    } else if var.ends_with("_SYMBOL_DIR") {
        "symbols"
    } else if var.ends_with("_3DMODEL_DIR") {
        "3dmodels"
    } else {
        return None;
    };

    install_dirs(kind)
        .into_iter()
        .map(|base| base.join(tail))
        .find(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn ailab49_empty_export_uses_configured_or_default_root() {
        let tmp = tempfile::tempdir().unwrap();
        let default = tmp
            .path()
            .join(KICAD_DOCUMENTS_SUBDIR)
            .join("10.0/3rdparty/Parts.pretty");
        let configured = tmp.path().join("configured");
        std::fs::create_dir_all(&default).unwrap();
        std::fs::create_dir_all(configured.join("Parts.pretty")).unwrap();
        let uri = "${KICAD10_3RD_PARTY}/Parts.pretty";
        assert_eq!(
            resolve(uri, Some(Path::new("")), None, Some(tmp.path())),
            Some(default)
        );
        assert_eq!(
            resolve(
                uri,
                Some(Path::new("")),
                Some(&configured),
                Some(tmp.path())
            ),
            Some(configured.join("Parts.pretty"))
        );
    }

    #[test]
    fn ailab49_missing_override_does_not_select_default_library() {
        let tmp = tempfile::tempdir().unwrap();
        let default = tmp
            .path()
            .join(KICAD_DOCUMENTS_SUBDIR)
            .join("10.0/3rdparty/Parts.pretty");
        std::fs::create_dir_all(&default).unwrap();
        let missing = tmp.path().join("explicit-but-missing");
        let uri = "${KICAD10_3RD_PARTY}/Parts.pretty";
        assert_eq!(
            resolve(uri, Some(&missing), None, Some(tmp.path())),
            None,
            "an exported override must not silently select the default library"
        );
        assert_eq!(
            resolve(uri, None, Some(&missing), Some(tmp.path())),
            None,
            "a configured override must not silently select the default library"
        );
    }

    fn resolve(
        uri: &str,
        process: Option<&Path>,
        configured: Option<&Path>,
        documents: Option<&Path>,
    ) -> Option<PathBuf> {
        let vars: HashMap<String, String> = configured
            .map(|p| ("KICAD10_3RD_PARTY".into(), p.to_string_lossy().into_owned()))
            .into_iter()
            .collect();
        expand_with(
            uri,
            None,
            |_| process.map(|p| p.as_os_str().to_owned()),
            &vars,
            documents,
            |_| vec![],
        )
    }

    #[test]
    fn ailab49_default_third_party_root_with_spaces() {
        let tmp = tempfile::tempdir().unwrap();
        let pretty = tmp
            .path()
            .join(KICAD_DOCUMENTS_SUBDIR)
            .join("10.0/3rdparty/footprints/Vendor Parts.pretty");
        std::fs::create_dir_all(&pretty).unwrap();
        assert_eq!(
            expand_with(
                "${KICAD10_3RD_PARTY}/footprints/Vendor Parts.pretty",
                None,
                |_| None,
                &Default::default(),
                Some(tmp.path()),
                |_| vec![]
            ),
            Some(pretty)
        );
    }

    #[test]
    fn ailab49_preserves_exported_configured_default_precedence() {
        let tmp = tempfile::tempdir().unwrap();
        let exported = tmp.path().join("exported");
        let configured = tmp.path().join("configured");
        let default = tmp
            .path()
            .join(KICAD_DOCUMENTS_SUBDIR)
            .join("10.0/3rdparty");
        for root in [&exported, &configured, &default] {
            std::fs::create_dir_all(root.join("Library.pretty")).unwrap();
        }
        let uri = "${KICAD10_3RD_PARTY}/Library.pretty";
        assert_eq!(
            resolve(uri, Some(&exported), Some(&configured), Some(tmp.path())),
            Some(exported.join("Library.pretty"))
        );
        assert_eq!(
            resolve(uri, None, Some(&configured), Some(tmp.path())),
            Some(configured.join("Library.pretty"))
        );
        // For third-party roots, a missing explicit target must refuse even when
        // a lower-priority configured/default library exists.
        assert_eq!(
            resolve(
                uri,
                Some(&tmp.path().join("absent")),
                Some(&configured),
                Some(tmp.path())
            ),
            None
        );
        assert_eq!(
            resolve(uri, None, None, Some(tmp.path())),
            Some(default.join("Library.pretty"))
        );
    }

    #[test]
    fn ailab49_missing_and_unknown_paths_stay_unresolved() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve(
                "${KICAD10_3RD_PARTY}/Missing.pretty",
                None,
                None,
                Some(tmp.path())
            ),
            None
        );
        assert_eq!(
            resolve("${KICAD10_3RD_PARTY}/Missing.pretty", None, None, None),
            None
        );
        assert_eq!(
            resolve("${UNKNOWN}/Missing.pretty", None, None, Some(tmp.path())),
            None
        );
        assert_eq!(
            resolve(
                "${KICAD10_3RD_PARTY/Missing.pretty",
                None,
                None,
                Some(tmp.path())
            ),
            None
        );
    }

    #[test]
    fn ailab49_default_root_is_version_specific() {
        let tmp = tempfile::tempdir().unwrap();
        let pretty = tmp
            .path()
            .join(KICAD_DOCUMENTS_SUBDIR)
            .join("9.0/3rdparty/Parts.pretty");
        std::fs::create_dir_all(&pretty).unwrap();
        assert_eq!(
            resolve(
                "${KICAD9_3RD_PARTY}/Parts.pretty",
                None,
                None,
                Some(tmp.path())
            ),
            Some(pretty)
        );
        assert_eq!(
            resolve(
                "${KICAD10_3RD_PARTY}/Parts.pretty",
                None,
                None,
                Some(tmp.path())
            ),
            None
        );
    }

    #[test]
    fn ailab49_project_context_and_stock_footprint_resolution_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let pretty = tmp.path().join("Parts With Spaces.pretty");
        std::fs::create_dir_all(&pretty).unwrap();
        let vars = HashMap::new();
        let project = |project| {
            expand_with(
                "${KIPRJMOD}/Parts With Spaces.pretty",
                project,
                |_| Some("wrong project".into()),
                &vars,
                None,
                |_| vec![],
            )
        };
        assert_eq!(project(Some(tmp.path())), Some(pretty.clone()));
        assert_eq!(project(None), None);
        assert_eq!(
            expand_with(
                "${KICAD10_FOOTPRINT_DIR}/Parts With Spaces.pretty",
                None,
                |_| None,
                &vars,
                None,
                |kind| {
                    assert_eq!(kind, "footprints");
                    vec![tmp.path().to_path_buf()]
                }
            ),
            Some(pretty)
        );
    }

    #[cfg(windows)]
    #[test]
    fn ailab49_windows_third_party_separators() {
        let tmp = tempfile::tempdir().unwrap();
        let pretty = tmp
            .path()
            .join(KICAD_DOCUMENTS_SUBDIR)
            .join("10.0/3rdparty/footprints/Parts With Spaces.pretty");
        std::fs::create_dir_all(&pretty).unwrap();
        assert_eq!(
            resolve(
                r"${KICAD10_3RD_PARTY}\footprints\Parts With Spaces.pretty",
                None,
                None,
                Some(tmp.path())
            ),
            Some(pretty)
        );
    }
}
