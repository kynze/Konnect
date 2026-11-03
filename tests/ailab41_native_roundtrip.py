"""AILAB-41 native round trip.

Footprints are authored by a task-built Konnect server (never by text edits),
then loaded, saved and reloaded by native KiCad 10.0.6 pcbnew in a scratch
configuration. No IPC endpoint, editor or installed configuration is used.

usage: python ailab41_native_roundtrip.py <konnect.exe> <kicad python.exe>
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading

NATIVE = r'''
import json, sys, pcbnew
lib, name, out = sys.argv[1], sys.argv[2], sys.argv[3]
def unwrap(v):
    # KiCad 10 exposes std::optional as value-or-None depending on binding.
    return None if v is None else v
def read(fp):
    rows = {}
    for pad in fp.Pads():
        rows[pad.GetNumber()] = dict(
            mask=unwrap(pad.GetLocalSolderMaskMargin()),
            paste=pad.GetLocalSolderPasteMargin(),
            ratio=unwrap(pad.GetLocalSolderPasteMarginRatio()),
            size=[pad.GetSizeX(), pad.GetSizeY()], pos=[pad.GetX(), pad.GetY()])
    return rows
fp = pcbnew.FootprintLoad(lib, name)
assert fp is not None, "native load failed"
before = read(fp)
Path = __import__("pathlib").Path
Path(out).mkdir(parents=True, exist_ok=True)
io = pcbnew.PCB_IO_KICAD_SEXPR()
io.FootprintSave(out, fp)
fp2 = pcbnew.FootprintLoad(out, name)
assert fp2 is not None, "native reload failed"
after = read(fp2)
print(json.dumps(dict(version=pcbnew.Version(), before=before, after=after)))
'''


def run(binary, native_python):
    with tempfile.TemporaryDirectory(prefix="ailab41 native ") as scratch:
        root = Path(scratch)
        pretty = root / "Lib.pretty"
        env = os.environ.copy()
        env.update(APPDATA=str(root / "config"), HOME=str(root),
                   KICAD_CONFIG_HOME=str(root / "native-config"),
                   KICAD_DOCUMENTS_HOME=str(root / "docs"),
                   KONNECT_STATE_DIR=str(root / "state"))
        config = root / "konnect.toml"
        config.write_text('ipc_address = "inproc://ailab41-library-only"\n')
        process = subprocess.Popen([str(Path(binary).resolve()), "--config", str(config)],
                                   cwd=root, env=env, stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        timer = threading.Timer(120, process.kill)
        timer.start()
        rid = 0

        def request(method, params):
            nonlocal rid
            rid += 1
            process.stdin.write(json.dumps(dict(jsonrpc="2.0", id=rid, method=method,
                                                 params=params)) + "\n")
            process.stdin.flush()
            while True:
                line = process.stdout.readline()
                assert line, "server exited before response"
                msg = json.loads(line)
                if msg.get("id") == rid:
                    assert "error" not in msg, msg
                    return msg["result"]

        def call(name, arguments, ok=True):
            result = request("tools/call", dict(name=name, arguments=arguments))
            assert bool(result.get("isError", False)) != ok, result
            return json.loads(result["content"][0]["text"])

        def native(footprint_name, out):
            r = subprocess.run([native_python, "-I", "-c", NATIVE, str(pretty), footprint_name,
                                str(out)], env=env, capture_output=True, text=True, timeout=60)
            assert r.returncode == 0, r.stderr + r.stdout
            return json.loads(r.stdout.strip().splitlines()[-1])

        try:
            request("initialize", dict(protocolVersion="2025-06-18", capabilities={},
                                       clientInfo=dict(name="ailab41", version="1")))
            process.stdin.write(json.dumps(dict(jsonrpc="2.0",
                                                method="notifications/initialized")) + "\n")
            process.stdin.flush()
            call("load_toolset", dict(name="library"))
            fp_path = pretty / "Probe.kicad_mod"
            pads = [
                dict(number="1", type="smd", shape="rect", x=-1, y=0, width=1, height=1.2,
                     solder_mask_margin=0.075, solder_paste_margin=-0.05,
                     solder_paste_margin_ratio=-0.1),
                dict(number="2", type="smd", shape="roundrect", x=1, y=0, width=1, height=1.2,
                     roundrect_rratio=0.25, solder_mask_margin=0.125,
                     solder_paste_margin=0.025, solder_paste_margin_ratio=0.05),
                dict(number="3", type="smd", shape="rect", x=3, y=0, width=1, height=1.2),
            ]
            call("create_footprint", dict(output=str(fp_path), name="Probe", pads=pads))

            # Phase 1: authored values survive native load/save/reload exactly.
            n = native("Probe", root / "Out1.pretty")
            for phase in ("before", "after"):
                p = n[phase]
                assert p["1"]["mask"] == 75000, p
                assert p["2"]["mask"] == 125000, p
                assert p["1"]["paste"] == -50000 and p["2"]["paste"] == 25000, p
                assert abs(p["1"]["ratio"] + 0.1) < 1e-9, p
                assert abs(p["2"]["ratio"] - 0.05) < 1e-9, p
                assert p["3"]["mask"] is None and p["3"]["paste"] is None, p
                assert p["3"]["ratio"] is None, p
            print(f"PASS native {n['version']} loaded, saved, reloaded: +0.075/+0.125 mm "
                  "mask, paste margin +0.025/-0.05, ratio +0.05/-0.1, pad 3 inherits")

            # Phase 2: edits (change, clear, add) are read identically by KiCad.
            call("edit_footprint_pad", dict(footprint_path=str(fp_path), pad_number="1",
                                            solder_mask_margin=0.05, solder_paste_margin=None))
            call("edit_footprint_pad", dict(footprint_path=str(fp_path), pad_number="2",
                                            solder_paste_margin_ratio=None))
            call("edit_footprint_pad", dict(footprint_path=str(fp_path), pad_number="3",
                                            solder_mask_margin=0.075))
            n = native("Probe", root / "Out2.pretty")
            for phase in ("before", "after"):
                p = n[phase]
                assert p["1"]["mask"] == 50000, p        # changed in place
                assert p["1"]["paste"] is None, p        # cleared: inherits
                assert abs(p["1"]["ratio"] + 0.1) < 1e-9, p  # omitted: preserved
                assert p["2"]["ratio"] is None, p        # cleared: inherits
                assert p["2"]["mask"] == 125000, p
                assert p["3"]["mask"] == 75000, p
                assert p["1"]["size"] == [1000000, 1200000], p  # unrelated geometry kept
            print("PASS native edit semantics: change in place, clear inherits, "
                  "omitted preserved, geometry unchanged")

            # Phase 3: Konnect reads KiCad's own saved bytes the same way.
            saved = root / "Out2.pretty" / "Probe.kicad_mod"
            info = call("get_footprint_info", dict(footprint_path=str(saved),
                                                   include_pads=True))
            mp = {p["number"]: p["mask_paste"] for p in info["pads"]}
            assert mp["1"]["solder_mask_margin"]["local"] == 0.05, mp
            assert mp["3"]["solder_mask_margin"]["local"] == 0.075, mp
            assert mp["2"]["solder_paste_margin_ratio"]["local"] is None, mp
            assert mp["2"]["solder_paste_margin_ratio"]["effective"] is None, mp
            print("PASS served readback of KiCad-saved file matches native values")

            # Phase 4: why zero is refused. KiCad drops a pad-local 0 on load/save.
            # (Scratch fixture written by this test, not a design or library file.)
            zero_lib = root / "Zero.pretty"
            zero_lib.mkdir()
            (zero_lib / "Z.kicad_mod").write_text(
                '(footprint "Z" (version 20240108) (layer "F.Cu") '
                '(pad "1" smd rect (at 0 0) (size 1 1) (layers "F.Cu" "F.Paste" "F.Mask") '
                '(solder_mask_margin 0) (solder_paste_margin 0) (solder_paste_margin_ratio 0)))')
            r = subprocess.run([native_python, "-I", "-c", NATIVE, str(zero_lib), "Z",
                                str(root / "ZeroOut.pretty")], env=env, capture_output=True,
                               text=True, timeout=60)
            assert r.returncode == 0, r.stderr + r.stdout
            z = json.loads(r.stdout.strip().splitlines()[-1])["after"]["1"]
            assert z["mask"] is None and z["paste"] is None and z["ratio"] is None, z
            saved_text = (root / "ZeroOut.pretty" / "Z.kicad_mod").read_text()
            assert "solder_mask_margin" not in saved_text, saved_text
            refused = call("edit_footprint_pad", dict(footprint_path=str(fp_path),
                           pad_number="3", solder_mask_margin=0), ok=False)
            assert "KiCad 10.0.6" in json.dumps(refused), refused
            print("PASS native drops pad-local 0 on load/save; served edit refuses 0")

            # Phase 5: a footprint-level 0 is dropped natively as well (scratch fixture).
            fp_zero = root / "FpZero.pretty"
            fp_zero.mkdir()
            (fp_zero / "F.kicad_mod").write_text(
                '(footprint "F" (version 20240108) (layer "F.Cu") (solder_mask_margin 0) '
                '(solder_paste_margin 0) (solder_paste_margin_ratio 0) '
                '(pad "1" smd rect (at 0 0) (size 1 1) (layers "F.Cu" "F.Paste" "F.Mask")))')
            code = ("import pcbnew,sys; fp=pcbnew.FootprintLoad(sys.argv[1],'F'); "
                    "print(fp.GetLocalSolderMaskMargin(), fp.GetLocalSolderPasteMargin(), "
                    "fp.GetLocalSolderPasteMarginRatio())")
            r = subprocess.run([native_python, "-I", "-c", code, str(fp_zero)], env=env,
                               capture_output=True, text=True, timeout=60)
            assert r.returncode == 0, r.stderr
            assert r.stdout.split() == ["None", "None", "None"], r.stdout
            info = call("get_footprint_info", dict(footprint_path=str(fp_zero / "F.kicad_mod"),
                                                   include_pads=True))
            m = info["pads"][0]["mask_paste"]["solder_mask_margin"]
            assert m["source"] == "unknown" and m["effective"] is None, m
            assert m["footprint_token_zero"] is True, m
            print("PASS native drops footprint-level 0 too; served readback reports it as inherited")
        finally:
            timer.cancel()
            process.stdin.close()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


if __name__ == "__main__":
    run(sys.argv[1], sys.argv[2])
