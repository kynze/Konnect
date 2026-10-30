//! Public dispatch tests: all file mutations here are performed by Konnect tools.
use super::*;
use crate::mcp::handler::McpHandler;
use crate::tools::ServerConfig;

async fn server() -> McpHandler {
    McpHandler::new(ServerConfig {
        eager_toolsets: true,
        ..Default::default()
    })
    .await
    .unwrap()
}

async fn call(server: &McpHandler, name: &str, args: serde_json::Value) -> serde_json::Value {
    server
        .handle_message(json!({"jsonrpc":"2.0", "id":40, "method":"tools/call",
        "params":{"name":name,"arguments":args}}))
        .await
        .unwrap()
        .result
        .unwrap()
}

fn body(result: &serde_json::Value) -> serde_json::Value {
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

async fn create(server: &McpHandler, path: &Path, extra: serde_json::Value) -> serde_json::Value {
    let mut args = json!({"library_path":path,"name":"Sanitized","reference_prefix":"U",
        "units":[{"pins":[{"number":"1","name":"IN","type":"input"}]},
                 {"pins":[{"number":"2","name":"OUT","type":"output"}]}]});
    args.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    call(server, "create_symbol", args).await
}

#[tokio::test]
async fn served_symbol_properties_schema_is_typed() {
    let server = server().await;
    let list = server
        .handle_message(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .await
        .unwrap()
        .result
        .unwrap();
    let tools = list["tools"].as_array().unwrap();
    for name in ["create_symbol", "edit_symbol_properties"] {
        let tool = tools
            .iter()
            .find(|t| t["name"] == name)
            .expect("property tools must be served");
        let schema = &tool["inputSchema"]["properties"];
        assert_eq!(schema["footprint"]["type"], "string");
        assert_eq!(
            schema["properties"]["items"]["properties"]["value"]["type"],
            "string"
        );
        assert_eq!(schema["properties"]["items"]["additionalProperties"], false);
    }
}

#[tokio::test]
async fn served_symbol_properties_create_edit_preserve_and_repeat() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sanitized.kicad_sym");
    let text = "A \"quoted\" \\path Ω\nsecond line";
    let result = create(
        &server,
        &path,
        json!({"footprint":"Fixture:Package",
        "properties":[{"name":"MPN","value":text,"visible":true,"x":2.54,"y":-5.08,"angle":90},
                      {"name":"Manufacturer","value":"Example"}]}),
    )
    .await;
    assert_ne!(result["isError"], true, "{result}");
    let before = std::fs::read_to_string(&path).unwrap();
    let root = parse_sexp(&before).unwrap();
    let sym = root.find("symbol").unwrap();
    assert_eq!(
        sym.find_all("property")
            .iter()
            .find(|p| p.get(1).unwrap().as_str() == Some("MPN"))
            .unwrap()
            .get(2)
            .unwrap()
            .as_str(),
        Some(text)
    );
    assert_eq!(body(&result)["properties"]["Footprint"], "Fixture:Package");
    let artwork: Vec<_> = sym.find_all("symbol").into_iter().cloned().collect();
    let args = json!({"library_path":path,"symbol_name":"Sanitized","footprint":"Fixture:Replacement",
        "properties":[{"name":"MPN","value":"new","visible":false,"x":0},
                      {"name":"Rating","value":"50 V"}]});
    let edited = call(&server, "edit_symbol_properties", args.clone()).await;
    assert_ne!(edited["isError"], true, "{edited}");
    let info = body(&edited);
    assert_eq!(info["source"], "saved_file");
    assert_eq!(info["properties"]["Footprint"], "Fixture:Replacement");
    let mpn = &info["property_details"]["MPN"];
    assert_eq!(mpn["visible"], false);
    assert_eq!(mpn["x"], 0.0);
    assert_eq!(mpn["y"], -5.08);
    assert_eq!(mpn["angle"], 90.0);
    let after = std::fs::read_to_string(&path).unwrap();
    let root = parse_sexp(&after).unwrap();
    let sym = root.find("symbol").unwrap();
    assert_eq!(
        sym.find_all("symbol")
            .into_iter()
            .cloned()
            .collect::<Vec<_>>(),
        artwork
    );
    assert_eq!(
        sym.find_all("property")
            .iter()
            .filter(|p| p.get(1).unwrap().as_str() == Some("MPN"))
            .count(),
        1
    );
    assert!(after.contains("(property \"Manufacturer\" \"Example\""));
    let repeated = call(&server, "edit_symbol_properties", args).await;
    assert_ne!(repeated["isError"], true, "{repeated}");
    assert_eq!(body(&repeated)["changed"], false);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), after);
    // Repeating a create must refuse instead of duplicating the definition.
    let repeated = create(&server, &path, json!({})).await;
    assert_eq!(repeated["isError"], true);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), after);
}

#[tokio::test]
async fn served_symbol_properties_invalid_batches_leave_library_unchanged() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sanitized.kicad_sym");
    assert_ne!(create(&server, &path, json!({})).await["isError"], true);
    let before = std::fs::read(&path).unwrap();
    for properties in [
        json!(null),
        json!({"MPN":"bad"}),
        json!([{"name":"MPN","value":1}]),
        json!([{"name":"","value":"bad"}]),
        json!([{"name":"MPN","value":"ok","visible":null}]),
        json!([{"name":"MPN","value":"ok","x":null}]),
        json!([{"name":"MPN","value":"ok","angle":45}]),
        json!([{"name":"MPN","value":"ok","unknown":1}]),
        json!([{"name":"MPN","value":"ok"},{"name":"MPN","value":"duplicate"}]),
    ] {
        for tool in ["create_symbol", "edit_symbol_properties"] {
            let args = if tool == "create_symbol" {
                json!({"library_path":path,"name":"Other","reference_prefix":"U",
                "pins":[],"properties":properties})
            } else {
                json!({"library_path":path,"symbol_name":"Sanitized","properties":properties})
            };
            let result = call(&server, tool, args).await;
            assert_eq!(result["isError"], true, "{tool}: {properties}: {result}");
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
    }
    let conflict = call(
        &server,
        "edit_symbol_properties",
        json!({"library_path":path,"symbol_name":"Sanitized",
        "footprint":"A:B","properties":[{"name":"Footprint","value":"C:D"}]}),
    )
    .await;
    assert_eq!(conflict["isError"], true);
    let missing = call(
        &server,
        "edit_symbol_properties",
        json!({"library_path":path,"symbol_name":"Missing","footprint":"A:B"}),
    )
    .await;
    assert_eq!(missing["isError"], true);
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn symbol_property_patch_preserves_omitted_coordinate_precision_and_effects() {
    // Sanitized KiCad property syntax; unrelated presentation must remain exact.
    let original="(symbol \"Example\"\r\n\t(property \"MPN\" \"old\" (at -2.54 1.23456789 90) (effects (font (size 1.1 1.2) bold) (justify left) hide))\r\n\t(symbol \"Example_1_1\" (rectangle (start 0 0) (end 2 2) (stroke (width 0) (type default)) (fill (type none)))))";
    let updates = symbol_properties::parse_updates(
        &json!({"properties":[{"name":"MPN","value":"new","x":0,"visible":true}]}),
        false,
    )
    .unwrap();
    let result = symbol_properties::patch_symbol(original, &updates).unwrap();
    assert_eq!(
        result,
        original
            .replace("\"old\"", "\"new\"")
            .replace("-2.54", "0")
            .replace(" hide)", " )")
    );
    assert_eq!(
        symbol_properties::patch_symbol(&result, &updates).unwrap(),
        result
    );
}

#[tokio::test]
async fn symbol_property_edit_refuses_malformed_ambiguous_or_inherited_sources() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rejected.kicad_sym");
    for original in [
        "(kicad_symbol_lib (symbol \"Bad\" (extends \"Base\")))",
        "(kicad_symbol_lib (symbol \"Bad\") (symbol \"Bad\"))",
        "(kicad_symbol_lib (symbol \"Bad\" (property \"MPN\" \"1\") (property \"MPN\" \"2\")))",
        "(kicad_symbol_lib (symbol \"Bad\" (property \"MPN\")))",
        "(kicad_symbol_lib (symbol \"Bad\" (property \"MPN\" \"old\" (at 0 nope 0))))",
        "(kicad_symbol_lib (symbol \"Bad\" (property \"MPN\" \"old\" (at 1 2 0) (at 3 4 0))))",
        "(kicad_symbol_lib (symbol \"Bad\")) garbage",
        "(kicad_symbol_lib (symbol \"Bad\")",
        "(unrelated (symbol \"Bad\"))",
        // nom's whitespace parser rejects this layout: prove no write on refusal.
        "(kicad_symbol_lib (symbol \"Bad\" (property \"MPN\"\u{2003}\"old\" (at 1 2 90))))",
    ] {
        std::fs::write(&path, original).unwrap();
        let result = call(
            &server,
            "edit_symbol_properties",
            json!({"library_path":path,"symbol_name":"Bad","footprint":"Example:Package"}),
        )
        .await;
        assert_eq!(result["isError"], true, "{original}: {result}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }
}

#[test]
fn symbol_property_lexical_layouts_patch_only_requested_spans() {
    for original in [
        "(symbol \"Example\" (property\"MPN\" \"old\" (at 1 2 90) (effects (font (size 1 1)))))",
        "(symbol \"Example\" ( property \"MPN\" \"old\" (at 1 2 90) (effects (font (size 1 1)))))",
        "( symbol \"Example\" (property \"MPN\" \"old\" (at 1 2 90) (effects (font (size 1 1)))))",
        "(symbol\"Example\" (property \"MPN\" \"old\" (at 1 2 90) (effects (font (size 1 1)))))",
    ] {
        assert!(parse_sexp(original).is_ok(), "fixture must be parseable");
        let updates = symbol_properties::parse_updates(
            &json!({"properties":[{"name":"MPN","value":"new","x":0}]}),
            false,
        )
        .unwrap();
        let patched = symbol_properties::patch_symbol(original, &updates).unwrap();
        assert_eq!(
            patched,
            original
                .replace("\"old\"", "\"new\"")
                .replace("(at 1 ", "(at 0 "),
            "{original}"
        );
        assert_eq!(
            symbol_properties::patch_symbol(&patched, &updates).unwrap(),
            patched
        );
    }
}

#[test]
fn symbol_property_readback_reports_concurrent_replacement_as_uncertain() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("race.kicad_sym");
    let expected = "(kicad_symbol_lib (symbol \"Example\" (property \"MPN\" \"requested\")))";
    std::fs::write(&path, expected.replace("requested", "external replacement")).unwrap();
    let result = symbol_properties::readback(&path, "Example", expected)
        .expect_err("must not report another writer's state as success");
    let text = serde_json::to_value(result).unwrap();
    assert_eq!(body(&text)["error"]["kind"], "mutation_outcome_uncertain");
}

#[test]
fn symbol_property_failed_write_and_missing_readback_are_not_success() {
    let dir = tempfile::tempdir().unwrap();
    // A directory cannot be replaced by a newly created library file.
    let failure = symbol_properties::persist(dir.path(), None, "content").unwrap_err();
    assert!(failure.is_error);
    assert!(dir.path().is_dir());
    let missing = symbol_properties::readback(&dir.path().join("missing.kicad_sym"), "Example", "")
        .unwrap_err();
    assert_eq!(
        body(&serde_json::to_value(missing).unwrap())["error"]["kind"],
        "mutation_outcome_uncertain"
    );
}

#[test]
fn symbol_property_direct_validation_refuses_null_and_duplicate_fields() {
    for args in [
        json!({"properties":null}),
        json!({"properties":[{"name":"MPN","value":"v","visible":null}]}),
        json!({"properties":[{"name":"MPN","value":"v","x":null}]}),
        json!({"properties":[{"name":"MPN","value":"v","unknown":true}]}),
        json!({"properties":[{"name":"MPN","value":"v"},{"name":"MPN","value":"v2"}]}),
        json!({"properties":[{"name":"MPN","value":"v","angle":45}]}),
        json!({"footprint":null}),
    ] {
        assert!(
            symbol_properties::parse_updates(&args, false).is_err(),
            "{args}"
        );
    }
    for name in ["Reference", "Value", "Datasheet", "Footprint"] {
        assert!(symbol_properties::parse_updates(
            &json!({"properties":[{"name":name,"value":"v"}]}),
            true
        )
        .is_err());
    }
}

#[test]
fn symbol_property_compare_and_swap_refuses_stale_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conflict.kicad_sym");
    std::fs::write(&path, "newer source").unwrap();
    let result =
        symbol_properties::persist(&path, Some("older source"), "replacement").unwrap_err();
    assert!(result.is_error);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "newer source");
}
