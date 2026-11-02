# AILAB-45 benchmark

Run from the repository root with Python 3.10+ and an explicit development binary:

```powershell
python -m unittest discover -s tools/benchmarks/ailab45 -v
python tools/benchmarks/ailab45/benchmark.py --server target-ailab45/debug/konnect.exe --output tools/benchmarks/ailab45/baseline.json
```

Build the baseline checkout with `cargo build --locked -p konnect --bin konnect`, an exclusive `CARGO_TARGET_DIR`, `KONNECT_STATE_DIR`, and the existing development toolchain. Do not use an installed server. The harness never starts KiCad: it pins a private in-process IPC endpoint with no responder and disables CLI/editor executables. All design/library writes use served Konnect tools in a fresh TEMP directory. Child state/config/home are isolated. No global library registration occurs.

The scenario file contains synthetic library inputs and a separately labelled synthetic PCB call projection. Each discovery policy gets its own process and scratch files. The on-demand policy uses the default starter kit, lists toolboxes, loads `library` and `pcb_board` together, then refreshes the catalogue once. The eager policy requests all schemas once. They create a project through the server builder, then read project info and saved PCB layers with explicit saved provenance. They perform identical subsequent business calls, sequentially, including independent disk and served readback and expected invalid-input refusals. The eager run is a measured client-configuration comparison, not a product before/after improvement.

Metrics count exact request and response UTF-8 payload bytes, excluding newline delimiters. Schema and complete tool-definition sizes use canonical compact sorted-key JSON; both overlap response bytes and must never be added to them. Each row records tool call count and list size. Server notifications are counted separately while awaiting each response; no claim about delayed notifications after the final response. The client initialized notification is recorded separately. No tokens, tokenizer, billing, network framing, hidden client prompt copies, or transport timing are inferred.

TEMP paths and server-generated IDs change some payload lengths. The baseline records scratch-path byte length, binary/scenario hashes, git HEAD, serving version and environment. Counting tests are deterministic; complete live responses need not be byte-identical across reruns. Results contain counts only, not private paths or real design data. Successful model metadata readback does not verify STEP existence, rendering, board placement, native undo, or native revision guards. No native IPC lease is requested or used.

The synthetic 57-item PCB trace is not dispatched, not a captured live session, and not schema-validated. Its names/arguments are illustrative only; it is useful for deterministic request-count geometry, not functionality validation. The user's 114 calls for 57 footprints is historical user evidence, not this harness's measurement or a batching saving. Bulk updates from another worker are not integrated; any batching benefit remains prospective.

On timeout or unexpected failure the harness exits nonzero and does not emit a fresh baseline result. Each child is killed and reaped; TEMP files are removed. An existing output file remains untouched on failure, so use a new output filename for evidence runs. A supplied server path is explicit operator authority; verify its provenance before running. The recorded SHA-256 identifies the exact executable, while git HEAD identifies the source checkout rather than proving an arbitrary supplied executable was built from it.

Scenario SHA-256 uses canonical compact sorted-key UTF-8 JSON, making fixture identity independent of checkout line endings/BOM. This metadata normalization does not change the measured request/response payloads.
