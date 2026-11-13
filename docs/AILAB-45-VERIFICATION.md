# AILAB-45 verification

## Scope and provenance

Owned files are `tools/benchmarks/ailab45/`, this report and `docs/superpowers/plans/2026-11-02-ailab-45-benchmark.md`. Product modules, manifests, lockfiles and generated catalogue are unchanged. The worktree started at `d38a6ed6a118bc51f775517a4a1e9b64a9ed9964`; origin is `https://github.com/kynze/Konnect.git`, upstream is `https://github.com/mixelpixx/Konnect.git`. Installed 0.13.0 / 6bbe3e4 was neither used nor modified.

The harness uses Python standard library and a worktree-built executable. Build output/state are isolated (`target-ailab45`, TEMP state), with `CARGO_BUILD_JOBS=2` after the initial toolchain-path failures. Existing tool dependencies are under the supplied `.local` toolchain; no dependencies were installed. Initial shell probes lacked cargo; the first compilation failed at nng-sys because CMake was missing from PATH; adding the existing `.local/build-python/cmake/data/bin` resumes the same isolated build.

## Original failure and focused checks

Before `benchmark.py` existed, `python -m unittest discover -s tools/benchmarks/ailab45 -v` failed with `ModuleNotFoundError: No module named 'benchmark'`. After implementation, the three initial tests passed: UTF-8 multibyte counting, schema/response separation, deterministic JSON with no line delimiter. Three additional tests cover truthful protocol/tool errors, explicitly unobserved synthetic projection and independent-readback failure detection. This is a new benchmark acceptance failure, not a reproduced product defect or a production fix.

## Measurement contract

`baseline.json` records exact UTF-8 request/response payload bytes, per-row tools/call count, returned tool count, separately calculated canonical input-schema and complete definition bytes, notifications, serverInfo, binary/scenario SHA-256 and OS/Python environment. Schema/definition bytes overlap response bytes. No tokenizer is used and no billed-token claim is made.

On-demand and eager are fresh, comparable client-policy sessions against the same binary, performing identical business scenarios after discovery. On-demand uses the existing starter kit and one array-valued toolset load plus one refresh. No product optimization or bulk-update branch is included. Each session creates only server-built scratch artifacts, explicitly binds paths, uses saved PCB provenance, checks real disk and served readback, and checks unchanged artifacts after refused invalid edits. Production transaction and revision checks remain intact.

## Limitations and next owner

No default IPC endpoint, native editor, installed configuration, real project, PCB573 repository, secret, remote publication or Plane mutation is involved. All native live behavior, native undo/revision guards, KiCad rendering and STEP geometry/existence are unverified. The private inproc endpoint has no peer; saved/offline results establish no live-board behavior. No native lease was requested or used. Windows is the observed platform; Unix conditional reload-tool differences are unobserved.

The 57-item PCB trace is synthetic and undispatched, with deliberately illustrative API arguments; its request counts/bytes establish only workload geometry. The historical 114 calls for 57 footprints remains user-supplied evidence. There is no measured batching before/after comparison or savings claim. The next owner is the coordinator for review/integration and the separate bulk-update owner for any future comparable native/mock before/after run, preserving document binding, readback and revision guards.
## Observed baseline (2026-11-02)

The completed isolated `cargo build --locked -p konnect --bin konnect` finished in 5m01s after resolving existing tool paths. Serving handshake reports Konnect 0.13.0; executable SHA-256 is `dfc1a889f012a6d85a68dbe65794f8dc3431337a91527c755c2a9f62742293d7`. This is the fresh d38a6ed worktree build, not the installed executable. Environment: Windows 11 AMD64, Python 3.12.10, pinned Rust 1.96.0, debug profile. Both scratch directory paths were 61 UTF-8 bytes.

Command: `python tools/benchmarks/ailab45/benchmark.py --server target-ailab45/debug/konnect.exe --output tools/benchmarks/ailab45/baseline.json` exited 0. Both fresh sessions passed five evidence checks (saved PCB provenance/unchanged file, symbol disk+served readback, symbol refusal unchanged, model disk+served readback, model refusal unchanged); no checks were skipped or ignored. Model tests prove reference metadata only.

| Measured metric | On-demand | Eager |
|---|---:|---:|
| JSON-RPC requests with responses | 19 | 16 |
| Actual tools/call requests | 16 | 14 |
| Initial returned tools | 21 | 236 |
| Tools after selective load | 52 | already 236 |
| Total request payload bytes | 4,012 | 3,731 |
| Total response payload bytes | 90,605 | 206,984 |
| Canonical input schema bytes, cumulative listings | 43,823 | 119,602 |
| Canonical complete definition bytes, cumulative listings | 65,911 | 193,135 |
| Server notification payload bytes / count | 61 / 1 | 0 / 0 |

Both clients also send one 54-byte initialized notification, separately recorded in JSON. Eager omits toolbox discovery/load/refresh; all 14 subsequent business tool calls match by operation and intent. Request/response envelope IDs can differ in digit count, so identical workflow semantics does not imply identical byte size. `get_active_toolsets` legitimately returns more state in eager mode.

Discovery detail: starter-kit list is 10,870 response bytes (21 tools, 4,303 input schema bytes); toolbox discovery is 4,645 bytes; batched `load_toolset` is 9,074 bytes; refreshed list is 55,204 bytes (52 tools, 39,520 input schema bytes). Eager initial list is 193,416 bytes (236 tools, 119,602 input schema bytes). These counts replace the approximate token claims in older development prose for this benchmark; they are not token measurements.

The pure synthetic projection produces 114 illustrative requests / 18,114 request bytes / unknown response bytes. It does not invoke the server, validate PCB mutations, or quantify a bulk update benefit.

## Ranked opportunities and compatibility risks

1. **Use existing selective toolsets when supported by the client.** This complete measured scenario used 116,379 fewer response bytes with on-demand routing, at the cost of two extra tool calls and one extra list request. No architecture change is needed. Clients that cache only the initial tool list or ignore list-change notifications still need eager discovery; never remove that compatibility path. Longer tasks loading many toolsets can reverse the tradeoff, so this is no universal savings target.
2. **Batch known toolset loads and avoid orchestration-driven catalogue churn.** This benchmark already loads library/pcb_board together and refreshes once. The 9,074-byte load response and 55,204-byte refresh show the boundary to measure before proposing refinements. Repeated full listings or serial loads by a client are orchestration costs, not evidence of per-mutation server overhead. Notification handling and schema availability must stay correct; no further savings was measured here.
3. **Keep schema retrieval distinct from domain work.** Initial eager discovery alone is 193,416 of 206,984 response bytes. Optimize observed catalogue/client behavior before proposing a generic execute tool or reducing verification responses. Schema compaction, targeted discovery or per-client caching would be prospective product changes requiring strict-schema compatibility and refreshed measurements; none is implemented here.
4. **Evaluate bounded bulk PCB/model workflows separately.** The separate bulk-update owner can measure comparable runs with real document binding, independent readback, revision guards, refusal/partial-outcome evidence and a safe scratch IPC lease/mock. This benchmark establishes no native PCB update speedup, no 114-to-N result and no reason to remove verification calls.

## Final validation and handoff

The coordinator owns integration and any Plane updates. Only owned files are committed; build output, Python caches and the pre-existing untracked `.serena/` directory are excluded. Final check evidence and commit identifier are sent in the Orca completion message; git history identifies this report's commit without a self-referential embedded hash.
Final checks: `python -m unittest discover -s tools/benchmarks/ailab45 -v` passed 6/6. A process-local negative control replaced byte measurement with decoded character length: exactly the UTF-8 regression failed (`13 != 14`); the following normal run passed all six tests, with no source changes required for the control. Broader baseline integrity validation verified 2 policies, 35 measured request/response rows, 30 actual tool calls, 10 evidence checks, 4 expected refusals, all counter sums, schema/definition/response bounds, binary hash and scenario hash. There were no ignored tests. `git diff --check` and owned staged-file review completed before commit. No workspace rebuild or native tests were added after the successful isolated build.

Scenario SHA-256 uses canonical compact sorted-key UTF-8 JSON, making fixture identity independent of checkout line endings/BOM. This metadata normalization does not change the measured request/response payloads.
