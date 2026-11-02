"""Sanitized stdio benchmark. Python stdlib only; no installed server or live IPC."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import queue
import subprocess
import tempfile
import threading
import time


def encoded(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")


def measure(value):
    return len(encoded(value))


def body(response):
    result = response.get("result", {})
    texts = [c.get("text", "") for c in result.get("content", []) if c.get("type") == "text"]
    try:
        return json.loads(texts[0]) if texts else result
    except json.JSONDecodeError:
        return texts


def failed(response):
    return "error" in response or response.get("result", {}).get("isError", False)


class Client:
    def __init__(self, server, scratch, eager):
        config = scratch / "benchmark.json"
        config.write_text(json.dumps({"transport":"stdio", "ipc_address":"inproc://ailab45-no-native-peer", "eager_toolsets":eager, "auto_load_toolsets":False, "log_level":"error", "kicad_cli":"ailab45-disabled-cli", "kicad_binary":"ailab45-disabled-editor"}), encoding="utf-8")
        env = dict(os.environ, KONNECT_STATE_DIR=str(scratch / "state"), APPDATA=str(scratch / "config"), HOME=str(scratch), XDG_CONFIG_HOME=str(scratch / "config"), KICAD_API_SOCKET="inproc://ailab45-no-native-peer")
        self.process = subprocess.Popen([str(server), "--config", str(config)], cwd=scratch, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        self.incoming = queue.Queue()
        def reader():
            for line in self.process.stdout:
                self.incoming.put(line)
            self.incoming.put(None)
        threading.Thread(target=reader, daemon=True).start()
        self.rows = []
        self.next_id = 0

    def request(self, method, params, phase):
        self.next_id += 1
        message = {"jsonrpc":"2.0", "id":self.next_id, "method":method, "params":params}
        wire = encoded(message)
        self.process.stdin.write(wire + b"\n")
        self.process.stdin.flush()
        started = time.monotonic()
        notification_bytes = 0
        notifications = 0
        while True:
            line = self.incoming.get(timeout=max(0.01, 30 - (time.monotonic() - started)))
            if line is None:
                raise RuntimeError("server closed stdout")
            response = json.loads(line)
            if response.get("id") == self.next_id:
                break
            notifications += 1
            notification_bytes += len(line.rstrip(b"\r\n"))
        tools = response.get("result", {}).get("tools", [])
        self.rows.append({"phase":phase, "method":method, "tool":params.get("name") if method == "tools/call" else None, "tool_calls":int(method == "tools/call"), "request_bytes":len(wire), "response_bytes":len(line.rstrip(b"\r\n")), "input_schema_bytes":sum(measure(t["inputSchema"]) for t in tools), "tool_definition_bytes":sum(measure(t) for t in tools), "listed_tools":len(tools), "notification_bytes":notification_bytes, "notifications":notifications, "failed":failed(response)})
        return response

    def call(self, name, args, phase, expect_error=False):
        response = self.request("tools/call", {"name":name, "arguments":args}, phase)
        if failed(response) != expect_error:
            raise AssertionError(f"unexpected outcome for {name}: {body(response)}")
        return body(response)

    def close(self):
        self.process.kill()
        self.process.wait(timeout=10)
        self.process.stdin.close()
        self.process.stdout.close()


def verify_readback(path, expected):
    """Independent disk read; never writes or parses/reformats a KiCad artifact."""
    content = path.read_text(encoding="utf-8")
    for fragment in expected:
        if fragment not in content:
            raise AssertionError(f"independent readback missing {fragment!r}")
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run_policy(server, policy, scenarios):
    with tempfile.TemporaryDirectory(prefix="konnect-ailab45-") as directory:
        scratch = Path(directory)
        client = Client(server, scratch, policy == "eager")
        checks = []
        try:
            init = client.request("initialize", {"protocolVersion":"2025-06-18", "capabilities":{}, "clientInfo":{"name":"ailab45-byte-benchmark", "version":"1"}}, "handshake")
            if "error" in init:
                raise AssertionError(init)
            # Explicitly account for this one-way client notification outside request totals.
            notification = encoded({"jsonrpc":"2.0", "method":"notifications/initialized"})
            client.process.stdin.write(notification + b"\n")
            client.process.stdin.flush()
            first = client.request("tools/list", {}, "initial_discovery")
            if not first.get("result", {}).get("tools"):
                raise AssertionError("empty initial catalogue")
            if policy == "on_demand":
                client.call("list_toolboxes", {}, "selective_discovery")
                client.call("load_toolset", {"name":["library", "pcb_board"]}, "selective_discovery")
                client.request("tools/list", {}, "selective_discovery")
            client.call("get_active_toolsets", {}, "ordinary_reads")
            client.call("create_project", {"path":str(scratch), "name":"Benchmark"}, "fixture_setup")
            project_info = client.call("get_project_info", {"path":str(scratch / "Benchmark.kicad_pro")}, "ordinary_reads")
            if not project_info.get("pcb_exists"):
                raise AssertionError("created board not observed by project read")
            board = scratch / "Benchmark.kicad_pcb"
            board_before = board.read_bytes()
            layers = client.call("get_layer_list", {"board":str(board), "board_source":"saved"}, "saved_pcb_read")
            if "saved_board" not in json.dumps(layers) or board_before != board.read_bytes():
                raise AssertionError("saved-board provenance or unchanged-state check failed")
            checks.append("saved_pcb_layers_provenance_and_unchanged_read")
            library = scratch / "Benchmark.kicad_sym"
            symbol = dict(scenarios["symbol"], library_path=str(library))
            client.call("create_symbol", symbol, "symbol_workflow")
            verify_readback(library, ['(symbol "BenchmarkSensor"', '"IN"', '"OUT"'])
            edit = dict(scenarios["symbol_edit"], library_path=str(library))
            client.call("edit_symbol_properties", edit, "symbol_workflow")
            verify_readback(library, ['"Manufacturer" "Sanitized example"', '"IN"', '"OUT"'])
            client.call("list_symbols_in_library", {"library_path":str(library)}, "symbol_workflow")
            client.call("register_symbol_library", {"library_path":str(library), "nickname":"Benchmark", "scope":"project", "project":str(scratch)}, "symbol_workflow")
            info = client.call("get_symbol_info", {"lib_id":"Benchmark:BenchmarkSensor", "project_dir":str(scratch)}, "symbol_workflow")
            if "Sanitized example" not in json.dumps(info):
                raise AssertionError("symbol served readback missing custom property")
            before = library.read_bytes()
            client.call("edit_symbol_properties", dict(edit, properties=[{"name":"Manufacturer", "value":None}]), "symbol_refusal", expect_error=True)
            if before != library.read_bytes():
                raise AssertionError("refused edit changed library")
            checks.extend(["symbol_create_edit_disk_and_served_readback", "invalid_symbol_edit_refused_unchanged"])
            footprint = scratch / "BenchmarkPad.kicad_mod"
            client.call("create_footprint", dict(scenarios["footprint"], output=str(footprint)), "model_workflow")
            client.call("set_footprint_models", dict(scenarios["model_edit"], footprint_path=str(footprint)), "model_workflow")
            verify_readback(footprint, ['(model "models/sanitized.step"', '(pad "1"'])
            info = client.call("get_footprint_info", {"footprint_path":str(footprint), "include_pads":True}, "model_workflow")
            if not info.get("has_3d_model"):
                raise AssertionError("served footprint readback lacks model")
            before = footprint.read_bytes()
            client.call("set_footprint_models", {"footprint_path":str(footprint), "mode":"wrong"}, "model_refusal", expect_error=True)
            if before != footprint.read_bytes():
                raise AssertionError("refused model edit changed footprint")
            checks.extend(["model_reference_disk_and_served_readback", "invalid_model_edit_refused_unchanged"])
            return {"policy":policy, "server_info":init["result"]["serverInfo"], "scratch_path_utf8_bytes":len(str(scratch).encode("utf-8")), "client_notification_bytes":len(notification), "checks":checks, "rows":client.rows, "totals":{key:sum(row[key] for row in client.rows) for key in ("tool_calls", "request_bytes", "response_bytes", "input_schema_bytes", "tool_definition_bytes", "notification_bytes", "notifications")}}
        finally:
            client.close()


def projection(scenario):
    calls = []
    for index in range(1, scenario["item_count"] + 1):
        for template in scenario["per_item_calls"]:
            arguments = {k:v.format(index=index) if isinstance(v, str) else v for k,v in template["arguments"].items()}
            calls.append({"jsonrpc":"2.0", "id":len(calls)+1, "method":"tools/call", "params":{"name":template["name"], "arguments":arguments}})
    return {"kind":scenario["kind"], "item_count":scenario["item_count"], "synthetic_calls":len(calls), "synthetic_request_bytes":sum(measure(c) for c in calls), "observed_response_bytes":None, "warning":scenario["warning"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, required=True, help="explicit worktree-built server; never the installed binary")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    server = args.server.resolve(strict=True)
    root = Path(__file__).resolve().parents[3]
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    scenarios_path = Path(__file__).with_name("scenarios.json")
    scenarios = json.loads(scenarios_path.read_text(encoding="utf-8-sig"))
    result = {"format_version":1, "baseline_commit":head, "binary_sha256":hashlib.sha256(server.read_bytes()).hexdigest(), "scenario_sha256":hashlib.sha256(encoded(scenarios)).hexdigest(), "scenario_hash_format":"canonical compact sorted-key UTF-8 JSON", "environment":{"os":platform.system(), "release":platform.release(), "machine":platform.machine(), "python":platform.python_version()}, "accounting":{"unit":"UTF-8 bytes; JSON-RPC line delimiters excluded", "requests":"exact compact sorted-key JSON sent", "responses":"exact server stdout payload", "schemas":"canonical compact inputSchema JSON sum; subset of response bytes, do not add", "definitions":"canonical complete tool definition sum; includes schemas, do not add", "tokens":"not estimated; no tokenizer or billed-token claim", "client":"one initial list; on-demand loads library and pcb_board together then refreshes once; eager lists full catalogue once; identical business calls", "ipc":"unused inproc endpoint, no native editor or API connection", "scope":"model reference metadata only; STEP existence/rendering and PCB placement unverified"}, "policies":[run_policy(server, p, scenarios) for p in ("on_demand", "eager")], "pcb_model_projection":projection(scenarios["pcb_projection"])}
    args.output.write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    for policy in result["policies"]:
        print(policy["policy"], json.dumps(policy["totals"]))


if __name__ == "__main__":
    main()
