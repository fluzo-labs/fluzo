# T22: self-diagnosing fixture discipline for quota tests — plan

Status: implemented. Acceptance is maintainer review on the PR.

## Contract

Follow-up hardening. #81 made store/migration fixtures self-diagnosing
and runner-deterministic (expect_open with bounded flake-only backoff,
report_and_log / note_recovered, canonicalized temp roots, forced 0700,
no bare unwrap on open/reopen/restore, deterministic close). The quota
tests (#80) predate that pattern and carry the same latent exposure to a
runner-scheduled `SQLITE_BUSY` surfacing as a bare `unwrap()` panic with
no environment evidence. Pure test-only change; zero production changes.
Ref: #12.

## Audit

- `quota_integration_tests.rs` — opens the store via `open_with`
  (`Store::open_quotated_with_probe(...).unwrap()`) and one direct
  `Store::open(...).unwrap()` in the no-guard test; `Root::new` used a
  raw `temp_dir()` with no canonicalization and no 0700. Reopen already
  routed through `close_and_reopen` -> `open_with`.
- `quota_tests.rs` — filesystem-only ledger fixture; never opens a
  `Store`, but its `Root::new` used a raw `temp_dir()` with no
  canonicalization (the drive got 0700 via `ensure_drive`, the root did
  not).

## Migration

- `test_diagnostics.rs`: added a reusable `expect_open(root, context,
  closure)` that mirrors the #81 store/migration discipline — bounded
  backoff on `Inaccessible`/`Io`/`Busy` only, `report_and_log` on every
  failed attempt, `note_recovered` on a recovered retry, panic with the
  full report on a semantic refusal. The open is a closure so the plain
  and quotated signatures share one self-diagnosing path.
- `quota_integration_tests.rs`:
  - `Root::new` now canonicalizes the temp base and forces 0700.
  - `open_with` routes through `expect_open` (was a bare `unwrap()`).
  - the no-guard test's direct `Store::open` routes through `expect_open`.
  - `close_and_reopen` keeps `close_for_test().unwrap()` for the close
    (deterministic close, same as `store_tests.rs`); its reopen already
    goes through `open_with`, now self-diagnosing.
- `quota_tests.rs`: `Root::new` now canonicalizes the temp base and
  forces 0700 on the root.

Every existing assertion is unchanged; only the failure surface moved.

## Verification (actual results)

- `cargo test -p fluzo-runtime --lib --locked --offline` x5: 181 passed, 0 failed, all five runs
- `cargo test --workspace --locked --offline`: green (runtime lib 181; tui 179; http_simulator 12; core 31; cli 10; setup 4; diagnostics 2)
- `cargo fmt --all -- --check`: clean (after `cargo fmt`)
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`: clean
- `python3 -B -m unittest discover -s scripts -p 'test_*.py'`: 76 OK
- `python3 scripts/check_dev_setup.py`: foundation graph, toolchain, LSP config, 12 skills checked
- Diff is test-only: `test_diagnostics` and `quota_integration_tests` are `#[cfg(test)]`; `quota_tests` is the `#[cfg(test)]` ledger fixture. No production file touched.
- Live inference: `not_run`
