"""Exercise the built MCP server and independently round-trip with KiCad.

Requires Python 3 and sexpdata. No production inputs are opened or modified.
Field names reproduce PCB573-74's library-import workflow; all values, names,
geometry and evidence strings here are synthetic. Private source files are not
read by this script or included in its output.
"""
import argparse
import json
import os
from pathlib import Path
import queue
import subprocess
import threading

import sexpdata as sx


def tag(node):
    return str(node[0]) if isinstance(node, list) and node else None


def children(node, name):
    return [child for child in node if tag(child) == name]


def symbols(path):
    return {node[1]: node for node in children(sx.loads(path.read_text(encoding="utf-8")), "symbol")}


def fields(symbol):
    return {node[1]: node for node in children(symbol, "property")}


def hidden(field):
    def hides(node):
        return any(
            str(item) == "hide" or (tag(item) == "hide" and (len(item) == 1 or str(item[1]) != "no"))
            for item in node
        )
    return hides(field) or any(hides(effect) for effect in children(field, "effects"))


class Server:
    def __init__(self, executable, output, cli):
        self.counter = 0
        config = output / "server.json"
        config.write_text(json.dumps({"transport": "stdio", "eager_toolsets": True,
            "kicad_cli": str(cli), "log_level": "warn"}), encoding="utf-8")
        env = dict(os.environ, KONNECT_STATE_DIR=str(output / "state"))
        self.log = (output / "server.log").open("w", encoding="utf-8")
        self.process = subprocess.Popen([str(executable), "--config", str(config)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log,
            text=True, encoding="utf-8", env=env)
        self.lines = queue.Queue()
        def read():
            for line in self.process.stdout:
                self.lines.put(line)
            self.lines.put(None)
        threading.Thread(target=read, daemon=True).start()
        self.request("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
            "clientInfo": {"name": "ailab40-verifier", "version": "1"}})

    def request(self, method, params):
        self.counter += 1
        identifier = self.counter
        self.process.stdin.write(json.dumps({"jsonrpc": "2.0", "id": identifier,
            "method": method, "params": params}) + "\n")
        self.process.stdin.flush()
        while True:
            line = self.lines.get(timeout=60)
            assert line is not None, "MCP server exited; inspect server.log"
            result = json.loads(line)
            if result.get("id") == identifier:
                assert "error" not in result, result
                return result["result"]

    def call(self, name, args, error=False):
        result = self.request("tools/call", {"name": name, "arguments": args})
        assert bool(result.get("isError")) == error, result
        return json.loads(result["content"][0]["text"])

    def close(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            self.process.wait(timeout=10)
        self.log.close()


def run(cli, *args):
    result = subprocess.run([str(cli), *map(str, args)], capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, (args, result.stdout, result.stderr)
    return result.stdout.strip()


def verify(executable, cli, output):
    output.mkdir(parents=True, exist_ok=False)
    version = run(cli, "version")
    assert version == "10.0.6", f"Acceptance requires KiCad 10.0.6, got {version}"
    server = Server(executable, output, cli)
    library = output / "Sanitized.kicad_sym"
    expected = {"GSIE_PN": "SANITIZED-001", "MPN": 'EXAMPLE "quoted" \\ path Ω\nline 2',
        "Manufacturer": "Example manufacturer", "GSI_Decal": "SanitizedPackage",
        "GSI_Source": "synthetic fixture: no private source data", "GSI_Part_Status": "Example",
        "Rating": "0.25 W", "Voltage": "50 V", "GSI_Notes": "", "Description": "Sanitized dual amplifier"}
    try:
        tools = server.request("tools/list", {})["tools"]
        for name in ["create_symbol", "edit_symbol_properties"]:
            tool = next(t for t in tools if t["name"] == name)
            assert tool["inputSchema"]["properties"]["properties"]["items"]["additionalProperties"] is False
        pins = [{"number": str(i), "name": name, "type": kind}
            for i, name, kind in [(1, "+", "input"), (2, "-", "input"), (3, "OUT", "output")]]
        second_pins = [dict(pin, number=str(int(pin["number"]) + 3)) for pin in pins]
        props = [{"name": name, "value": value} for name, value in expected.items()]
        next(p for p in props if p["name"] == "MPN").update(visible=True, x=2.54, y=-5.08, angle=90)
        create = {"library_path": str(library), "name": "SanitizedDual", "reference_prefix": "U",
            "glyph": "opamp", "units": [{"pins": pins}, {"pins": second_pins}],
            "footprint": "Example:Package", "properties": props}
        created = server.call("create_symbol", create)
        assert created["properties"]["Footprint"] == "Example:Package"
        server.call("create_symbol", {"library_path": str(library), "name": "Untouched",
            "reference_prefix": "R", "pins": [{"number": "1", "name": "1", "type": "passive"}]})
        registered = server.call("register_symbol_library", {"library_path": str(library),
            "nickname": "Sanitized", "scope": "project", "project": str(output)})
        assert registered.get("success", True)
        # The native loader saves this library, providing a KiCad-authored input
        # for the editor and an independent check of the create path.
        run(cli, "sym", "upgrade", "--force", library)
        native = symbols(library)
        original = native["SanitizedDual"]
        original_fields = fields(original)
        for name, value in expected.items():
            assert original_fields[name][2] == value, (name, original_fields[name])
        assert original_fields["Footprint"][2] == "Example:Package"
        assert not hidden(original_fields["MPN"])
        assert hidden(original_fields["Manufacturer"])
        patch = {"library_path": str(library), "symbol_name": "SanitizedDual", "footprint": "Example:Revised",
            "properties": [{"name": "MPN", "value": "EXAMPLE-REV-B", "visible": False, "x": 0},
                {"name": "Rating", "value": "0"}, {"name": "NewField", "value": "", "visible": True}]}
        edited = server.call("edit_symbol_properties", patch)
        assert edited["changed"] is True
        after = symbols(library)
        untouched = lambda symbol: [node for node in symbol if tag(node) != "property" or node[1] not in {"MPN", "Footprint", "Rating", "NewField"}]
        assert untouched(after["SanitizedDual"]) == untouched(original)
        assert after["Untouched"] == native["Untouched"]
        saved = library.read_bytes()
        assert server.call("edit_symbol_properties", patch)["changed"] is False
        assert library.read_bytes() == saved
        server.call("create_symbol", create, error=True)
        invalid = dict(patch, properties=[{"name": "MPN", "value": "bad", "visible": None}])
        server.call("edit_symbol_properties", invalid, error=True)
        assert library.read_bytes() == saved
        read = server.call("get_symbol_info", {"lib_id": "Sanitized:SanitizedDual", "project_dir": str(output)})
        assert read["properties"]["MPN"] == "EXAMPLE-REV-B"
        assert read["property_details"]["MPN"] == {"value": "EXAMPLE-REV-B", "visible": False,
            "x": 0.0, "y": -5.08, "angle": 90.0}
        run(cli, "sym", "upgrade", "--force", library)
        roundtrip = symbols(library)
        final = fields(roundtrip["SanitizedDual"])
        assert final["MPN"][2] == "EXAMPLE-REV-B" and hidden(final["MPN"])
        assert children(final["MPN"], "at")[0][1:] == [0, -5.08, 90]
        assert final["Footprint"][2] == "Example:Revised" and final["Rating"][2] == "0"
        assert final["NewField"][2] == "" and not hidden(final["NewField"])
        assert children(roundtrip["SanitizedDual"], "symbol") == children(original, "symbol")
        assert roundtrip["Untouched"] == native["Untouched"]
        run(cli, "sym", "export", "svg", "--include-hidden-fields", "--output", output / "svg", library)
        svgs = list((output / "svg").glob("*.svg"))
        assert len(svgs) >= 3, [p.name for p in svgs]
        evidence = {"result": "PASS", "kicad_version": version, "symbols": 2,
            "custom_fields_checked": len(expected), "dual_symbol_pins": 6, "svg_exports": len(svgs),
            "native_roundtrips": 2, "repeated_edit_byte_identical": True,
            "refused_calls_byte_identical": True, "unrelated_units_and_symbol_preserved": True}
        (output / "evidence.json").write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(evidence))
    finally:
        server.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, required=True)
    parser.add_argument("--kicad-cli", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="New directory for sanitized artifacts")
    args = parser.parse_args()
    verify(args.server.resolve(), args.kicad_cli.resolve(), args.output.resolve())
