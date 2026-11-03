//! AILAB-41 public dispatch tests. Every file mutation is performed by Konnect
//! tools; fixtures are synthetic and live in temporary directories.
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
        .handle_message(json!({"jsonrpc":"2.0","id":41,"method":"tools/call",
            "params":{"name":name,"arguments":args}}))
        .await
        .unwrap()
        .result
        .unwrap()
}

fn body(result: &serde_json::Value) -> serde_json::Value {
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

fn is_error(result: &serde_json::Value) -> bool {
    result["isError"].as_bool().unwrap_or(false)
}

fn pads() -> serde_json::Value {
    json!([
        {"number":"1","type":"smd","shape":"rect","x":-1.0,"y":0.0,"width":1.0,"height":1.2,
         "solder_mask_margin":0.075,"solder_paste_margin":-0.05,"solder_paste_margin_ratio":-0.1},
        {"number":"2","type":"smd","shape":"roundrect","x":1.0,"y":0.0,"width":1.0,"height":1.2,
         "roundrect_rratio":0.25,"solder_mask_margin":0.125,
         "solder_paste_margin":0.025,"solder_paste_margin_ratio":0.05},
        {"number":"3","type":"smd","shape":"rect","x":3.0,"y":0.0,"width":1.0,"height":1.2}
    ])
}

async fn make(server: &McpHandler, dir: &Path) -> PathBuf {
    let path = dir.join("Sanitized.kicad_mod");
    let r = call(
        server,
        "create_footprint",
        json!({"output":path,"name":"Sanitized","pads":pads()}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    path
}

async fn info(server: &McpHandler, path: &Path) -> serde_json::Value {
    let r = call(
        server,
        "get_footprint_info",
        json!({"footprint_path":path,"include_pads":true}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    body(&r)
}

async fn plain_pad_footprint(server: &McpHandler, dir: &Path) -> PathBuf {
    let path = dir.join("Plain.kicad_mod");
    let r = call(
        server,
        "create_footprint",
        json!({"output":path,"name":"Plain","pads":[
            {"number":"1","type":"smd","shape":"rect","x":0,"y":0,"width":1,"height":1},
            {"number":"2","type":"smd","shape":"rect","x":2,"y":0,"width":1,"height":1}]}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    path
}

#[tokio::test]
async fn served_schemas_expose_typed_mask_and_paste_fields() {
    let server = server().await;
    let list = server
        .handle_message(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .await
        .unwrap()
        .result
        .unwrap();
    let tools = list["tools"].as_array().unwrap();
    let find = |n: &str| tools.iter().find(|t| t["name"] == n).unwrap().clone();
    let create = find("create_footprint");
    let edit = find("edit_footprint_pad");
    let create_props = &create["inputSchema"]["properties"]["pads"]["items"]["properties"];
    let edit_props = &edit["inputSchema"]["properties"];
    for field in [
        "solder_mask_margin",
        "solder_paste_margin",
        "solder_paste_margin_ratio",
    ] {
        assert!(
            create_props[field]["type"].is_string() || create_props[field]["type"].is_array(),
            "create_footprint must declare {field}"
        );
        assert_eq!(
            edit_props[field]["type"],
            json!(["number", "null"]),
            "edit_footprint_pad must declare {field} as number|null"
        );
    }
}

#[tokio::test]
async fn create_writes_exact_nanometre_values_and_inherits_when_omitted() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = make(&server, dir.path()).await;
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("(solder_mask_margin 0.075)"), "{text}");
    assert!(text.contains("(solder_mask_margin 0.125)"), "{text}");
    assert!(text.contains("(solder_paste_margin -0.05)"), "{text}");
    assert!(text.contains("(solder_paste_margin_ratio -0.1)"), "{text}");
    assert!(text.contains("(solder_paste_margin 0.025)"), "{text}");
    assert!(text.contains("(solder_paste_margin_ratio 0.05)"), "{text}");
    assert!(
        !text.contains("margin 0)") && !text.contains("ratio 0)"),
        "no zero tokens"
    );
    assert!(!text.contains("0.0749"), "no nanometre truncation: {text}");
    assert_eq!(text.matches("(solder_mask_margin").count(), 2);

    let got = info(&server, &path).await;
    let p1 = &got["pads"][0]["mask_paste"];
    assert_eq!(p1["solder_mask_margin"]["local"], 0.075);
    assert_eq!(p1["solder_mask_margin"]["source"], "pad");
    assert_eq!(p1["solder_mask_margin"]["effective"], 0.075);
    assert_eq!(p1["solder_paste_margin_ratio"]["local"], -0.1);
    let p2 = &got["pads"][1]["mask_paste"];
    assert_eq!(p2["solder_mask_margin"]["local"], 0.125);
    assert_eq!(p2["solder_paste_margin"]["local"], 0.025);
    assert_eq!(p2["solder_paste_margin"]["source"], "pad");
    assert_eq!(p2["solder_paste_margin_ratio"]["local"], 0.05);
    let p3 = &got["pads"][2]["mask_paste"];
    for field in [
        "solder_mask_margin",
        "solder_paste_margin",
        "solder_paste_margin_ratio",
    ] {
        assert!(p3[field]["local"].is_null(), "{field}");
        assert_eq!(p3[field]["source"], "unknown", "{field}");
        assert!(
            p3[field]["effective"].is_null(),
            "{field}: effective must stay unknown without board context"
        );
    }
}

#[tokio::test]
async fn edit_preserves_unrelated_settings_replaces_in_place_and_null_restores_inheritance() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = make(&server, dir.path()).await;
    // Unrelated edit: mask/paste of pad 1 are untouched.
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":path,"pad_number":"1","width":1.1}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let got = info(&server, &path).await;
    assert_eq!(
        got["pads"][0]["mask_paste"]["solder_mask_margin"]["local"],
        0.075
    );
    assert_eq!(
        got["pads"][0]["mask_paste"]["solder_paste_margin_ratio"]["local"],
        -0.1
    );

    // A change to another value replaces the override in place.
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":path,"pad_number":"1","solder_mask_margin":0.05}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let got = info(&server, &path).await;
    let m = &got["pads"][0]["mask_paste"]["solder_mask_margin"];
    assert_eq!(m["local"], 0.05);
    assert_eq!(m["source"], "pad");
    assert_eq!(
        std::fs::read_to_string(&path)
            .unwrap()
            .matches("(solder_mask_margin")
            .count(),
        2
    );

    // Add to a pad that inherited, with +0.075 exactly.
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":path,"pad_number":"3","solder_mask_margin":0.075,
               "solder_paste_margin_ratio":0.2}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let after_add = std::fs::read_to_string(&path).unwrap();
    assert!(after_add.matches("(solder_mask_margin 0.075)").count() >= 1);
    let got = info(&server, &path).await;
    assert_eq!(
        got["pads"][2]["mask_paste"]["solder_mask_margin"]["local"],
        0.075
    );
    assert_eq!(
        got["pads"][2]["mask_paste"]["solder_paste_margin_ratio"]["local"],
        0.2
    );

    // Clearing restores inheritance; the second repeat is byte-identical.
    let clear = json!({"footprint_path":path,"pad_number":"3",
        "solder_mask_margin":null,"solder_paste_margin_ratio":null});
    let r = call(&server, "edit_footprint_pad", clear.clone()).await;
    assert!(!is_error(&r), "{r}");
    let once = std::fs::read_to_string(&path).unwrap();
    let r = call(&server, "edit_footprint_pad", clear).await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), once, "idempotent");
    let got = info(&server, &path).await;
    assert!(got["pads"][2]["mask_paste"]["solder_mask_margin"]["local"].is_null());
    assert_eq!(
        got["pads"][2]["mask_paste"]["solder_mask_margin"]["source"],
        "unknown"
    );
}

#[tokio::test]
async fn set_then_clear_round_trips_the_file_byte_for_byte() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = plain_pad_footprint(&server, dir.path()).await;
    let original = std::fs::read_to_string(&path).unwrap();
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":path,"pad_number":"2","solder_mask_margin":0.125,
               "solder_paste_margin":-0.05,"solder_paste_margin_ratio":-0.1}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let edited = std::fs::read_to_string(&path).unwrap();
    assert_ne!(edited, original);
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":path,"pad_number":"2","solder_mask_margin":null,
               "solder_paste_margin":null,"solder_paste_margin_ratio":null}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        original,
        "unrelated geometry/artwork and other pads must be untouched"
    );
}

#[tokio::test]
async fn invalid_values_are_refused_before_any_write() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = make(&server, dir.path()).await;
    let original = std::fs::read(&path).unwrap();
    let cases = [
        ("solder_mask_margin", json!("0.1")),
        ("solder_mask_margin", json!(true)),
        ("solder_mask_margin", json!(1e9)),
        ("solder_paste_margin", json!([0.1])),
        ("solder_paste_margin", json!(-11)),
        ("solder_paste_margin_ratio", json!(1.5)),
        ("solder_paste_margin_ratio", json!(-1.01)),
        ("solder_paste_margin_ratio", json!({"v": 1})),
    ];
    for (field, value) in cases {
        let mut args = json!({"footprint_path":path,"pad_number":"3","width":2.0});
        args[field] = value.clone();
        let r = call(&server, "edit_footprint_pad", args).await;
        assert!(is_error(&r), "{field}={value} must be refused: {r}");
        assert_eq!(std::fs::read(&path).unwrap(), original, "{field}={value}");
    }
    // Unknown pad with a valid value: still no write.
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":path,"pad_number":"99","solder_mask_margin":0.1}),
    )
    .await;
    assert!(is_error(&r));
    assert_eq!(std::fs::read(&path).unwrap(), original);

    // create refuses too, leaving an existing file untouched.
    let before = std::fs::read(&path).unwrap();
    let r = call(
        &server,
        "create_footprint",
        json!({"output":path,"name":"X","pads":[
            {"number":"1","type":"smd","shape":"rect","x":0,"y":0,"width":1,"height":1,
             "solder_paste_margin_ratio":"bad"}]}),
    )
    .await;
    assert!(is_error(&r), "{r}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[tokio::test]
async fn footprint_level_settings_are_reported_as_the_effective_source() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("FpLevel.kicad_mod");
    std::fs::write(
        &path,
        "(footprint \"FpLevel\"\n  (version 20240108)\n  (generator \"konnect\")\n  (layer \"F.Cu\")\n  (solder_mask_margin 0.05)\n  (pad \"1\" smd rect (at 0 0) (size 1 1) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\"))\n)",
    )
    .unwrap();
    let got = info(&server, &path).await;
    let m = &got["pads"][0]["mask_paste"]["solder_mask_margin"];
    assert!(m["local"].is_null());
    assert_eq!(m["source"], "footprint");
    assert_eq!(m["effective"], 0.05);
    let p = &got["pads"][0]["mask_paste"]["solder_paste_margin"];
    assert_eq!(p["source"], "unknown");
    assert!(p["effective"].is_null());
}

#[tokio::test]
async fn unreadable_setting_fails_readback_instead_of_being_dropped() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Broken.kicad_mod");
    std::fs::write(
        &path,
        "(footprint \"Broken\"\n  (version 20240108)\n  (layer \"F.Cu\")\n  (pad \"1\" smd rect (at 0 0) (size 1 1) (layers \"F.Cu\") (solder_mask_margin abc))\n)",
    )
    .unwrap();
    let r = call(
        &server,
        "get_footprint_info",
        json!({"footprint_path":path,"include_pads":true}),
    )
    .await;
    assert!(is_error(&r), "{r}");
}

#[tokio::test]
async fn tokens_land_where_kicad_writes_them_in_a_native_style_pad() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Native.kicad_mod");
    let source = include_str!("../../tests/fixtures/pad_zone_connect_kicad10.kicad_mod");
    std::fs::write(&path, source).unwrap();
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":path,"pad_number":"1","match_all":true,
               "solder_mask_margin":0.075,"solder_paste_margin_ratio":-0.1}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    // The fixture may carry CRLF; the product reuses the file's own separators.
    let written = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\r\n", "\n");
    assert_eq!(
        written
            .matches("\t\t(solder_mask_margin 0.075)\n\t\t(solder_paste_margin_ratio -0.1)\n\t\t(zone_connect 2)\n")
            .count(),
        3,
        "mask/paste group goes immediately before zone_connect on every pad: {written}"
    );
    assert_eq!(body(&r)["mask_paste"]["solder_mask_margin"]["local"], 0.075);
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":path,"pad_number":"1","match_all":true,
               "solder_mask_margin":null,"solder_paste_margin_ratio":null}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
}

#[tokio::test]
async fn zero_is_refused_everywhere_and_never_written() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = make(&server, dir.path()).await;
    let original = std::fs::read(&path).unwrap();
    for field in [
        "solder_mask_margin",
        "solder_paste_margin",
        "solder_paste_margin_ratio",
    ] {
        for zero in [json!(0), json!(0.0), json!(1e-9), json!(-4e-7)] {
            let mut args = json!({"footprint_path":path,"pad_number":"3"});
            args[field] = zero.clone();
            let r = call(&server, "edit_footprint_pad", args).await;
            assert!(is_error(&r), "{field}={zero} must be refused: {r}");
            let text = r["content"][0]["text"].as_str().unwrap();
            assert!(
                text.contains(field) && text.contains("KiCad 10.0.6"),
                "{text}"
            );
            assert_eq!(std::fs::read(&path).unwrap(), original, "{field}={zero}");
        }
        let mut pad = json!({"number":"1","type":"smd","shape":"rect","x":0,"y":0,
                             "width":1,"height":1});
        pad[field] = json!(0);
        let r = call(
            &server,
            "create_footprint",
            json!({"output":path,"name":"X","pads":[pad]}),
        )
        .await;
        assert!(is_error(&r), "create {field}=0 must be refused: {r}");
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
}

#[tokio::test]
async fn an_existing_zero_token_reads_back_as_inherited_like_kicad() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ZeroToken.kicad_mod");
    std::fs::write(
        &path,
        "(footprint \"ZeroToken\"\n  (version 20240108)\n  (layer \"F.Cu\")\n  (solder_paste_margin 0)\n  (pad \"1\" smd rect (at 0 0) (size 1 1) (layers \"F.Cu\" \"F.Mask\") (solder_mask_margin 0) (solder_paste_margin 0))\n)",
    )
    .unwrap();
    let got = info(&server, &path).await;
    let m = &got["pads"][0]["mask_paste"]["solder_mask_margin"];
    assert!(m["local"].is_null(), "{m}");
    assert_eq!(m["source"], "unknown");
    assert!(m["effective"].is_null());
    assert_eq!(m["file_token_zero"], true);
    let p = &got["pads"][0]["mask_paste"]["solder_paste_margin"];
    assert_eq!(p["file_token_zero"], true);
    assert_eq!(p["footprint_token_zero"], true);
    assert_eq!(p["source"], "unknown");
    assert!(got["pads"][0]["mask_paste"]["solder_paste_margin_ratio"]
        .get("file_token_zero")
        .is_none());
}

/// Every `(solder_… value)` token in the text, in order.
fn token_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("(solder_") {
        let end = rest[start..]
            .find(')')
            .map_or(rest.len(), |i| start + i + 1);
        out.push(rest[start..end].to_string());
        rest = &rest[end..];
    }
    out
}

#[tokio::test]
async fn an_unrelated_edit_keeps_every_existing_token_byte_for_byte() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = make(&server, dir.path()).await;
    let before = std::fs::read_to_string(&path).unwrap();
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":path,"pad_number":"1","x":-1.5}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(token_lines(&before), token_lines(&after));
    // Only pad 1's position text changed.
    assert_eq!(before.replacen("(at -1 0)", "(at -1.5 0)", 1), after);
}

#[tokio::test]
async fn a_malformed_token_elsewhere_does_not_block_an_unrelated_setting() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Mixed.kicad_mod");
    std::fs::write(
        &path,
        "(footprint \"Mixed\"
  (version 20240108)
  (layer \"F.Cu\")
  (pad \"1\" smd rect (at 0 0) (size 1 1) (layers \"F.Cu\" \"F.Mask\") (solder_paste_margin foo))
)",
    )
    .unwrap();
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":path,"pad_number":"1","solder_mask_margin":0.05}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("(solder_mask_margin 0.05)"));
    assert!(text.contains("(solder_paste_margin foo)"), "left as it was");
}

#[tokio::test]
async fn setting_replaces_the_first_token_and_collapses_duplicates() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Dup.kicad_mod");
    std::fs::write(
        &path,
        "(footprint \"Dup\"
  (version 20240108)
  (layer \"F.Cu\")
  (pad \"1\" smd rect (at 0 0) (size 1 1) (layers \"F.Cu\" \"F.Mask\") (solder_mask_margin 0.01) (solder_mask_margin 0.02))
)",
    )
    .unwrap();
    // KiCad keeps the last duplicate, and readback follows it.
    let got = info(&server, &path).await;
    assert_eq!(
        got["pads"][0]["mask_paste"]["solder_mask_margin"]["local"],
        0.02
    );
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":path,"pad_number":"1","solder_mask_margin":0.125}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.matches("(solder_mask_margin").count(), 1, "{text}");
    assert!(text.contains("(solder_mask_margin 0.125)"));
}

#[tokio::test]
async fn exponent_and_sub_nanometre_input_is_rounded_to_whole_nanometres() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Round.kicad_mod");
    let r = call(
        &server,
        "create_footprint",
        json!({"output":path,"name":"Round","pads":[
            {"number":"1","type":"smd","shape":"rect","x":0,"y":0,"width":1,"height":1,
             "solder_mask_margin":7.5e-2,"solder_paste_margin":-1.00000049e-1,
             "solder_paste_margin_ratio":-5e-2}]}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("(solder_mask_margin 0.075)"), "{text}");
    assert!(text.contains("(solder_paste_margin -0.1)"), "{text}");
    assert!(text.contains("(solder_paste_margin_ratio -0.05)"), "{text}");
    assert!(!text.contains("e-0"), "no exponent forms written: {text}");
}

#[tokio::test]
async fn edit_response_reports_pad_local_values_without_inventing_effective_ones() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = make(&server, dir.path()).await;
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":path,"pad_number":"3","solder_mask_margin":0.05}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let mp = &body(&r)["mask_paste"];
    assert_eq!(mp["solder_mask_margin"]["local"], 0.05);
    assert!(mp["solder_paste_margin"]["local"].is_null());
    assert!(mp["solder_mask_margin"].get("effective").is_none(), "{mp}");
    // match_all over identical pads reports one object, wherever the pads sit.
    let two = dir.path().join("Two.kicad_mod");
    let r = call(
        &server,
        "create_footprint",
        json!({"output":two,"name":"Two","pads":[
            {"number":"1","type":"smd","shape":"rect","x":0,"y":0,"width":1,"height":1},
            {"number":"2","type":"smd","shape":"rect","x":2,"y":0,"width":1,"height":1},
            {"number":"1","type":"smd","shape":"rect","x":4,"y":0,"width":1,"height":1}]}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":two,"pad_number":"1","match_all":true,"solder_mask_margin":0.05}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    assert!(body(&r)["mask_paste"].is_object(), "{}", body(&r));
}
