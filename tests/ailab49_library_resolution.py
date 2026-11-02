"""Run against a task-built server; fixtures use served builders, never text edits."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading


def run(binary, native_python, root_source):
    with tempfile.TemporaryDirectory(prefix="ailab49 spaces ") as scratch:
        root = Path(scratch)
        documents = root / "Documents With Spaces"
        kicad_dir = "KiCad" if sys.platform in ("win32", "darwin") else "kicad"
        third_party = documents / kicad_dir / "10.0/3rdparty"
        pretty = third_party / "footprints/Vendor Parts.pretty"
        pretty.mkdir(parents=True)
        env = os.environ.copy()
        env.update(APPDATA=str(root / "config"), HOME=str(root),
                   KICAD_CONFIG_HOME=str(root / "native-config"),
                   KICAD_DOCUMENTS_HOME=str(documents), KONNECT_STATE_DIR=str(root / "state"))
        env.pop("KICAD10_3RD_PARTY", None)
        if root_source.startswith("empty-"):
            env["KICAD10_3RD_PARTY"] = ""
        override = root / "missing override" if root_source.startswith("missing-") else third_party
        if root_source.endswith("exported"):
            env["KICAD10_3RD_PARTY"] = str(override)
        elif root_source.endswith("configured"):
            config_base = root / "config" if sys.platform == "win32" else (
                root / "Library/Preferences" if sys.platform == "darwin" else root / ".config")
            config_dir = config_base / "kicad/10.0"
            config_dir.mkdir(parents=True)
            (config_dir / "kicad_common.json").write_text(json.dumps({"environment": {
                "vars": {"KICAD10_3RD_PARTY": str(override)}}}))
        config = root / "konnect.toml"
        config.write_text('ipc_address = "inproc://ailab49-library-only"\n')
        process = subprocess.Popen([str(Path(binary).resolve()), "--config", str(config)],
                                   cwd=root, env=env, stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        timer = threading.Timer(60, process.kill)
        timer.start()
        request_id = 0

        def request(method, params):
            nonlocal request_id
            request_id += 1
            process.stdin.write(json.dumps(dict(jsonrpc="2.0", id=request_id,
                                                 method=method, params=params)) + "\n")
            process.stdin.flush()
            while True:
                line = process.stdout.readline()
                assert line, "server exited before response"
                result = json.loads(line)
                if result.get("id") == request_id:
                    assert "error" not in result, result
                    return result["result"]

        def call(name, arguments, success=True):
            result = request("tools/call", dict(name=name, arguments=arguments))
            assert bool(result.get("isError", False)) != success, result
            text = result["content"][0]["text"]
            return json.loads(text) if success else text

        try:
            request("initialize", dict(protocolVersion="2025-06-18", capabilities={},
                                       clientInfo=dict(name="ailab49", version="1")))
            process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
            process.stdin.flush()
            call("load_toolset", dict(name="library"))
            call("create_footprint", dict(output=str(pretty / "Probe.kicad_mod"), name="Probe",
                 pads=[dict(number="1", type="smd", shape="rect", x=0, y=0, width=1, height=1)]))
            call("register_footprint_library", dict(
                library_path="${KICAD10_3RD_PARTY}/footprints/Vendor Parts.pretty",
                nickname="AILAB49", scope="global"))
            if root_source.startswith("missing-"):
                refused = call("get_footprint_info", dict(footprint_path="AILAB49:Probe"), False)
                assert "unresolvable URI" in refused and "KICAD10_3RD_PARTY" in refused, refused
                print(f"PASS served {root_source} refuses existing default same-name library")
            else:
                found = call("get_footprint_info", dict(footprint_path="AILAB49:Probe"))
                assert found["pad_count"] == 1 and Path(found["path"]) == pretty / "Probe.kicad_mod", found
                print(f"PASS served {root_source} third-party lookup with spaces; pad_count=1")
            call("register_footprint_library", dict(library_path="${AILAB49_UNKNOWN}/Missing.pretty",
                                                    nickname="AILAB49Missing", scope="global"))
            refused = call("get_footprint_info", dict(footprint_path="AILAB49Missing:Probe"), False)
            assert "unresolvable URI" in str(refused) and "AILAB49_UNKNOWN" in str(refused), refused
            print("PASS served unknown variable refusal names URI")
            call("register_footprint_library", dict(
                library_path="${KICAD10_3RD_PARTY}/footprints/Missing.pretty",
                nickname="AILAB49Absent", scope="global"))
            absent = call("get_footprint_info", dict(footprint_path="AILAB49Absent:Probe"), False)
            assert "unresolvable URI" in str(absent) and "Missing.pretty" in str(absent), absent
            print("PASS served missing third-party directory refusal names URI")
            if native_python:
                code = ("import pcbnew,sys; fp=pcbnew.FootprintLoad(sys.argv[1], 'Probe'); "
                        "assert fp is not None and len(list(fp.Pads())) == 1; "
                        "print('PASS native '+pcbnew.Version()+' same footprint; pad_count=1')")
                result = subprocess.run([native_python, "-c", code, str(pretty)], env=env,
                                        capture_output=True, text=True, timeout=30)
                assert result.returncode == 0, result.stderr
                print(result.stdout.strip())
        finally:
            timer.cancel()
            process.stdin.close()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("binary")
    parser.add_argument("--native-python")
    parser.add_argument("--root-source", choices=["default", "configured", "exported",
                        "missing-configured", "missing-exported", "empty-default",
                        "empty-configured"], default="default")
    args = parser.parse_args()
    run(args.binary, args.native_python, args.root_source)
