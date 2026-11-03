"""AILAB-42 native round trip (per-layer copper padstack).

Footprints are authored by a task-built Konnect server (never by text edits),
then loaded, saved and reloaded by native KiCad 10.0.6 pcbnew in a scratch
configuration. No IPC endpoint, editor or installed configuration is used.
The only non-Konnect fixture is a custom-mode pad built by pcbnew itself.

usage: python ailab42_native_roundtrip.py <konnect.exe> <kicad python.exe>
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading

NATIVE = r'''
import json, sys, pcbnew, pathlib
lib, name, out = sys.argv[1], sys.argv[2], sys.argv[3]
SHAPES = {pcbnew.PAD_SHAPE_CIRCLE: "circle", pcbnew.PAD_SHAPE_RECTANGLE: "rect",
          pcbnew.PAD_SHAPE_OVAL: "oval", pcbnew.PAD_SHAPE_ROUNDRECT: "roundrect"}
MODES = {pcbnew.PADSTACK.MODE_NORMAL: "normal",
         pcbnew.PADSTACK.MODE_FRONT_INNER_BACK: "front_inner_back",
         pcbnew.PADSTACK.MODE_CUSTOM: "custom"}
def read(fp):
    rows = {}
    for pad in fp.Pads():
        layers = {}
        for lname, layer in (("F.Cu", pcbnew.F_Cu), ("In1.Cu", pcbnew.In1_Cu), ("B.Cu", pcbnew.B_Cu)):
            size = pad.GetSize(layer)
            layers[lname] = dict(shape=SHAPES.get(pad.GetShape(layer), str(pad.GetShape(layer))),
                                 size=[size.x, size.y],
                                 ratio=round(pad.GetRoundRectRadiusRatio(layer), 6))
        rows[pad.GetNumber()] = dict(mode=MODES[pad.Padstack().Mode()], layers=layers,
                                     drill=pad.GetDrillSizeX(), mask=pad.GetLocalSolderMaskMargin())
    return rows
fp = pcbnew.FootprintLoad(lib, name)
assert fp is not None, "native load failed"
before = read(fp)
pathlib.Path(out).mkdir(parents=True, exist_ok=True)
pcbnew.PCB_IO_KICAD_SEXPR().FootprintSave(out, fp)
fp2 = pcbnew.FootprintLoad(out, name)
assert fp2 is not None, "native reload failed"
print(json.dumps(dict(version=pcbnew.Version(), before=before, after=read(fp2))))
'''

CUSTOM_GEN = '''
import pcbnew
fp = pcbnew.FOOTPRINT(None); fp.SetFPID(pcbnew.LIB_ID("", "Custom"))
p = pcbnew.PAD(fp); p.SetNumber("1"); p.SetAttribute(pcbnew.PAD_ATTRIB_PTH)
p.SetDrillSize(pcbnew.VECTOR2I(800000, 800000))
p.Padstack().SetMode(pcbnew.PADSTACK.MODE_CUSTOM)
p.SetShape(pcbnew.In2_Cu, pcbnew.PAD_SHAPE_OVAL)
p.SetSize(pcbnew.In2_Cu, pcbnew.VECTOR2I(1400000, 1000000))
fp.Add(p)
pcbnew.PCB_IO_KICAD_SEXPR().FootprintSave(%r, fp)
'''


def layer(shape, w, h, ratio=None):
    out = dict(shape=shape, width=w, height=h)
    if ratio is not None:
        out["roundrect_rratio"] = ratio
    return out


STACK = dict(mode="front_inner_back", front=layer("rect", 1.6, 1.6),
             inner=layer("circle", 1.2, 1.2), back=layer("rect", 1.6, 1.6))


def run(binary, native_python):
    with tempfile.TemporaryDirectory(prefix="ailab42 native ") as scratch:
        root = Path(scratch)
        pretty = root / "Lib.pretty"
        env = os.environ.copy()
        env.update(APPDATA=str(root / "config"), HOME=str(root),
                   KICAD_CONFIG_HOME=str(root / "native-config"),
                   KICAD_DOCUMENTS_HOME=str(root / "docs"),
                   KONNECT_STATE_DIR=str(root / "state"))
        config = root / "konnect.toml"
        config.write_text('ipc_address = "inproc://ailab42-library-only"\n')
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

        def native(name, out, lib=None):
            r = subprocess.run([native_python, "-I", "-c", NATIVE, str(lib or pretty), name, str(out)],
                               env=env, capture_output=True, text=True, timeout=60)
            assert r.returncode == 0, r.stderr + r.stdout
            return json.loads(r.stdout.strip().splitlines()[-1])

        def th(number, x, **extra):
            pad = dict(number=number, type="thru_hole", shape="rect", x=x, y=0, width=1.6,
                       height=1.6, drill=0.8)
            pad.update(extra)
            return pad

        try:
            request("initialize", dict(protocolVersion="2025-06-18", capabilities={},
                                       clientInfo=dict(name="ailab42", version="1")))
            process.stdin.write(json.dumps(dict(jsonrpc="2.0",
                                                method="notifications/initialized")) + "\n")
            process.stdin.flush()
            call("load_toolset", dict(name="library"))
            path = pretty / "Probe.kicad_mod"

            # Phase 1: authored per-layer copper is what KiCad loads, saves and reloads.
            call("create_footprint", dict(output=str(path), name="Probe", pads=[
                th("1", 0, padstack=STACK, solder_mask_margin=0.075), th("2", 3)]))
            n = native("Probe", root / "Out1.pretty")
            for phase in ("before", "after"):
                p1 = n[phase]["1"]
                assert p1["mode"] == "front_inner_back", p1
                assert p1["layers"]["F.Cu"]["shape"] == "rect", p1
                assert p1["layers"]["F.Cu"]["size"] == [1600000, 1600000], p1
                assert p1["layers"]["In1.Cu"]["shape"] == "circle", p1
                assert p1["layers"]["In1.Cu"]["size"] == [1200000, 1200000], p1
                assert p1["layers"]["B.Cu"]["shape"] == "rect", p1
                assert p1["layers"]["B.Cu"]["size"] == [1600000, 1600000], p1
                assert p1["drill"] == 800000 and p1["mask"] == 75000, p1
                assert n[phase]["2"]["mode"] == "normal", n[phase]["2"]
            print(f"PASS native {n['version']} loads, saves, reloads rect front/back + circle inner "
                  "copper, 0.8 mm drill, mask margin preserved")

            # Phase 2: served readback equals what KiCad itself saved.
            saved = root / "Out1.pretty" / "Probe.kicad_mod"
            info = call("get_footprint_info", dict(footprint_path=str(saved), include_pads=True))
            ps = info["pads"][0]["padstack"]
            assert ps["mode"] == "front_inner_back", ps
            assert (ps["front"]["shape"], ps["inner"]["shape"], ps["back"]["shape"]) == (
                "rect", "circle", "rect"), ps
            assert ps["inner"]["size"] == dict(x=1.2, y=1.2), ps
            assert all(c["layer"] != "*.Cu" for c in info["pads"][0]["copper_layers"]), info["pads"][0]
            assert info["pads"][1]["padstack"]["mode"] == "normal"
            print("PASS served readback of the KiCad-saved file reports per-layer copper, not *.Cu")

            # Phase 3: edit a plain pad into a roundrect/oval stack; KiCad agrees layer by layer.
            call("edit_footprint_pad", dict(footprint_path=str(path), pad_number="2", padstack=dict(
                mode="front_inner_back", front=layer("roundrect", 1.6, 1.6, 0.2),
                inner=layer("oval", 1.4, 1.0), back=layer("roundrect", 1.6, 1.6, 0.25))))
            n = native("Probe", root / "Out2.pretty")
            for phase in ("before", "after"):
                p2 = n[phase]["2"]
                assert p2["mode"] == "front_inner_back", p2
                assert p2["layers"]["F.Cu"]["shape"] == "roundrect", p2
                assert abs(p2["layers"]["F.Cu"]["ratio"] - 0.2) < 1e-6, p2
                assert p2["layers"]["In1.Cu"]["shape"] == "oval", p2
                assert p2["layers"]["In1.Cu"]["size"] == [1400000, 1000000], p2
                assert p2["layers"]["B.Cu"]["shape"] == "roundrect", p2
                assert abs(p2["layers"]["B.Cu"]["ratio"] - 0.25) < 1e-6, p2
                assert n[phase]["1"]["layers"]["In1.Cu"]["shape"] == "circle", n[phase]["1"]
            print("PASS native edit: roundrect front/back + oval inner, other pad untouched")

            # Phase 4: clearing returns an ordinary pad; a refusal leaves the bytes alone.
            call("edit_footprint_pad", dict(footprint_path=str(path), pad_number="2", padstack=None))
            before = path.read_bytes()
            refused = call("edit_footprint_pad", dict(footprint_path=str(path), pad_number="1",
                                                      width=2.0), ok=False)
            assert "padstack" in json.dumps(refused), refused
            assert path.read_bytes() == before
            n = native("Probe", root / "Out3.pretty")
            assert n["after"]["2"]["mode"] == "normal", n
            assert n["after"]["1"]["mode"] == "front_inner_back", n
            print("PASS native clear returns a normal pad; header edit on a stack pad refused, "
                  "bytes unchanged")

            # Phase 5: a custom-mode pad that KiCad authored is read, never rewritten.
            native_dir = root / "Native.pretty"
            native_dir.mkdir()
            r = subprocess.run([native_python, "-I", "-c", CUSTOM_GEN % str(native_dir)], env=env,
                               capture_output=True, text=True, timeout=60)
            assert r.returncode == 0, r.stderr
            native_file = native_dir / "Custom.kicad_mod"
            info = call("get_footprint_info", dict(footprint_path=str(native_file),
                                                   include_pads=True))
            stack = info["pads"][0]["padstack"]
            assert stack["mode"] == "custom", info
            assert any(l["layer"] == "In2.Cu" and l["shape"] == "oval" for l in stack["layers"]), info
            before = native_file.read_bytes()
            refused = call("edit_footprint_pad", dict(footprint_path=str(native_file),
                                                      pad_number="1", padstack=STACK), ok=False)
            assert "custom" in json.dumps(refused), refused
            assert native_file.read_bytes() == before
            print("PASS KiCad-authored custom stack read per layer; rewrite refused, bytes unchanged")

            # Phase 6: edit the stack of a pad KiCad itself saved (tab-indented, multi-line).
            saved_lib = root / "KiCadSaved.pretty"
            saved_lib.mkdir()
            kicad_file = saved_lib / "Probe.kicad_mod"
            kicad_file.write_bytes((root / "Out1.pretty" / "Probe.kicad_mod").read_bytes())
            original = kicad_file.read_bytes()
            call("edit_footprint_pad", dict(footprint_path=str(kicad_file), pad_number="1",
                                            padstack=STACK))
            assert kicad_file.read_bytes() == original, "identical stack must not change KiCad's file"
            changed = dict(STACK, inner=layer("oval", 1.4, 1.0))
            call("edit_footprint_pad", dict(footprint_path=str(kicad_file), pad_number="1",
                                            padstack=changed))
            assert kicad_file.read_bytes() != original
            n = native("Probe", root / "Out6.pretty", lib=saved_lib)
            for phase in ("before", "after"):
                p1 = n[phase]["1"]
                assert p1["mode"] == "front_inner_back", p1
                assert p1["layers"]["In1.Cu"]["shape"] == "oval", p1
                assert p1["layers"]["In1.Cu"]["size"] == [1400000, 1000000], p1
                assert p1["layers"]["F.Cu"]["shape"] == "rect" and p1["mask"] == 75000, p1
            print("PASS served edit of a KiCad-saved multi-line pad: identical stack leaves the file "
                  "byte-identical, a changed inner layer is read back by KiCad")
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
