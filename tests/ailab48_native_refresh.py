"""AILAB-48 native verification of the library-refresh slice against a scratch KiCad 10.0.6 editor.

Isolation (asserted before any call): the editor and the Konnect server run with a scratch
APPDATA / KICAD_CONFIG_HOME / KICAD_DOCUMENTS_HOME / TEMP / TMP under <root>; the server is
given the scratch socket address explicitly; nothing connects to the default endpoint or to an
existing editor. The editor process started here is always terminated by PID.

usage: python ailab48_native_refresh.py <konnect.exe> <kicad bin dir> <scratch root> [--stop-after launch]
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from ailab48_hidden_process import HiddenProcess  # noqa: E402
PAD_KEYS = ("mask", "paste", "ratio", "clearance", "zone", "gap", "angle", "die")


def scratch_env(root: Path):
    env = os.environ.copy()
    appdata = root / "appdata"
    for key in ("KICAD_API_SOCKET", "KICAD10_3RD_PARTY"):
        env.pop(key, None)
    env.update(
        APPDATA=str(appdata),
        KICAD_CONFIG_HOME=str(appdata / "kicad"),
        KICAD_DOCUMENTS_HOME=str(root / "docs"),
        TEMP=str(root / "t"),
        TMP=str(root / "t"),
        HOME=str(root),
        KONNECT_STATE_DIR=str(root / "state"),
    )
    return env


def assert_isolated(env, root: Path):
    for key in ("APPDATA", "KICAD_CONFIG_HOME", "KICAD_DOCUMENTS_HOME", "TEMP", "TMP"):
        assert Path(env[key]).resolve().is_relative_to(root.resolve()), f"{key} not isolated"
    assert "KICAD_API_SOCKET" not in env


def pipe_exists(socket_path: Path) -> bool:
    """NNG maps ipc:// to a named pipe on Windows whose name is the socket path."""
    try:
        return str(socket_path) in os.listdir(r"\\.\pipe")
    except OSError:
        return False


class Server:
    """Minimal stdio MCP client for the task-built Konnect server."""

    def __init__(self, binary, root, env, config_text):
        config = root / "konnect-live.toml"
        config.write_text(config_text, encoding="utf-8")
        self.proc = subprocess.Popen([str(binary), "--config", str(config)], cwd=root, env=env,
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.DEVNULL, text=True)
        self.rid = 0
        self.request("initialize", dict(protocolVersion="2025-06-18", capabilities={},
                                        clientInfo=dict(name="ailab48", version="1")))
        self.proc.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
        self.proc.stdin.flush()

    def request(self, method, params):
        self.rid += 1
        self.proc.stdin.write(json.dumps(dict(jsonrpc="2.0", id=self.rid, method=method,
                                              params=params)) + "\n")
        self.proc.stdin.flush()
        while True:
            line = self.proc.stdout.readline()
            assert line, "server exited before response"
            msg = json.loads(line)
            if msg.get("id") == self.rid:
                assert "error" not in msg, msg
                return msg["result"]

    def call(self, name, arguments, ok=True):
        result = self.request("tools/call", dict(name=name, arguments=arguments))
        assert bool(result.get("isError", False)) != ok, result
        return json.loads(result["content"][0]["text"])

    def call_text(self, name, arguments):
        result = self.request("tools/call", dict(name=name, arguments=arguments))
        text = result["content"][0]["text"]
        assert not result.get("isError", False), text
        return text

    def close(self):
        self.proc.stdin.close()
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def run_refresh(binary, py, root, env, socket_path, fixture):
    board = root / "Test.kicad_pcb"
    server = Server(binary, root, env, f'ipc_address = "{socket_path.as_posix()}"\n')

    def native_read():
        r = subprocess.run([str(py), "-I", str(HERE / "ailab48_native_fixture.py"), "read", str(board)],
                           env=env, capture_output=True, text=True, timeout=120)
        assert r.returncode == 0, r.stderr + r.stdout
        return json.loads(r.stdout.strip().splitlines()[-1])

    try:
        for toolset in ("pcb_components", "project", "library"):
            loaded = server.request("tools/call", dict(name="load_toolset", arguments=dict(name=toolset)))
            assert not loaded.get("isError"), (toolset, loaded)
        # The socket opens before the board has finished loading: retry the read-only dry
        # run while KiCad answers AS_NOT_READY.
        deadline = time.time() + 120
        while True:
            result = server.request("tools/call", dict(name="update_footprints_from_library",
                                                       arguments=dict(board=str(board), dry_run=True)))
            text = result["content"][0]["text"]
            if result.get("isError") and "AS_NOT_READY" in text and time.time() < deadline:
                time.sleep(3)
                continue
            assert not result.get("isError"), text
            plan = json.loads(text)
            break
        print("dry run:", json.dumps(plan)[:900])
        revision = plan.get("plan_revision") or plan.get("revision")
        assert revision, plan

        stale = server.call("update_footprints_from_library", dict(
            board=str(board), dry_run=False, expected_plan_revision="not-the-revision"))
        assert stale["status"] == "conflict", stale
        assert [d["code"] for d in stale["diagnostics"]] == ["stale_plan_revision"], stale
        assert stale["coverage"]["changed"]["applied"] == 0, stale
        print("stale revision answered with a conflict, nothing applied:", json.dumps(stale["diagnostics"]))
        print("save:", server.call_text("save_project", dict())[:120])
        before = native_read()
        assert before["pad"]["mask"] is None and before["pad"]["clearance"] is None, before
        print("PASS stale apply refused; saved board still has the stale pad:", json.dumps(before["pad"]))

        result = server.request("tools/call", dict(name="update_footprints_from_library", arguments=dict(
            board=str(board), dry_run=False, expected_plan_revision=revision)))
        text = result["content"][0]["text"]
        if result.get("isError"):
            # A readback mismatch after the commit is undone by a compensating commit
            # (see docs/AILAB-48-VERIFICATION.md). What must hold regardless: the board is
            # confirmed back in its original state.
            assert "independent readback differs" in text and "rolled back" in text, text
            print("apply failed verification and was rolled back:", text[:420])
            print("save:", server.call_text("save_project", dict())[:120])
            after = native_read()
            assert after == before, (before, after)
            print("PASS refused apply left the native board unchanged:", json.dumps(after["pad"]))
            again = server.call("update_footprints_from_library", dict(board=str(board), dry_run=True))
            assert again["status"] == "ready" and again["changes"], again
            print("PASS the refused plan is still planned afterwards (nothing was consumed)")
            return
        print("applied:", json.dumps(json.loads(text))[:600])
        print("save:", server.call_text("save_project", dict())[:120])
        after = native_read()
        lib = fixture["library"]
        for key in PAD_KEYS:
            assert after["pad"][key] == lib[key], (key, after["pad"], lib)
        assert after["pad"]["net"] == "SIG1", after
        keep = ("x", "y", "orientation", "side")
        assert [after[k] for k in keep] == [before[k] for k in keep], (before, after)
        print("PASS native reopen of the saved board: library pad settings present, net SIG1, "
              "position/orientation/side unchanged:", json.dumps(after))

        again = server.call("update_footprints_from_library", dict(board=str(board), dry_run=True))
        assert again["status"] == "noop", again
        print("PASS second dry run is a no-op")
    finally:
        server.close()


def main():
    binary, kicad_bin, root = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
    stop_after = sys.argv[sys.argv.index("--stop-after") + 1] if "--stop-after" in sys.argv else None
    env = scratch_env(root)
    assert_isolated(env, root)
    for sub in ("appdata/kicad/10.0", "t", "docs", "state"):
        (root / sub).mkdir(parents=True, exist_ok=True)
    py = kicad_bin / "python.exe"

    # 1. Native fixtures: library footprint with pad-local settings; board with a stale copy.
    r = subprocess.run([str(py), "-I", str(HERE / "ailab48_native_fixture.py"), str(root)],
                       env=env, capture_output=True, text=True, timeout=120)
    assert r.returncode == 0, r.stderr + r.stdout
    fixture = json.loads(r.stdout.strip().splitlines()[-1])
    print("fixture:", json.dumps(fixture))

    # 2. Editor settings: API server on (scratch config only).
    # Seeded so the editor never shows its first-run "Welcome to KiCad" or prompt dialogs
    # (versions match the files KiCad 10.0.6 itself writes).
    cfg = root / "appdata/kicad/10.0"
    (cfg / "kicad_common.json").write_text(json.dumps({
        "meta": {"version": 6}, "api": {"enable_server": True},
        "do_not_show_again": {"data_collection_prompt": True, "update_check_prompt": True},
    }), encoding="utf-8")
    (cfg / "pcbnew.json").write_text(json.dumps({
        "meta": {"version": 5}, "system": {"first_run_shown": True},
    }), encoding="utf-8")
    # The global symbol / design-block tables KiCad's first-run "KiCad Setup" dialog would
    # install (a modal dialog, which also makes the API answer AS_NOT_READY). Scratch config only.
    shutil.copyfile(kicad_bin.parent / "share/kicad/template/sym-lib-table", cfg / "sym-lib-table")
    (cfg / "design-block-lib-table").write_text("(design_block_lib_table\n  (version 7)\n)\n",
                                                encoding="utf-8")

    # 3. Register the scratch library through the Konnect server (scratch global table).
    server = Server(binary, root, env, 'ipc_address = "inproc://ailab48-library-only"\n')
    try:
        server.request("tools/call", dict(name="load_toolset", arguments=dict(name="library")))
        server.call("register_footprint_library", dict(
            library_path=str(root / "Lib.pretty"), nickname="Lib", scope="global"))
    finally:
        server.close()
    assert (root / "appdata/kicad/10.0/fp-lib-table").is_file(), "table not under the scratch config"
    print("registered Lib in the scratch global fp-lib-table")

    # 4. Launch the scratch editor on the scratch board; always terminate it by PID.
    socket_path = root / "t" / "kicad" / "api.sock"
    # KiCad has no headless mode: the editor runs on a private, never-displayed desktop.
    editor = HiddenProcess(kicad_bin / "pcbnew.exe", [root / "Test.kicad_pcb"], root, env,
                           f"ailab48-{os.getpid()}")
    print("editor pid", editor.pid, "(hidden desktop)")
    try:
        deadline = time.time() + 90
        while time.time() < deadline and not pipe_exists(socket_path):
            assert editor.poll() is None, f"editor exited early: {editor.returncode}"
            time.sleep(1)
        assert pipe_exists(socket_path), "scratch API socket never appeared"
        print("scratch API socket is up:", socket_path)
        if stop_after == "launch":
            return
        try:
            run_refresh(binary, py, root, env, socket_path, fixture)
        except BaseException:
            print("windows on the editor's hidden desktop:", editor.window_titles())
            raise
    finally:
        editor.terminate()
        editor.wait(timeout=15)
        print("editor terminated")


if __name__ == "__main__":
    main()
