# Provider gateway (AGT-01)

`fluzo-runtime::providers` implements the OpenAI-compatible Chat
Completions generative gateway with the complete streaming contract.
Every call passes the consent gate, the per-process admission
controller and the diagnostic hub; the gateway itself owns no policy.

## Modules

- `sse.rs` — pure incremental SSE framing: LF/CRLF/CR terminators,
  comments, multi-line `data:`, the `[DONE]` sentinel, a hard
  per-event byte ceiling, and typed errors for invalid UTF-8 or
  framing violations. No JSON knowledge, no I/O.
- `chat.rs` — request builder (`stream`, `stream_options.include_usage`)
  and `ChatStreamAssembler`: text deltas, tool-call fragments keyed by
  index, finish reasons, usage-only final chunks (unknown fields stay
  `None`), malformed-frame typing and a response size ceiling. At
  `finish()` every tool-call argument buffer must parse as a JSON
  object with id and name present; otherwise the call fails typed as
  `InvalidToolCall` and is never exposed (issue #20 AC1).
- `gateway.rs` — `ProviderGateway::complete()`: endpoint parse (http,
  no query/userinfo/percent escapes, `/v1` path only), DNS resolve,
  address-class consent, credential-reference resolution, admission
  (FIFO wait, cancellable, bounded), one physical attempt per retry
  budget unit, streaming assembly and pinned GenAI telemetry.

## Failure semantics (all typed, machine-readable via `code()`)

| Event | Outcome | Admission slot | Retry |
| --- | --- | --- | --- |
| 3xx (any) | `redirect_blocked`; the Location target is never contacted | Completed | never |
| 429 / quota | `rate_limited` | Completed | never |
| 5xx / connect failure before any delta | typed transient | Completed | bounded by `GatewayConfig::retries`, each retry re-enters admission |
| Transport break after any delta | `partial_stream` (display text only, no tool-call buffers) | Uncertain | never |
| Timeout before any delta | typed `timeout_*` (transient) | Completed | within budget |
| Timeout after deltas | `partial_stream` | Uncertain | never |
| Cancel before dispatch | `cancelled` | Completed | never |
| Cancel after dispatch | `cancelled` | Uncertain | never |
| Consent denied/required | `consent_*`, zero sockets opened | untouched | never |
| Admission denied | `admission_*`, zero sockets opened | n/a | never |

Once the request body is handed to the transport, cancellation and
breaks are treated as *dispatched with unconfirmed outcome*
(`Uncertain`): dropping the connection is not proof the provider
stopped. Uncertain slots end only through explicit reconciliation
(`AdmissionController::resolve_uncertain`), never by timeout.

## Credentials and consent

`api_key_env` names an environment variable resolved at call time;
the value goes only into the `Authorization` header (marked sensitive)
and never into errors, telemetry or assembled state. The consent gate
mirrors #110 address classification: `LoopbackOnlyConsent` by default,
`AcceptedEndpoints` for explicit per-run acceptance that is bound to
the accepted address class (a later class change fails closed).

## Telemetry

One client span (`chat <model>`, pinned GenAI vocabulary) plus
duration/time-to-first-chunk histograms and known-usage counters per
logical operation through `DiagnosticHub`. Unknown usage fields are
omitted, never zeroed. Retry consumption is recorded via
`UsageNormalizer::report_retry` with the distinct physical-retry
identity. Content attributes (`gen_ai.input.messages`,
`gen_ai.output.messages`) are never emitted here.

## Verified commands

```sh
cargo test -p fluzo-runtime --lib --locked --offline providers        # 52 pure tests
cargo test -p fluzo-runtime --test provider_gateway_simulator --locked --offline  # 13 fixture tests
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo test --workspace --locked --offline
```

All passed on 2026-10-17 (Linux x86_64, pinned toolchain 1.98.0).
Live inference: `not_run` (deterministic loopback fixtures only).

## Gaps

- No agent loop, harness or CLI wiring yet: `GatewayConfig` values
  (timeouts, retries, pool/capacity ids) must still be mapped from
  the typed settings registry by the future wiring layer.
- No compaction caller and no Laya `/v1/systemone` client (separate
  work).
- The simulator's scripted disconnect can abort before the response
  head flushes (documented in `admission_simulator`); gateway tests
  use gates between chunks to force deterministic flush ordering.
