# AILAB-45 sanitized benchmark plan

Baseline: d38a6ed6a118bc51f775517a4a1e9b64a9ed9964. Scope is a Python standard-library JSON-RPC stdio client, scenario inputs, captured byte metrics, and verification notes only.

1. Add counting tests first, observing failure before implementation. Count UTF-8 wire payload bytes (excluding line delimiters), schema JSON bytes separately, calls, notifications and listed tools. No tokenizer or billed-token claim.
2. Build this worktree server once with isolated target/state. Start a fresh process per eager/on-demand client policy with an explicit unused IPC endpoint and TEMP-only library assets. Never inspect real user projects or global library tables.
3. Measure starter-kit and eager discovery, selective batched load/list refresh of library and pcb_board, ordinary project and saved-board reads, synthetic symbol create/edit/readback, and footprint-library/model create/edit/readback. A deterministic synthetic PCB/model call trace is a mock workload, not live PCB success or measured batching savings.
4. Validate success and truthful refusal, independent artifact readback, and deterministic counter tests. Store baseline provenance and exact byte metrics with environment assumptions; rank observed/prospective opportunities separately.
5. Run focused tests and one broader harness validation, review only owned paths, diff-check and commit with AILAB-45. Native live IPC remains unverified; no lease needed for offline work.

Design choice: measure actual served schemas rather than regenerate the catalogue from source; retain existing routing/toolset semantics. Dynamic timing/path/UUID content makes some actual response sizes environment-dependent; canonical JSON schema sizes remain deterministic for a fixed platform/build. Do not weaken document binding, transactions, revision checks or readback to improve counts.
