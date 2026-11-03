//! AILAB-42 public dispatch tests. Every file mutation is performed by Konnect
//! tools; fixtures are synthetic (two were generated natively by KiCad 10.0.6).
use super::*;
use crate::mcp::handler::McpHandler;
use crate::tools::ServerConfig;

const NATIVE_FRONT_INNER_BACK: &str =
    include_str!("../../tests/fixtures/ailab42_padstack_front_inner_back_native.kicad_mod");
const NATIVE_CUSTOM: &str =
    include_str!("../../tests/fixtures/ailab42_padstack_custom_native.kicad_mod");

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
        .handle_message(json!({"jsonrpc":"2.0","id":42,"method":"tools/call",
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

fn err_text(result: &serde_json::Value) -> String {
    result["content"][0]["text"].as_str().unwrap().to_string()
}

fn layer(shape: &str, w: f64, h: f64) -> serde_json::Value {
    json!({"shape":shape,"width":w,"height":h})
}

fn fixture_stack() -> serde_json::Value {
    json!({"mode":"front_inner_back",
        "front":layer("rect",1.6,1.6),
        "inner":layer("circle",1.2,1.2),
        "back":layer("rect",1.6,1.6)})
}

fn th_pad(number: &str, x: f64, extra: serde_json::Value) -> serde_json::Value {
    let mut pad = json!({"number":number,"type":"thru_hole","shape":"rect","x":x,"y":0,
        "width":1.6,"height":1.6,"drill":0.8});
    pad.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    pad
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

async fn make_plain(server: &McpHandler, dir: &Path) -> PathBuf {
    let path = dir.join("Plain.kicad_mod");
    let r = call(
        server,
        "create_footprint",
        json!({"output":path,"name":"Plain","pads":[
            th_pad("1", 0.0, json!({"solder_mask_margin":0.075})),
            th_pad("2", 3.0, json!({}))]}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    path
}

#[tokio::test]
async fn served_schemas_expose_the_typed_padstack() {
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
    let c = &create["inputSchema"]["properties"]["pads"]["items"]["properties"]["padstack"];
    let e = &edit["inputSchema"]["properties"]["padstack"];
    assert_eq!(c["properties"]["mode"]["enum"], json!(["front_inner_back"]));
    assert_eq!(e["type"], json!(["object", "null"]));
    for side in ["front", "inner", "back"] {
        assert_eq!(
            c["properties"][side]["properties"]["shape"]["enum"],
            json!(["circle", "rect", "oval", "roundrect"]),
            "{side}"
        );
        assert_eq!(c["required"], json!(["mode", "front", "inner", "back"]));
    }
}

#[tokio::test]
async fn create_authors_per_layer_copper_and_reads_it_back_faithfully() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Stack.kicad_mod");
    let r = call(
        &server,
        "create_footprint",
        json!({"output":path,"name":"Stack","pads":[
            th_pad("1", 0.0, json!({"padstack":fixture_stack(),"solder_mask_margin":0.075})),
            th_pad("2", 3.0, json!({}))]}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.matches("(padstack").count(), 1, "{text}");
    assert!(text.contains("(mode front_inner_back)"), "{text}");
    assert!(text.contains("(layer \"Inner\""), "{text}");
    assert!(text.contains("(layer \"B.Cu\""), "{text}");
    assert!(
        text.contains("(solder_mask_margin 0.075)"),
        "mask kept: {text}"
    );

    let got = info(&server, &path).await;
    let p1 = &got["pads"][0];
    assert_eq!(p1["padstack"]["mode"], "front_inner_back");
    assert_eq!(p1["padstack"]["front"]["shape"], "rect");
    assert_eq!(p1["padstack"]["inner"]["shape"], "circle");
    assert_eq!(p1["padstack"]["inner"]["size"], json!({"x":1.2,"y":1.2}));
    assert_eq!(p1["padstack"]["back"]["shape"], "rect");
    assert_eq!(p1["drill"]["size"], json!({"x":0.8,"y":0.8}));
    // copper_layers must describe each layer, not copy the header onto *.Cu.
    let cl = p1["copper_layers"].as_array().unwrap();
    let by = |l: &str| {
        cl.iter()
            .find(|c| c["layer"] == l)
            .unwrap_or_else(|| panic!("{l}: {cl:?}"))
    };
    assert_eq!(by("F.Cu")["shape"], "rect");
    assert_eq!(by("Inner")["shape"], "circle");
    assert_eq!(by("B.Cu")["shape"], "rect");
    assert!(cl.iter().all(|c| c["layer"] != "*.Cu"), "{cl:?}");
    assert_eq!(p1["mask_paste"]["solder_mask_margin"]["local"], 0.075);
    // The ordinary pad is unchanged and truthful.
    assert_eq!(got["pads"][1]["padstack"]["mode"], "normal");
    assert_eq!(got["pads"][1]["copper_layers"][0]["layer"], "*.Cu");
}

#[tokio::test]
async fn create_roundrect_layers_keep_their_own_ratio() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Rr.kicad_mod");
    let mut stack = fixture_stack();
    stack["front"] = json!({"shape":"roundrect","width":1.6,"height":1.6,"roundrect_rratio":0.2});
    stack["back"] = json!({"shape":"roundrect","width":1.6,"height":1.6,"roundrect_rratio":0.25});
    let r = call(
        &server,
        "create_footprint",
        json!({"output":path,"name":"Rr","pads":[th_pad("1", 0.0,
            json!({"shape":"roundrect","roundrect_rratio":0.2,"padstack":stack}))]}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let got = info(&server, &path).await;
    let ps = &got["pads"][0]["padstack"];
    assert_eq!(ps["front"]["corner_rounding_ratio"], 0.2);
    assert_eq!(
        ps["inner"]["corner_rounding_ratio"],
        serde_json::Value::Null
    );
    assert_eq!(ps["back"]["corner_rounding_ratio"], 0.25);
}

#[tokio::test]
async fn edit_sets_a_padstack_and_preserves_everything_else() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = make_plain(&server, dir.path()).await;
    let before = std::fs::read_to_string(&path).unwrap();
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":path,"pad_number":"1","padstack":fixture_stack()}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let after = std::fs::read_to_string(&path).unwrap();
    // Pad 2 and all artwork outside pad 1 are byte-identical.
    let tail = |s: &str| s[s.find("(pad \"2\"").unwrap()..].to_string();
    assert_eq!(tail(&before), tail(&after));
    assert!(after.contains("(solder_mask_margin 0.075)"));
    let got = info(&server, &path).await;
    assert_eq!(got["pads"][0]["padstack"]["inner"]["shape"], "circle");
    assert_eq!(got["pads"][0]["drill"]["size"], json!({"x":0.8,"y":0.8}));
    assert_eq!(
        got["pads"][0]["mask_paste"]["solder_mask_margin"]["local"],
        0.075
    );
    assert_eq!(got["pads"][1]["padstack"]["mode"], "normal");

    // Repeating the identical edit changes nothing.
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":path,"pad_number":"1","padstack":fixture_stack()}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), after, "idempotent");

    // A different stack replaces the previous one in place (one block only).
    let mut other = fixture_stack();
    other["inner"] = layer("oval", 1.4, 1.0);
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":path,"pad_number":"1","padstack":other}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.matches("(padstack").count(), 1);
    let got = info(&server, &path).await;
    assert_eq!(got["pads"][0]["padstack"]["inner"]["shape"], "oval");
    assert_eq!(
        got["pads"][0]["padstack"]["inner"]["size"],
        json!({"x":1.4,"y":1.0})
    );
}

#[tokio::test]
async fn edit_null_returns_a_readable_stack_to_a_normal_pad() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = make_plain(&server, dir.path()).await;
    let original = std::fs::read_to_string(&path).unwrap();
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":path,"pad_number":"1","padstack":fixture_stack()}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":path,"pad_number":"1","padstack":null}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    let got = info(&server, &path).await;
    assert_eq!(got["pads"][0]["padstack"]["mode"], "normal");
    // Clearing an already normal pad is a no-op.
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":path,"pad_number":"1","padstack":null}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
}

#[tokio::test]
async fn unsupported_requests_are_refused_precisely_before_any_write() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = make_plain(&server, dir.path()).await;
    let original = std::fs::read(&path).unwrap();
    let with = |f: &dyn Fn(&mut serde_json::Value)| {
        let mut s = fixture_stack();
        f(&mut s);
        s
    };
    let cases: Vec<(&str, serde_json::Value, &str)> = vec![
        (
            "custom mode",
            with(&|s| s["mode"] = json!("custom")),
            "mode",
        ),
        (
            "normal mode",
            with(&|s| s["mode"] = json!("normal")),
            "mode",
        ),
        (
            "missing inner",
            with(&|s| {
                s.as_object_mut().unwrap().remove("inner");
            }),
            "inner",
        ),
        (
            "missing back",
            with(&|s| {
                s.as_object_mut().unwrap().remove("back");
            }),
            "back",
        ),
        (
            "trapezoid",
            with(&|s| s["inner"]["shape"] = json!("trapezoid")),
            "inner",
        ),
        (
            "zero size",
            with(&|s| s["inner"]["width"] = json!(0)),
            "inner",
        ),
        (
            "negative size",
            with(&|s| s["back"]["height"] = json!(-1.0)),
            "back",
        ),
        (
            "huge size",
            with(&|s| s["front"]["width"] = json!(1000)),
            "front",
        ),
        (
            "string size",
            with(&|s| s["front"]["width"] = json!("1.6")),
            "front",
        ),
        (
            "ratio on rect",
            with(&|s| s["inner"]["roundrect_rratio"] = json!(0.2)),
            "inner",
        ),
        (
            "ratio out of range",
            with(&|s| {
                s["back"] = json!({"shape":"roundrect","width":1,"height":1,"roundrect_rratio":0.9})
            }),
            "back",
        ),
        ("unknown key", with(&|s| s["offset"] = json!(1)), "padstack"),
        ("not an object", json!("front_inner_back"), "padstack"),
    ];
    for (name, stack, field) in cases {
        let r = call(
            &server,
            "edit_footprint_pad",
            json!({"footprint_path":path,"pad_number":"1","padstack":stack}),
        )
        .await;
        assert!(is_error(&r), "{name} must be refused: {r}");
        assert!(err_text(&r).contains(field), "{name}: {}", err_text(&r));
        assert_eq!(std::fs::read(&path).unwrap(), original, "{name}");
    }
    // Header edits that would desync a padstack pad, and a header that is not the front layer.
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":path,"pad_number":"1","width":2.0,"padstack":fixture_stack()}),
    )
    .await;
    assert!(is_error(&r), "{r}");
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[tokio::test]
async fn smd_and_layerless_pads_cannot_take_a_padstack() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Smd.kicad_mod");
    let r = call(
        &server,
        "create_footprint",
        json!({"output":path,"name":"Smd","pads":[
            {"number":"1","type":"smd","shape":"rect","x":0,"y":0,"width":1.6,"height":1.6}]}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let original = std::fs::read(&path).unwrap();
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":path,"pad_number":"1","padstack":fixture_stack()}),
    )
    .await;
    assert!(is_error(&r), "{r}");
    assert!(err_text(&r).contains("thru_hole"), "{}", err_text(&r));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    for pad in [
        json!({"number":"1","type":"smd","shape":"rect","x":0,"y":0,"width":1.6,"height":1.6,
               "padstack":fixture_stack()}),
        th_pad(
            "1",
            0.0,
            json!({"layers":["F.Cu","B.Cu"],"padstack":fixture_stack()}),
        ),
        th_pad("1", 0.0, json!({"width":2.0,"padstack":fixture_stack()})),
        th_pad(
            "1",
            0.0,
            json!({"shape":"circle","padstack":fixture_stack()}),
        ),
    ] {
        let r = call(
            &server,
            "create_footprint",
            json!({"output":path,"name":"X","pads":[pad]}),
        )
        .await;
        assert!(is_error(&r), "{r}");
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
}

#[tokio::test]
async fn native_front_inner_back_pad_reads_back_and_survives_unrelated_edits() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Native.kicad_mod");
    std::fs::write(&path, NATIVE_FRONT_INNER_BACK).unwrap();
    let got = info(&server, &path).await;
    let ps = &got["pads"][0]["padstack"];
    assert_eq!(ps["mode"], "front_inner_back");
    assert_eq!(ps["front"]["shape"], "rect");
    assert_eq!(ps["inner"]["shape"], "circle");
    assert_eq!(ps["inner"]["size"], json!({"x":1.2,"y":1.2}));
    assert_eq!(ps["back"]["shape"], "rect");

    // A drill edit must leave the stack bytes alone.
    let r = call(
        &server,
        "edit_footprint_pad",
        json!({"footprint_path":path,"pad_number":"1","drill":0.9}),
    )
    .await;
    assert!(!is_error(&r), "{r}");
    let after = std::fs::read_to_string(&path).unwrap();
    let stack = |s: &str| s[s.find("(padstack").unwrap()..].to_string();
    assert_eq!(stack(NATIVE_FRONT_INNER_BACK), stack(&after));
    assert!(after.contains("(drill 0.9)"));

    // Header geometry edits without the stack would desync it: refused.
    let before = std::fs::read(&path).unwrap();
    for args in [
        json!({"width":2.0}),
        json!({"height":2.0}),
        json!({"shape":"circle"}),
    ] {
        let mut a = json!({"footprint_path":path,"pad_number":"1"});
        a.as_object_mut()
            .unwrap()
            .extend(args.as_object().unwrap().clone());
        let r = call(&server, "edit_footprint_pad", a).await;
        assert!(is_error(&r), "{r}");
        assert!(err_text(&r).contains("padstack"), "{}", err_text(&r));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

#[tokio::test]
async fn native_custom_stack_reads_back_per_layer_and_is_not_editable_here() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Custom.kicad_mod");
    std::fs::write(&path, NATIVE_CUSTOM).unwrap();
    let got = info(&server, &path).await;
    let p1 = &got["pads"][0];
    assert_eq!(p1["padstack"]["mode"], "custom");
    let layers = p1["padstack"]["layers"].as_array().unwrap();
    let find = |name: &str| layers.iter().find(|l| l["layer"] == name).unwrap();
    assert_eq!(find("In1.Cu")["shape"], "circle");
    assert_eq!(find("In2.Cu")["shape"], "oval");
    assert_eq!(find("In2.Cu")["size"], json!({"x":1.4,"y":1.0}));
    assert_eq!(find("B.Cu")["shape"], "roundrect");
    assert_eq!(find("B.Cu")["corner_rounding_ratio"], 0.25);
    // Native writes these per-layer tokens for custom stacks; they are reported, not hidden.
    assert!(find("In3.Cu")["unmodelled"]
        .as_array()
        .unwrap()
        .contains(&json!("zone_connect")));
    let cl = p1["copper_layers"].as_array().unwrap();
    assert!(cl.iter().all(|c| c["layer"] != "*.Cu"));
    assert_eq!(cl[0]["layer"], "F.Cu");
    assert_eq!(got["pads"][1]["padstack"]["mode"], "normal");

    let original = std::fs::read(&path).unwrap();
    for padstack in [fixture_stack(), serde_json::Value::Null] {
        let r = call(
            &server,
            "edit_footprint_pad",
            json!({"footprint_path":path,"pad_number":"1","padstack":padstack}),
        )
        .await;
        assert!(is_error(&r), "{r}");
        assert!(err_text(&r).contains("custom"), "{}", err_text(&r));
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
}

#[tokio::test]
async fn a_stack_with_unmodelled_layer_tokens_is_refused_not_rewritten() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Extra.kicad_mod");
    // Derived from the native fixture in memory: the inner layer gains an offset.
    let source = NATIVE_FRONT_INNER_BACK.replacen(
        "(size 1.2 1.2)",
        "(size 1.2 1.2)\n\t\t\t\t(offset 0.1 0)",
        1,
    );
    assert!(source.contains("(offset 0.1 0)"));
    std::fs::write(&path, &source).unwrap();
    let got = info(&server, &path).await;
    assert!(got["pads"][0]["padstack"]["inner"]["unmodelled"]
        .as_array()
        .unwrap()
        .contains(&json!("offset")));
    for padstack in [fixture_stack(), serde_json::Value::Null] {
        let r = call(
            &server,
            "edit_footprint_pad",
            json!({"footprint_path":path,"pad_number":"1","padstack":padstack}),
        )
        .await;
        assert!(is_error(&r), "{r}");
        assert!(err_text(&r).contains("offset"), "{}", err_text(&r));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
    }
}

#[tokio::test]
async fn an_unreadable_or_unknown_stack_fails_readback() {
    let server = server().await;
    let dir = tempfile::tempdir().unwrap();
    for (name, replacement) in [
        (
            "unknown mode",
            NATIVE_FRONT_INNER_BACK.replace("front_inner_back", "future_mode"),
        ),
        (
            "missing back layer",
            NATIVE_FRONT_INNER_BACK.replacen("(layer \"B.Cu\"", "(layer \"F.Cu\"", 1),
        ),
        (
            "bad shape",
            NATIVE_FRONT_INNER_BACK.replacen("(shape circle)", "(shape hexagon)", 1),
        ),
    ] {
        let path = dir
            .path()
            .join(format!("{}.kicad_mod", name.replace(' ', "_")));
        std::fs::write(&path, replacement).unwrap();
        let r = call(
            &server,
            "get_footprint_info",
            json!({"footprint_path":path,"include_pads":true}),
        )
        .await;
        assert!(is_error(&r), "{name} must fail readback: {r}");
        assert!(
            err_text(&r).contains("padstack"),
            "{name}: {}",
            err_text(&r)
        );
    }
}
