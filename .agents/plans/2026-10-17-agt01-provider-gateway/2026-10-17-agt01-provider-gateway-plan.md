# T33: AGT-01 generative provider gateway and streaming contract — plan

Status: approved-for-execution via task handoff (/tmp/fluzo-task-T33.md).
Ref: #20. PRD 8, 34, 42.1, 43, 45.2 and Architecture A04/A08/A09 at
fluzo-docs revision 60c5b0732fb710cdf705476cee8d9156a5ecd971.

## Contract

OpenAI-compatible Chat Completions gateway with the complete streaming
contract, executed only behind the existing consent (#110) and
per-process admission (#112) gates, observed only through the existing
DiagnosticHub with the pinned GenAI vocabulary (#89/#105). No new
crates, no new registry dependencies (hyper client + tokio + serde_json
are already production-pinned for OTLP export).

Acceptance mapping (issue #20):
- AC1 "Validated tool calls are assembled before dispatch": the
  assembler completes and JSON-validates every streamed tool-call
  argument buffer before the call is exposed to any dispatcher; a
  fragment is never dispatchable.
- AC2 "usage-only chunks, redirects, cancellation, non-quota retries
  and unknown usage are handled against HTTP fixtures": each is a
  named simulator test below.
- AC3 evidence + docs linked from the PR.

Key rules:
- Redirects: any 3xx is a typed `redirect_blocked` outcome; the
  Location target is never contacted; sanitized summary only (no
  userinfo/query/fragment, no headers, no body) per PRD 42.1.
- 429/quota: typed `RateLimited`, zero automatic retry, `Retry-After`
  ignored (core `classify_status`, already merged).
- Retries: only Transient (5xx/connection-before-send), bounded by
  `harness.provider_retries`, each retry re-enters admission, retry
  usage recorded via `UsageNormalizer::report_retry`. A partial
  stream (any delta already surfaced) is NEVER retried: the outcome is
  `PartialStream` with the admission slot left Uncertain.
- Cancellation: before socket open releases the slot Completed (nothing
  was sent); after dispatch releases Uncertain. Never auto-retries.
- Timeouts from typed settings: connect 10s / first output 90s /
  idle stream 30s / whole request 180s defaults; enforced with
  tokio timers; a first-output or idle timeout after deltas is
  PartialStream, before deltas is a typed timeout failure (retry only
  if nothing was received and class is transient-safe... a timeout
  before any response byte is treated as connection-transient).
- Credentials: only `api_key_env` references resolved from the process
  environment at call time; the value never enters logs, diagnostics,
  error strings or assembled state.
- Consent: endpoint address class computed like #110 (loopback/private/
  public/unresolved); loopback allowed; non-loopback requires an
  injected `ConsentGate` decision; denial = zero sockets.
- Admission: every gateway call carries purpose class, capacity id,
  pool, budgets and remaining task time through
  `AdmissionController::try_admit`; queued waits are async, bounded
  and cancellable; the slot is held until the stream fully completes
  (not until headers/first token); disconnects leave it Uncertain.
- Telemetry: one client span + duration/TTFC histograms + usage
  counters per logical operation using the pinned instrument names;
  unknown usage omitted, never zeroed; content attributes stay absent.

## Phases

### P1 — pure streaming contract (core of the increment)

- `providers/sse.rs`: incremental SSE framing (LF/CRLF/CR, comments,
  multi-line data, `[DONE]`, hard buffer ceiling). Pure; unit tests
  feed arbitrary byte-boundary splits.
- `providers/chat.rs`: request builder (stream + include_usage +
  max_tokens from settings) and `ChatStreamAssembler`: text deltas,
  tool-call fragments by index, finish reasons, usage-only final
  chunk, unknown usage stays `None`, malformed frame typed error,
  tool-call arguments JSON-validated at finish (AC1). Unit tests.

### P2 — gateway execution

- `providers/gateway.rs`: `ProviderGateway` composed of endpoint +
  credential-reference resolution, `ConsentGate` trait (loopback-only
  default impl), `AdmissionController` + `DiagnosticHub` handles.
  Async `complete()` implementing the flow above with tokio timeouts
  and a `watch` cancellation channel. Hyper client with redirect
  following disabled by construction (manual status handling).

### P3 — simulator integration + docs

- `tests/provider_gateway_simulator.rs` on the hardened loopback
  fixture (#81/#94 discipline): normal stream with usage-only final
  chunk; fragmented tool-call assembly over HTTP; 429 exactly one
  request; redirect blocked with a SECOND fixture listener proving
  zero requests/bytes at the target; mid-stream disconnect →
  PartialStream + uncertain slot + no retry; transient 503 → bounded
  retry re-entering admission; cancel mid-stream → uncertain, no
  retry; first-output timeout; admission-full → zero sockets;
  consent-denied → zero sockets; telemetry records carry pinned
  names and no content attributes and no secret values.
- `PROVIDER_GATEWAY.md`: contract, gate wiring, retry/uncertain
  semantics, verified commands, gaps.
- `AGENTS.md`: AGT-01 status paragraph + focused checks.

## Verification (commands to run and record)

- `cargo test -p fluzo-runtime --lib --locked --offline providers`
- `cargo test -p fluzo-runtime --test provider_gateway_simulator --locked --offline`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`
- `cargo test --workspace --locked --offline`
- `python3 -B -m unittest discover -s scripts -p 'test_*.py'`
- `python3 scripts/check_dev_setup.py`
- Live inference: `not_run`

## Non-goals

No agent loop/harness integration, no compaction caller, no Laya
`/v1/systemone` client (seeded as separate work), no cross-instance
coordination, no provider fallback, no real endpoints, no new
dependencies.

## Verification results (2026-10-17, toolchain 1.98.0, Linux x86_64)

- P1/P2/P3 implemented on top of the WIP checkpoint commit.
- `cargo test -p fluzo-runtime --lib --locked --offline providers` — 52
  pure tests pass (16 SSE framing, 15 chat assembly/AC1, 8 gateway unit,
  plus existing).
- `cargo test -p fluzo-runtime --test provider_gateway_simulator
  --locked --offline` — 13 fixture tests pass: normal stream with
  usage-only final chunk; fragmented tool-call assembly; incomplete
  tool-call never dispatchable (AC1); 429 with retry budget present
  produced exactly one request; redirect blocked with a second fixture
  listener proving zero requests at the target; mid-stream disconnect
  after deltas -> PartialStream + Uncertain slot + no retry; transient
  503 -> bounded retry (attempts=2) re-entering admission; transient
  budget exhausted -> typed error, one request; cancel mid-stream ->
  Cancelled + Uncertain + no retry + fixture cancelled accounting;
  first-output timeout typed; admission-full -> zero sockets;
  consent-denied -> zero sockets; telemetry carries pinned names,
  purpose and finish reasons with no secret value and no content
  attributes.
- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --locked --offline -- -D
  warnings` — clean.
- `cargo test --workspace --locked --offline` — all pass (440 runtime
  lib tests, 184 TUI, simulator suites, CLI).
- `cargo build --workspace --locked --offline` — ok.
- `python3 scripts/check_dev_setup.py` — ok.
- `python3 -B -m unittest discover -s scripts -p 'test_*.py'` — 90 ok.
- Live inference: `not_run`.

Implementation notes found during verification:
- The fixture's scripted disconnect can abort before the response head
  flushes (same documented race as `admission_simulator`); gateway
  simulator tests insert gates between chunks for deterministic flush
  ordering.
- Cancellation after the request body is handed to hyper is treated as
  dispatched (Uncertain slot): `send_request` failing or being cancelled
  does not prove the provider never saw the request.
