---
name: "2026-10-09-endpoint-consent"
description: "Make model discovery refuse to send a cleartext HTTP catalog request to a non-loopback endpoint until the operator explicitly accepts that exact endpoint, with the resolved address class shown before the first byte leaves the process."
created_at: "2026-10-09T08:19:50Z"
last_implementation_at: "2026-10-14T09:14:39Z"
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
  - [x] Implement the notice step, the acceptance handoff, and invalidation on endpoint edit.
  - [x] Add buffer tests for content, accept, decline, invalidation, and the size and fallback matrix.
  - [x] Extend the wizard PTY harness coverage if the keyboard flow is not already expressible.
  - [x] Run applicable checks and record actual results.
  - [x] Present evidence and stop for review before another phase.
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
  - [x] Update `CONFIGURATION.md` and `TUI.md` in this repository.
  - [ ] Prepare the `fluzo-docs` amendment pointer as a separate change and obtain its own approval before publishing. **Out of scope for this change:** `fluzo-labs/fluzo-docs` is not cloned on this machine and the amendment needs its own PR and its own approval there. Recorded as pending, not dropped.
  - [x] Run `python3 scripts/check_dev_setup.py` and the documentation-link checks and record results.
  - [x] Present evidence and stop for review.
- Verification: local link and setup checks from the repository root, plus a read-back of the published `fluzo-docs` revision after that separate change lands.
- Risks and recovery: the cross-repository change cannot be completed from this workspace alone. If `fluzo-docs` approval is withheld, the local documentation still lands and the criterion stays explicitly open rather than silently dropped.

## Progress (local plans only)

| Phase | Implementation | Verification | Review | Evidence |
| --- | --- | --- | --- | --- |
| P1 | implemented | passed | accepted | Merged into `main` as `66047d96058e5209910698994f1b233e52fb0890` (PR #61); see the P1 evidence section for the tested state and acceptance reference |
| P2 | implemented | passed | accepted | Accepted by the user on 2026-10-14 from the explicit directive "acepto P2", recorded in the acceptance note below; not a self-approval. P2 evidence below: 11 wizard tests (8 new), 262 workspace tests, 3 wizard PTY tests, 73 script tests, clean fmt/clippy at the working state fingerprinted `db23d73afa9044f2be005a8528493e0029e94cb839fa4ef331107865c93cf9d8`, captured 2026-10-09T15:48:44Z. Landed locally as `7ddca6a` on `feat/endpoint-consent-p2`; merge into `main` still pending review of P3. |
| P3 | implemented | passed | pending | Documentation of the accepted contract landed in this repository: `TUI.md` gains the "Model wizard cleartext warning step" section, and `CONFIGURATION.md`, `APPLICATION.md`, `AGENTS.md` and the CLI help text were completed where the accepted behavior was understated. Verification: `cargo fmt --all -- --check` clean, `cargo test -p fluzo-tui --locked --offline model_wizard` 11 passed, `python3 scripts/check_dev_setup.py` OK, ad-hoc relative-link scan 26 links / none broken. The `fluzo-docs` PRD 26.1 pointer stays open and outside this change. See the P3 evidence section; `Review` is a human gate and has not been given. |

Implementation, verification and review are distinct states. A selected commit message is not acceptance. `has_completed_all_phases` stays `false` until every phase is implemented, required verification passes, all reviews are accepted, and global criteria including any required merge are satisfied.

P2 review acceptance is user-supplied, recorded 2026-10-14 from the explicit
directive "acepto P2", given after the P2 evidence had been presented and after
the current state was re-checked the same day: `cargo fmt --all -- --check` clean,
`cargo test -p fluzo-tui --locked --offline model_wizard` 11 passed,
`python3 -B -m unittest discover -s scripts -p 'test_model_wizard.py'` 3 tests
OK, `cargo test --workspace --locked --offline` 262 passed, clippy clean with
`-D warnings`, and `python3 scripts/check_dev_setup.py` OK. The reviewer is the
user, not the agent: this is a record of a human decision, not a self-approval,
and it covers P2 only. P3 still stops for its own review.

## Next step

P2 is closed out: implemented, verified, and accepted by the user on 2026-10-14.
It is committed as `7ddca6a` on `feat/endpoint-consent-p2` and is not yet merged
into `main`.

**P3: Documentation of the accepted contract** is the active phase. It is
implemented and verified in this repository (see the P3 evidence section), and
its `Review` stays `pending` until a maintainer accepts it.

One P3 criterion remains open by design and is deliberately outside this change:
replacing the "not yet surfaced" note in the PRD 26.1 amendment with a pointer
to the closed implementation. That text lives in `fluzo-labs/fluzo-docs`, which
is not cloned on this machine and needs its own PR and its own approval there.
For that reason `has_completed_all_phases` stays `false`, and the pull request for
P3 references #55 without closing it.

## Evidence and tracking

- Issue: [fluzo-labs/fluzo#55](https://github.com/fluzo-labs/fluzo/issues/55). GitHub remains the authority for issue state, blockers and assignment; this document is a design snapshot, not a parallel board.
- Unrelated delivery, now closed: PR #60 on `fix/settings-restart-pending-line` (issue #56) merged as `881630cb4f08b73f42ac10579fdc16e451b1a311` on 2026-10-09, with issue #56 closed as completed. It was kept separate from this plan as decision D4 requires.
- Design baseline: fluzo-docs revision `3c7fc8e1f6934922ad66fe3eda8123854620839b`, PRD 26.1 and architecture 10.2.

### P1 evidence (implemented, verification passed, review accepted)

- Acceptance reference: accepted and merged into `main` as `66047d96058e5209910698994f1b233e52fb0890` (PR #61, squash merge, commit date `2026-10-09 16:39:55 +0200`). The merge SHA differs from the tested local commit `1233d9c`; the merged content was verified identical to that tested commit before merging.

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

### P2 evidence (implemented, verification passed, review accepted)

- Tested state: uncommitted working tree on `feat/endpoint-consent-p2`, based on `origin/main` at `66047d96058e5209910698994f1b233e52fb0890`, the merged P1. Tracked diff SHA-256 `db23d73afa9044f2be005a8528493e0029e94cb839fa4ef331107865c93cf9d8`, captured `2026-10-09T15:48:44Z`: 532 insertions and 45 deletions across `crates/fluzo-tui/src/model_wizard.rs` and `scripts/test_model_wizard.py`. Results belong to that working state, not to HEAD.
- Implementation shape: `Step::Notice` sits between `Endpoint` and `Loading`. `query()` now submits a `Probe` instead of a discovery `Request`, so the warning is reached while no socket exists; the catalog request is submitted only by `submit_discovery()`, reached after a loopback classification or an explicit acceptance. `pending` became `Option<Pending>` (`Probe` / `Discover`) because a pending probe and a pending discovery request must be cancellable distinctly and `poll` must tell a notice from a result.
- Acceptance bookkeeping: the view holds `accepted: Option<(String, AddressClass)>` and delegates the authoritative record to the port. A match requires both the exact endpoint string and the same class, so a DNS change between the probe and a later attempt re-warns instead of silently connecting. Acceptance is invalidated at every endpoint edit site (`paste`, `Ctrl+U`, `Backspace`) rather than by value comparison alone, so edit-and-revert to an already-accepted value still re-shows the notice.
- Decline path: `Esc` on the notice clears it, returns to `Step::Endpoint` and stages nothing. The pending job is already resolved when the notice is shown, so no cancel is issued and no request follows.
- Inertness: on `Step::Notice` only `Enter`/`Tab` (accept) and `Esc` (decline) act. Characters, `Backspace`, arrows, `Space`, `Ctrl+U` and a mouse click change no state and produce no probe or request.
- Rendering: the notice body is five lines (cleartext statement, endpoint, resolved addresses, class with credential reference, scope-of-acceptance note), sized to fit the six-row body at 60x16. Every dynamic value passes through `safe_text`, and the resolved-address list is truncated with an explicit `+N more` count rather than dropped. The title stays inside step 4 as `4 / Cleartext warning` so the wizard's numbering scheme is preserved.
- Commands run from the repository root at the tested state, all passing:
  - `cargo test -p fluzo-tui --locked --offline model_wizard` → 11 passed, 0 failed
  - `python3 -B -m unittest discover -s scripts -p 'test_model_wizard.py'` → 3 tests OK
  - `cargo fmt --all -- --check` → clean
  - `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` → clean
  - `cargo test --workspace --locked --offline` → 2 / 4 / 8 / 15 / 78 / 12 / 143 passed, 0 failed (262 total, 8 more than P1's 254)
  - `python3 -B -m unittest discover -s scripts -p 'test_*.py'` → 73 tests OK (100.3s)
  - `cargo build --workspace --locked --offline` → Finished
  - `python3 scripts/check_dev_setup.py` → "Resolved foundation graph, toolchain, LSP config and 12 skills checked."
- Acceptance-criterion coverage, by test name in `crates/fluzo-tui/src/model_wizard.rs::tests`:
  - Loopback goes straight from URL blur to loading, no extra prompt → `loopback_blur_probes_then_discovers_without_a_notice`
  - Non-loopback warns before any request; accept submits that exact endpoint → `non_loopback_warns_before_any_request_then_accept_submits_that_endpoint`
  - Warning names endpoint, resolved addresses, cleartext transport and credential attachment → `notice_names_endpoint_addresses_transport_and_credential` (80x24, 120x40 and 160x50, color off and on)
  - Acceptance of one endpoint does not authorize another, and editing after acceptance invalidates it → `editing_the_endpoint_after_acceptance_invalidates_it` (clear, re-type, and A to B to A all re-warn; the submitted request count stays at one)
  - A class change at connect time re-warns instead of connecting → `a_class_change_at_connect_time_rewarns_instead_of_connecting`
  - Decline sends nothing and stages no state → `declining_the_notice_sends_nothing_and_stages_no_state`
  - The notice is inert to every other key and to the mouse → `the_notice_is_inert_except_accept_and_decline`
  - Control and bidi text is sanitized in the notice → `notice_sanitizes_control_and_bidi_text`
  - A pending probe and a pending discovery are both fenced by cancel → `cancel_fences_a_pending_probe_and_a_pending_discovery`
  - Shared dialog background and control hints survive the new step → `shared_dialogs_keep_background_and_controls_at_all_sizes`
- PTY coverage added: `scripts/test_model_wizard.py` now asserts `4 / Cleartext warning` never appears in a run. That harness uses a real `127.0.0.1` listener, so it proves end to end that the loopback path shows nothing new. The keyboard flow needed no other harness extension; the existing one already routes `Tab`, `Enter` and `Esc`.
- Deliberate test replacement: the pre-existing `url_blur_queries_once_and_cancel_fences_late_results` was replaced by the loopback and cancel tests. `Tab` now probes before it discovers, so request ids shifted and the old single-request assertion no longer described the flow.
- Limitations:
  - Buffer snapshots cover layout and sanitization; they do not certify visual quality, which stays a human review gate.
  - The non-loopback accept and decline flow is asserted by buffer tests only. The deterministic `scripts/` harness has no non-loopback fixture, so no PTY run exercises the warning screen.
  - "Resize and window focus produce no requests while the notice is displayed" holds structurally: resize and focus events never reach `query()` or `submit_discovery()`. The size matrix re-renders stored state at several sizes; no test drives a resize event against a displayed notice.
  - `crates/fluzo-cli/src/main.rs:102` still describes the URL step as "Leaving URL fetches the catalog". Documentation prose is owned by P3, so it is flagged here rather than changed in this phase.
  - The trust-on-first-use bound recorded in P1 is unchanged: re-resolving at connect bounds the window to a same-class swap, it does not eliminate it.
- Approval reference: the plan's approval annotation covers the plan's content and destination, not acceptance of this implementation. `Review` remains `pending`.

### P3 evidence (implemented, verification passed, review pending)

- Tested state: working tree on `feat/endpoint-consent-p2` at HEAD `7ddca6a`, the accepted P2 commit, on top of `main` at `66047d96058e5209910698994f1b233e52fb0890` (P1). The change is documentation prose plus one CLI help string; no product behavior was touched.
- `TUI.md` gains "Model wizard cleartext warning step", documenting that the step is reached before any part of the catalog request leaves the process (the resolver's own name query is named as the only traffic performed to that point), that it names the exact endpoint, the resolved addresses and the address class, that the transport is cleartext HTTP because HTTPS is unsupported for discovery, that it reports whether a credential reference is attached and never its value, that Enter/Tab accept that endpoint for the current run only while Esc sends nothing, and that editing the URL invalidates a prior acceptance. The trust-on-first-use bound is stated as a same-class swap, not as a closed gap.
- `CONFIGURATION.md` now states what the acceptance is not: per run, held in process memory, neither a credential nor repository configuration, never written to `.fluzo`, not reused by a later process, and granting nothing beyond the catalog query it authorizes. Its URL-step prose was corrected from "queries once" to resolve-then-maybe-warn.
- `APPLICATION.md` now describes the URL step as probe-first, with the catalog GET following only for a loopback endpoint or after the operator accepts the notice. `AGENTS.md` names the cleartext warning between the URL step and model selection.
- `crates/fluzo-cli/src/main.rs:102` help text was corrected from "Leaving URL fetches the catalog" to the probe-then-warn behavior. This was the prose flagged in the P2 limitations as P3-owned; `fluzo --help` output was read back to confirm the new wording reaches users.
- Commands run from the repository root at this state, all passing:
  - `cargo fmt --all -- --check` → clean
  - `cargo test -p fluzo-tui --locked --offline model_wizard` → 11 passed, 0 failed
  - `cargo test -p fluzo-cli --locked --offline` → 8 passed, 0 failed (run because the CLI help string changed)
  - `cargo test --workspace --locked --offline` → 2 / 4 / 8 / 15 / 78 / 12 / 143 passed, 0 failed (262 total, unchanged from P2)
  - `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` → clean
  - `python3 -B -m unittest discover -s scripts -p 'test_model_wizard.py'` → 3 tests OK
  - `python3 scripts/check_dev_setup.py` → "Resolved foundation graph, toolchain, LSP config and 12 skills checked."
- Documentation-link check: no link checker exists in `scripts/`. An ad-hoc scan of every relative Markdown link in the repository-root documents was run instead: 26 relative links, none broken. It is not a standing check and is not wired into CI.
- Not changed, recorded deliberately:
  - The `fluzo-docs` PRD 26.1 pointer stays open and out of scope: different repository, not cloned here, its own approval required.
  - `crates/fluzo-tui/src/model_wizard.rs:361` still reads "Leaving URL with Tab/Enter sends this Authorization header to that endpoint." Strictly the header travels with the discovery request, not the probe. It is accepted P2 product copy, so it is flagged here rather than rewritten inside a documentation change.
- Limitations: this is documentation evidence. No test asserts the prose, and nothing here certifies the notice's visual quality, which remains the human gate recorded in P2.
- Approval reference: the plan's approval annotation covers the plan's content and destination, not acceptance of this phase. `Review` remains `pending`.

## Optional handoff to another skill

For an authorized execution of a phase, transmit as data: the plan path and revision, the selected phase, the baseline `31d4fb9`, the scope and exclusions above, the affected contracts, the acceptance criteria, the verification commands, and the approval reference with its exact scope. The executing step must revalidate the issue state, the baseline and the branch before editing. This plan's Approval field is an annotation and grants nothing by itself.
