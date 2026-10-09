---
name: "2026-10-09-endpoint-consent"
description: "Make model discovery refuse to send a cleartext HTTP catalog request to a non-loopback endpoint until the operator explicitly accepts that exact endpoint, with the resolved address class shown before the first byte leaves the process."
created_at: "2026-10-09T08:19:50Z"
last_implementation_at: "2026-10-09T13:55:36Z"
has_completed_all_phases: false
---

# Endpoint-scoped cleartext consent before model discovery

## Source and revision

- Plan identifier: `endpoint-scoped-consent`.
- Source: [fluzo-labs/fluzo#55](https://github.com/fluzo-labs/fluzo/issues/55), "Surface warned endpoint-scoped consent before non-loopback HTTP discovery".
- Reviewed revision (issue): `updatedAt 2026-10-08T08:32:48Z`, state `OPEN`, labels `area:security` and `type:feature`, no assignee, no linked PR.
- Reviewed revision (consumer): `31d4fb926d722fb0d758512be98c00766a8a7ed2` on `origin/main`. Local HEAD at planning time was `f6308db` on `fix/settings-restart-pending-line` (open PR #60, unmerged). This plan targets `origin/main` and does not build on that branch.
- Design baseline: [PRD 26.1](https://github.com/fluzo-labs/fluzo-docs/blob/3c7fc8e1f6934922ad66fe3eda8123854620839b/PRD.md) and architecture 10.2 at fluzo-docs revision `3c7fc8e1f6934922ad66fe3eda8123854620839b` (the accepted DNS amendment, landed via fluzo-labs/fluzo-docs#12); [AGENTS.md](../../../AGENTS.md) dependency boundaries and the non-negotiable external-I/O rules; [CONFIGURATION.md](../../../CONFIGURATION.md); [TUI.md](../../../TUI.md).
- Approval: content and destination approved by the user on 2026-10-09, with decisions D1 through D4 resolved as recommended below. This annotation is not proof of authorization by itself.

## Goal

An operator adding a model by host name sees one explicit warning naming the exact endpoint, the resolved address, the cleartext transport, and whether a credential reference is attached, before the first HTTP request leaves the process. The runtime refuses to connect to any non-loopback endpoint without a per-run acceptance bound to that exact endpoint, and re-resolves at connect so the address actually used cannot differ in class from the one shown.

## Scope and exclusions

In scope:

- An explicit resolution step ahead of `connect`, with the resolved address classified as loopback, private, or public.
- A per-run, endpoint-bound acceptance carried in process memory and enforced by the runtime, not by the view.
- A wizard step that presents the warning and takes accept or decline, with acceptance invalidated by editing the endpoint.
- Redacted typed diagnostics for both the accepted class and a declined attempt.
- Documentation of the resulting contract in `CONFIGURATION.md` and `TUI.md`, and a pointer replacing the open note in the PRD 26.1 amendment.

Preserved unchanged:

- Endpoint syntax validation, including the rejection of userinfo, query credentials, `#`, `@` and `%`.
- The redirect prohibition, the five-second deadline, the response size bound, and the no-automatic-retry rule.
- Loopback discovery behavior: no added friction, no extra prompt.
- Typing, paste, resize and window focus stay request-free, as they are today.
- `.fluzo` contents and the repository configuration model. The acceptance is never written there.

Explicitly excluded:

- Adding HTTPS to discovery.
- General credential-service endpoint scoping, owned by RUN-02 (#16).
- Any change to redirect handling, deadlines, size bounds, or retry policy.
- A telemetry or OTLP pipeline, owned by OBS-01 (#13).

## Verified context

Observed facts:

- `crates/fluzo-runtime/src/model_discovery.rs:198` calls `tokio::net::TcpStream::connect((target.host.as_str(), target.port))`. The process never learns the resolved address before the socket is established, which is the gap this plan closes.
- `endpoint()` at `crates/fluzo-runtime/src/model_discovery.rs:155` validates scheme, host, port, path and authority, and rejects ambiguous or credential-bearing URLs. It performs no name resolution and no address classification.
- `crates/fluzo-tui/src/model_wizard.rs:148` `query()` normalizes the typed URL and calls `port.submit(...)` directly, moving to `Step::Loading`. There is no intermediate confirmation step. `Step` is defined at `model_wizard.rs:17` with variants `Kind, Provider, Authorization, Header, Endpoint, Loading, Models, Alias, Review`.
- `crates/fluzo-core/src/model_discovery.rs` defines `PROTOCOL = 2`, `Request { protocol, id, endpoint, authorization_env }`, `Status { Unknown, Pending, Complete }`, and an `Error` enum with no consent-related variant.
- No `tracing`, telemetry, or OTLP dependency exists in the root manifest or any crate manifest. Criterion 8 therefore cannot be satisfied by a diagnostics pipeline; the available surface is the typed notice and error values plus the sanitized status line.
- The discovery service is worker-owned (`ModelDiscoveryService::start()` at `model_discovery.rs:24`, with `submit`, `status`, `cancel` and a `Drop` join), so resolution can stay off the input and control path as AGENTS.md requires.
- `safe_text` at `crates/fluzo-tui/src/setup.rs:69` escapes control characters and bidi or isolate ranges, and is the shared display sanitizer for configuration text.
- SIM-01 provides strict, rejecting HTTP/SSE fixtures and a Linux private-network profile, which is the deterministic harness for the P1 negative cases.

Hypotheses not yet verified:

- Whether the platform resolver returns all addresses for a name and whether a class can differ between the probe and the connect. P1 must handle the multi-address case rather than assume one result.
- Whether the existing wizard PTY harness can express the new step's keyboard flow without extension.

## Decisions, dependencies, and risks

### Decisions (approved as recommended)

| | Decision | Resolution |
| --- | --- | --- |
| D1 | Contract shape for the notice and the acceptance | Add `AddressClass { Loopback, Private, Public, Unresolved }` and `Notice { endpoint, resolved, class, credential_attached }` to core, add `Status::Notice(Notice)`, add `Error::ConsentRequired` and `Error::AddressClassChanged`, and bump `PROTOCOL` from 2 to 3. The port gains a resolve-only call and an acceptance call. Rejected: a `stage` field on `Request`, which overloads one message with two purposes. |
| D2 | Meaning of "diagnostics" with no telemetry dependency | Use the typed notice and error values plus the sanitized status line only. No new dependency. Real three-path diagnostics stay with OBS-01 (#13). |
| D3 | The PRD 26.1 amendment note | Lives in `fluzo-labs/fluzo-docs`, a different repository. It needs its own PR and its own explicit approval; P3 is split accordingly. |
| D4 | Branch strategy | New branch from `origin/main` (`31d4fb9`). `fix/settings-restart-pending-line` and its open PR #60 stay untouched. |

### Dependencies

- The accepted DNS amendment in fluzo-docs `3c7fc8e1f6934922ad66fe3eda8123854620839b`, which permits localhost, private addresses and DNS names over cleartext HTTP with the operator responsible for locality. Accepted and landed.
- The merged model wizard in fluzo-labs/fluzo#53. Accepted and in `main`.
- SIM-01 HTTP simulator fixtures for deterministic negative tests. Accepted and in `main`.

### Risks

- **Interim usability.** After P1 and before P2 lands, a non-loopback endpoint returns `ConsentRequired` and the wizard cannot add it. Acceptable at this stage only if P2 follows immediately; recorded so it is not mistaken for a regression introduced later.
- **Trust-on-first-use window.** Between the address shown and the address connected there is always a gap. Re-resolving at connect and refusing when the class changes bounds it to a same-class swap; it does not eliminate it. The plan states this bound honestly rather than claiming the gap is closed.
- **Protocol bump.** `PROTOCOL 2 → 3` touches the discovery contract. Both ends live in this workspace, so the bump is contained, but every existing discovery test asserting protocol 2 must be updated deliberately, not silently.
- **Resolver behavior variance.** Multi-address names, IPv4/IPv6 ordering and resolver caching can differ across machines. P1 must classify conservatively: if any resolved address is public, the endpoint is treated as public.
- **Credential exposure.** The warning must state whether a credential reference is attached without ever rendering the referenced value. The reference name is operator-supplied and safe to show; the resolved environment value never is.

## Phases

### P1: Runtime endpoint classification and consent enforcement

- Outcome: discovery cannot establish a socket to a non-loopback endpoint without a matching per-run acceptance. The resolved address class is observable before any connect, and a class change between the probe and the connect aborts the request. Loopback discovery is unchanged.
- Owning repository: `fluzo-labs/fluzo` (`fluzo-core`, `fluzo-runtime`).
- Dependencies: none beyond the accepted baseline. P2 depends on this phase, not the reverse.
- Affected contracts:
  - `fluzo-core/src/model_discovery.rs`: `PROTOCOL` 2 to 3; new `AddressClass`; new `Notice`; `Status::Notice(Notice)`; `Error::ConsentRequired`; `Error::AddressClassChanged`.
  - `ModelDiscoveryPort`: a resolve-only operation that performs no connect, and an acceptance operation recording the exact endpoint plus class for the current run.
  - `fluzo-runtime/src/model_discovery.rs`: explicit resolution before connect, conservative classification, consent lookup before connect, re-resolve and compare at connect, redacted typed diagnostics.
- Acceptance criteria:
  - A non-loopback endpoint submitted without acceptance returns `ConsentRequired` and performs zero connects, verified by a simulator that would reject an unexpected request.
  - A loopback endpoint proceeds with no acceptance required and no added step.
  - The notice names the endpoint, the resolved addresses, the class, and whether a credential reference is attached, and never contains the resolved credential value.
  - Acceptance bound to endpoint A does not authorize endpoint B.
  - If the class changes between the probe and the connect, the request fails with `AddressClassChanged` and no request is sent.
  - A name resolving to any public address is classified public.
  - An unresolvable host fails closed with `Unresolved` and no hang, matching the existing unresolvable-host behavior.
  - Acceptance lives only in process memory; nothing is written to `.fluzo` or any file, verified by asserting the file is untouched.
  - A declined or absent acceptance leaves no partial state staged in the service.
- Actions:
  - [x] Implement explicit resolution, classification, the consent contract, and the connect-time re-resolve guard.
  - [x] Add focused deterministic tests for each acceptance criterion above, including the negative cases, using the existing simulator fixtures.
  - [x] Update existing discovery tests for the protocol bump deliberately and record what changed.
  - [x] Run applicable checks and record actual results.
  - [x] Present evidence and stop for review before another phase.
- Verification: `cargo test -p fluzo-runtime --locked --offline model_discovery` and `cargo test -p fluzo-runtime --test http_simulator --locked --offline` from the repository root, then `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`, `cargo test --workspace --locked --offline`, and `python3 scripts/check_dev_setup.py`. Evidence is the test names and results at the tested working revision, plus a simulator log showing zero requests for the unconsented case.
- Risks and recovery: the protocol bump breaks no external consumer because both ends are in this workspace. If the re-resolve guard proves too strict in practice, the fallback is to keep the guard and widen the accepted comparison, decided at review rather than by removing the check.

### P2: Wizard cleartext warning step

- Outcome: the operator sees the warning between entering the URL and the discovery request, and can accept that endpoint or back out. Loopback users see nothing new.
- Owning repository: `fluzo-labs/fluzo` (`fluzo-tui`).
- Dependencies: P1, because the step renders the notice the runtime produces and the acceptance it enforces.
- Affected contracts:
  - `ModelWizard` gains `Step::Notice` between `Endpoint` and `Loading`, plus per-endpoint acceptance bookkeeping held in the view and delegated to the port.
  - Editing the endpoint after an acceptance invalidates it; the notice must be re-shown for the new endpoint.
  - Declining returns to `Step::Endpoint` with no pending request and no staged state.
- Acceptance criteria:
  - A loopback endpoint goes straight from URL blur to loading with no extra prompt.
  - A non-loopback endpoint shows a warning naming the exact endpoint, the resolved address, that the transport is cleartext HTTP because HTTPS is unsupported for discovery, and whether a credential reference is attached.
  - Accepting submits the request bound to that endpoint. Accepting one endpoint does not authorize another.
  - Editing the URL after acceptance invalidates it and the notice reappears before any request.
  - Declining returns to the endpoint step with zero requests sent and no partial state staged.
  - Typing, paste, resize and window focus produce no requests at any point, including while the notice is displayed.
  - The notice renders correctly at 80x24, 120x40 and 160x50 under no-color and ASCII fallback, with all dynamic text sanitized.
- Actions:
  - [ ] Implement the notice step, the acceptance handoff, and invalidation on endpoint edit.
  - [ ] Add buffer tests for content, accept, decline, invalidation, and the size and fallback matrix.
  - [ ] Extend the wizard PTY harness coverage if the keyboard flow is not already expressible.
  - [ ] Run applicable checks and record actual results.
  - [ ] Present evidence and stop for review before another phase.
- Verification: `cargo test -p fluzo-tui --locked --offline model_wizard` and `python3 -B -m unittest discover -s scripts -p 'test_model_wizard.py'` from the repository root, plus the workspace checks. Buffer snapshots cover layout and sanitization; they do not certify visual quality, which remains a human review gate.
- Risks and recovery: the PTY emulator has known gaps and cannot assert columns. If the new flow cannot be expressed there, the buffer tests carry the assertions and the PTY gap is recorded rather than papered over.

### P3: Documentation of the accepted contract

- Outcome: the documented restart of the discovery contract matches what the operator sees, and the open note in the design baseline points at the closed implementation.
- Owning repositories: `fluzo-labs/fluzo` for `CONFIGURATION.md` and `TUI.md`; `fluzo-labs/fluzo-docs` for the PRD 26.1 amendment note.
- Dependencies: P1 and P2 accepted, so the documented wording describes shipped behavior rather than intent.
- Affected contracts: documentation only.
- Acceptance criteria:
  - `CONFIGURATION.md` describes the endpoint classification, the per-run non-persisted acceptance, and that the acceptance is neither a credential nor repository configuration.
  - `TUI.md` describes the warning step, its content, the accept and decline outcomes, and the invalidation on endpoint edit.
  - The "not yet surfaced" note in the PRD 26.1 amendment is replaced by a pointer to the closed implementation, in a separate `fluzo-docs` change under its own approval.
  - No wording claims a property the implementation does not have, in particular the exact bound of the trust-on-first-use window.
- Actions:
  - [ ] Update `CONFIGURATION.md` and `TUI.md` in this repository.
  - [ ] Prepare the `fluzo-docs` amendment pointer as a separate change and obtain its own approval before publishing.
  - [ ] Run `python3 scripts/check_dev_setup.py` and the documentation-link checks and record results.
  - [ ] Present evidence and stop for review.
- Verification: local link and setup checks from the repository root, plus a read-back of the published `fluzo-docs` revision after that separate change lands.
- Risks and recovery: the cross-repository change cannot be completed from this workspace alone. If `fluzo-docs` approval is withheld, the local documentation still lands and the criterion stays explicitly open rather than silently dropped.

## Progress (local plans only)

| Phase | Implementation | Verification | Review | Evidence |
| --- | --- | --- | --- | --- |
| P1 | implemented | passed | pending | P1 evidence below: 21 focused discovery tests, 254 workspace tests, clean fmt/clippy at the working state fingerprinted 2026-10-09T11:54:20Z |
| P2 | pending | not_run | pending | none yet |
| P3 | pending | not_run | pending | none yet |

Implementation, verification and review are distinct states. A selected commit message is not acceptance. `has_completed_all_phases` stays `false` until every phase is implemented, required verification passes, all reviews are accepted, and global criteria including any required merge are satisfied.

## Next step

Review **P1: Runtime endpoint classification and consent enforcement** at the working state recorded under "Evidence and tracking". Two gates precede any commit:

1. Resolve the D4 deviation. Resolved on 2026-10-09 by user decision: the split was executed. The work now sits on `feat/endpoint-consent-p1`, based on `origin/main` (`31d4fb9`), and `fix/settings-restart-pending-line` / PR #60 are untouched. Recorded for future splits: `CONFIGURATION.md` is modified on both sides, so a plain `git switch -c` is refused (`git switch` only carries local changes for files identical across branches) and that one file has to move as a patch while the others carry normally.
2. Human review acceptance of the P1 diff. `Verification` is `passed`; `Review` stays `pending` until a maintainer accepts it.

Once P1 is accepted, **P2: Wizard cleartext warning step** is the next eligible phase. It depends on the notice P1 produces, and until it lands non-loopback endpoints remain unusable from the wizard.

## Evidence and tracking

- Issue: [fluzo-labs/fluzo#55](https://github.com/fluzo-labs/fluzo/issues/55). GitHub remains the authority for issue state, blockers and assignment; this document is a design snapshot, not a parallel board.
- Unrelated open delivery: PR #60 on `fix/settings-restart-pending-line` (issue #56). Kept separate as decision D4 requires; the split was executed on 2026-10-09.
- Design baseline: fluzo-docs revision `3c7fc8e1f6934922ad66fe3eda8123854620839b`, PRD 26.1 and architecture 10.2.

### P1 evidence (implemented, verification passed, review pending)

- Tested state: uncommitted working tree on `feat/endpoint-consent-p1`, based on `origin/main` at `31d4fb926d722fb0d758512be98c00766a8a7ed2`. Tracked diff SHA-256 `8b03b4522934ef51fca5529b0341b0aaf2f1838b5edd48d4d4e6eadd6f109a14`, captured `2026-10-09T13:55:36Z`. Results belong to that working state, not to HEAD alone.
- D4 deviation: resolved. P1 was first implemented in PR #60's branch working tree, contrary to D4. On 2026-10-09 the user chose the split and it was executed by moving the `CONFIGURATION.md` hunk as a patch onto a new branch from `31d4fb9`. The resulting change set is identical: 777 insertions and 73 deletions across the same six files. `fix/settings-restart-pending-line` and PR #60 were not modified.
- Re-verified after the move at the new state: `cargo fmt --all -- --check` clean, `cargo test -p fluzo-runtime --locked --offline model_discovery` 21 passed, `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` clean.
- Commands run from the repository root at the tested state, all passing:
  - `cargo test -p fluzo-runtime --locked --offline model_discovery` → 21 passed, 0 failed (5.01s)
  - `cargo test -p fluzo-runtime --test http_simulator --locked --offline` → 12 passed, 0 failed
  - `cargo test -p fluzo-tui --locked --offline model_wizard` → 3 passed, 0 failed
  - `cargo fmt --all -- --check` → clean
  - `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` → clean
  - `cargo test --workspace --locked --offline` → 2 / 4 / 8 / 15 / 78 / 12 / 135 passed, 0 failed
  - `python3 scripts/check_dev_setup.py` → "Resolved foundation graph, toolchain, LSP config and 12 skills checked."
  - Same working state, earlier in the session: `cargo check --workspace --all-targets --locked --offline`, `cargo build --workspace --locked --offline`, `cargo run --locked --offline -p fluzo-cli --bin fluzo -- --help`, `cargo run --locked --offline -p fluzo-cli --bin fluzo -- demo`, and `python3 -B -m unittest discover -s scripts -p 'test_model_wizard.py'` (3 tests).
- Acceptance-criterion coverage, by test name in `crates/fluzo-runtime/src/model_discovery.rs::tests`:
  - Non-loopback without acceptance: zero connects → `unaccepted_non_loopback_notices_then_refuses_with_zero_connects`
  - Loopback proceeds with no added step → `loopback_probe_and_submit_need_no_acceptance`
  - Notice names endpoint, resolved addresses, class and credential attachment, never the credential value → `probe_reports_a_notice_and_never_connects`, `declined_attempt_carries_neither_endpoint_nor_credential_reference`
  - Acceptance bound to the exact endpoint → `acceptance_is_bound_to_the_exact_endpoint_string`, `acceptance_never_crosses_a_service_instance`
  - Class change aborts before any request → `class_change_after_acceptance_aborts_before_any_request`
  - Any public address classifies the endpoint public → `classify_is_conservative_across_address_ranges`
  - Unresolvable host fails closed, no hang → `unresolved_host_fails_closed_and_cannot_be_accepted`, `unprobed_unresolvable_submit_fails_without_hanging_or_connecting`
  - Protocol bump and control-path behavior → `protocol_two_is_rejected_after_the_bump`, `probe_validates_before_touching_the_worker`, `cancel_reaches_a_pending_probe`
- Protocol bump record: `PROTOCOL` 2 → 3. Existing discovery tests referenced the constant rather than a literal, so none needed editing; the deliberate record of the bump is `protocol_two_is_rejected_after_the_bump`. The TUI test `Port` gained `probe` and `accept` because the widened port intentionally has no default implementations.
- Documentation touched beyond the P1 action list, because the bump made existing prose factually wrong: `AGENTS.md` (discovery protocol version and the locality sentence), `APPLICATION.md` (protocol version, `Status` ownership, rejection of protocols 1 and 2), `CONFIGURATION.md` (the "address class is not restricted" claim). `CONFIGURATION.md:795` was left as-is; it remains true.
- Limitations:
  - The zero-connect claim is asserted with an in-process scripted transport that records connect attempts and never creates a socket, which is stronger for this claim than a remote listener that would reject an unexpected request. The SIM-01 fixtures still cover the real HTTP protocol paths and are unchanged (12 tests).
  - "Nothing written to `.fluzo`" holds by construction: the discovery module contains no filesystem API, and acceptance lives in `Arc<Mutex<Consent>>` in process memory. No test asserts a file is untouched because no file is opened.
  - The interim usability risk is live: until P2 lands, a non-loopback endpoint cannot be added from the wizard. The TUI renders the first `Status::Notice` through its existing catch-all branch and a retry shows `ConsentRequired`; no crash and no unconsented connect.
  - The trust-on-first-use window is bounded to a same-class swap between the probe and the connect, not eliminated.
  - The full `python3 -B -m unittest discover -s scripts -p 'test_*.py'` suite was not re-run at this state; only the wizard subset was.
- Approval reference: the plan's approval annotation covers content and destination of the plan, not acceptance of this implementation. `Review` remains `pending`.

## Optional handoff to another skill

For an authorized execution of a phase, transmit as data: the plan path and revision, the selected phase, the baseline `31d4fb9`, the scope and exclusions above, the affected contracts, the acceptance criteria, the verification commands, and the approval reference with its exact scope. The executing step must revalidate the issue state, the baseline and the branch before editing. This plan's Approval field is an annotation and grants nothing by itself.
