---
plan: dat02a-artifact-capture
issue: https://github.com/fluzo-labs/fluzo/issues/72
has_completed_all_phases: false
---

# DAT-02a: artifact capture, finalization and reference integrity

## Baseline

- Branch `feat/dat02a-artifact-capture` from `origin/main` at `8eb3306`
  (DAT-01 #71 merged and maintainer-accepted).
- Design: PRD v0.2 sections 22.2 and 44.5, Architecture A03 at
  `fluzo-docs@60c5b0732fb710cdf705476cee8d9156a5ecd971`.
- Ground truth: merged DAT-01 contract (`STORAGE.md`,
  `fluzo-runtime::store`): protected drive, verified PRAGMAs, atomic
  `BEGIN IMMEDIATE` units, fd-anchored fail-closed discipline.

## Contracts

New module `fluzo-runtime::artifact` (types + `impl Store` methods).
All artifact bytes live under the managed root `.fluzodrive/artifacts/`,
reached only through a drive-anchored (`O_NOFOLLOW|O_DIRECTORY`) writer
owned by the store. No other component writes there.

- `ArtifactSpec { id, state_id, kind, provenance }` — identity, owning
  state row and capture provenance persisted in SQL, so DAT-02d can do
  shared-reference analysis without scanning bytes.
- `CaptureRequest { spec, content, secrets, omitted }` — fixture-driven
  producer input. Secret literals supplied by the caller's redaction
  layer are replaced with `[REDACTED]` in content and record fields
  before anything is persisted; any replacement marks `Redacted`.
- `finalize_artifact(request) -> FinalizedArtifact` — durability step 1:
  validate ids (single safe path component), redact, enforce the finite
  per-output limit `MAX_ARTIFACT_BYTES` (1 MiB; overflow stored as the
  prefix and marked `Truncated`), write staging file `0600`, fsync,
  rename into place, fsync directory. Returns recorded size and SHA-256.
  Finalized artifacts are immutable: duplicate finalize or re-capture of
  a committed id is `Conflict`.
- `commit_artifact_reference(finalized, updates, events)` — durability
  step 2: re-checks the finalized file (present, hash matches) before
  opening the transaction, then inserts the `artifact` reference row
  together with the owning state updates and journal events in one
  `BEGIN IMMEDIATE` unit. A reference can never commit without its
  finalized content, and the reference/state/event trio is atomic (FK on
  `artifact.state_id`).
- `verify_artifact(id)` — record-vs-file check: missing file, symlinked
  file, size or hash mismatch all report `Corrupt` with reason and never
  repair or re-finalize; matching content reports `Verified`; an
  `Omitted` record is consistent with no content file by contract.
- `detect_orphaned_artifacts()` — lists finalized or staging files under
  the managed root with no committed reference (detectable and
  reclaimable; actual reclamation is DAT-02d purge scope).
- `read_artifact_content(id)` — verify-then-read through `O_NOFOLLOW`;
  corrupt content is an error, never returned as if valid.

Schema: one new table `artifact(id PK, state_id FK->state, kind,
provenance, stored_bytes, content_hash, capture_flags)` added through the
existing open-time `CREATE TABLE IF NOT EXISTS` path. The versioned
migration framework is DAT-02c (#74) and is not pulled in here.

Dependency: `sha2 = "=0.11.0"` (default-features off, `std`) for
content hashing; closure (digest, crypto-common, cpufeatures,
generic-array, typenum) verified present in the pinned offline cache
before selection. Graph policy in `scripts/check_dev_setup.py` extended
with the reviewed closure and feature ceilings.

## Acceptance criteria mapping (issue #72)

| AC | Evidence (artifact_tests) |
| --- | --- |
| Managed root, store-owned writer, no symlink traversal | `artifact_ids_are_confined_to_the_managed_root`, `symlinked_content_is_never_followed`, anchored-dir creation |
| Finalization separate from reference commit; reference atomic with state+event | `capture_finalizes_content_before_reference_commit`, `reference_commit_requires_finalized_content`, `reference_commit_is_atomic_with_state_and_event` |
| Bounded capture with explicit markers | `capture_marks_truncation_at_the_limit` |
| Redaction before persistence; records and bytes secret-free | `capture_marks_redaction_and_stores_no_secret` |
| Crash boundaries never half-apply | `crash_after_content_write_before_reference_commit_leaves_reclaimable_orphan`, `crash_after_reference_commit_keeps_content_verifiable`, `reference_without_content_is_reported_corrupt_not_repaired`, `torn_content_file_is_detected_and_not_repaired` |
| Identity/size/provenance/owner persisted for shared-reference analysis | record fields asserted in `capture_finalizes_content_before_reference_commit`; orphan scan by recorded ids in `orphan_scan_lists_unreferenced_files_only` |
| Byte-level independent verification | `stored_bytes_are_independently_verified_at_byte_level` (raw `fs::read` + independent SHA-256) |
| STORAGE.md extended | docs phase |

## Exclusions honored

No quotas/admission (#73), no purge commands or reclamation execution
(#75), no migration framework (#74), no telemetry-archive behavior
(#76), no dedup, no real model/shell capture (fixture producers only).

## Verification plan (pinned toolchain, `--locked --offline`)

- `cargo test -p fluzo-runtime --locked --offline artifact` (focused)
- `cargo fmt --all -- --check`
- `cargo test --workspace --locked --offline`
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`
- `python3 -B -m unittest discover -s scripts -p 'test_*.py'`
- `python3 scripts/check_dev_setup.py`
- Live inference: `not_run`.

## Progress

| Phase | Implemented | Verification | Review |
| --- | --- | --- | --- |
| P1 artifact module + schema | done | green (15 artifact tests) | pending |
| P2 crash + safety fixtures | done | green (2 real-process crash fixtures) | pending |
| P3 docs + graph policy | done | green (fmt/clippy/workspace 299/python 74/checker) | pending |

## Next step

Review on the PR. Nothing accepted from this plan; merging must not close
#72 or the parent #12.

- Issue: [fluzo-labs/fluzo#72](https://github.com/fluzo-labs/fluzo/issues/72)
