# T20: pinned-vocabulary vs redaction-policy sweep — plan

Status: proposed -> implemented. Acceptance is maintainer review on the PR.

## Contract

Follow-up from OBS-01 (#89): the default `RedactionPolicy`'s broad
`token` substring silently matched pinned GenAI keys and rewrote
legitimate token accounting into `[REDACTED]`. #89 fixed that one
vocabulary with `known_safe_keys`. This sweep checks whether the same
class of bug exists elsewhere. Ref: #13.

## Enumeration

Redaction paths that match on **keys** (the only place a vocabulary
collision can rewrite structured data):

- `telemetry.rs::redact_map` — the sole key-substring matcher.
  Forbidden substrings (default policy): `authorization`, `token`,
  `secret`, `api_key`, `apikey`, `password`, `cookie`. Exempt via exact
  match in `known_safe_keys` (derived from `pinned_safe_genai_keys()`).
  Applied to `log.attributes`, `span.attributes`, `metric.dimensions`.

Redaction paths that do **not** match on keys (no collision vector):

- `artifact.rs::redact_bytes`/`redact_str` (DAT-02a #78) — replaces
  only caller-supplied literal `secrets` values in content/kind/
  provenance. No key-substring matching, so no vocabulary can collide.
- `MetricSnapshot.name` (instrument name) is a `String` field, not a map
  key; `redact_record` redacts only `metric.dimensions` for metrics, so
  instrument names never hit the key-substring rule.

Pinned vocabulary (core `attributes` + `instruments` + `ABSENT_BY_DEFAULT`,
per GenAI conventions `8ffdf568e1b4391a99adb081db16e8102e36918e`):
every key was tested against every forbidden substring.

## Collision table

| Pinned key | Forbidden substring | In `known_safe_keys`? | Verdict |
| --- | --- | --- | --- |
| `gen_ai.request.max_tokens` | `token` | **No** | **COLLISION — fixed** (added to exemption) |
| `gen_ai.usage.input_tokens` | `token` | Yes | safe (exempt) |
| `gen_ai.usage.output_tokens` | `token` | Yes | safe (exempt) |
| `gen_ai.usage.cache_read.input_tokens` | `token` | Yes | safe (exempt) |
| `gen_ai.usage.cache_write.input_tokens` | `token` | Yes | safe (exempt) |
| `gen_ai.usage.reasoning.output_tokens` | `token` | Yes | safe (exempt) |
| `gen_ai.token.modality` | `token` | Yes | safe (exempt) |
| all other `gen_ai.*` / `error.type` / `fluzo.*` / `ABSENT_BY_DEFAULT` | none | n/a | no overlap |
| `gen_ai.client.inference.usage.*_tokens`, `...operation.*_tokens` (instrument names) | `token` | n/a | near-miss, NOT a collision: metric names are `String` fields, never map keys, so the key-substring rule never applies |

No pinned key contains `authorization`, `secret`, `api_key`, `apikey`,
`password` or `cookie`.

## Fix (minimal, same pattern as #89)

Add `attributes::REQUEST_MAX_TOKENS` to `pinned_safe_genai_keys()` so
the exemption is derived from the vocabulary constant and cannot drift.
Exact-match exemption only; the broad `token` rule still catches any
arbitrary credential-shaped key.

## Regression test

Table-driven test (`telemetry_tests.rs`): every pinned vocabulary key
survives the default policy unchanged (value preserved), while every
secret-shaped value under a credential-shaped key still gets redacted.
The table is built from the core constants so a future pinned key that
introduces a forbidden substring without an exemption fails the test.

## Verification (actual results)

- `cargo fmt --all -- --check`: clean (after `cargo fmt`)
- `cargo test --workspace --locked --offline`: green (runtime lib 170 incl. 3 new sweep tests; tui 179; http_simulator 12; core 31; cli 10; setup 4; diagnostics 2)
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`: clean
- `python3 -B -m unittest discover -s scripts -p 'test_*.py'`: 76 OK
- `python3 scripts/check_dev_setup.py`: foundation graph, toolchain, LSP config, 12 skills checked
- Negative check: with `REQUEST_MAX_TOKENS` removed from the exemption, `every_pinned_vocabulary_key_survives_default_policy` fails with `gen_ai.request.max_tokens -> [REDACTED]`, proving the regression test catches the collision
- Live inference: `not_run`
