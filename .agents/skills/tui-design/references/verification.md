# Verification and review evidence

Read before implementation verification or a final design review. Use the consumer's documented commands from its project root; this skill does not prescribe a nonexistent application test suite.

## Evidence contract

For each relevant criterion, record the source revision or reviewed diff, environment, input, observed result, and limitation. Use `passed`, `failed`, `not run`, or `blocked` truthfully. Proposed behavior and textual review are not execution results. An old screenshot does not prove a new revision works.

Start with state and formatting tests, then deterministic render checks, then a small number of PTY paths for boundaries that require a terminal. Reuse existing test libraries; do not install tools without authorization. Freeze or inject clock, randomness, external responses, dimensions, and color policy where the framework allows it.

## Representative task

Use a disposable project or existing authorized test fixture to present a read-only task inspector backed by synthetic events. It has a list, task details, a filter field, and a plain-output path. In the agent-session variant, add a fake executor and an approval state without real tool execution.

Verify selecting a stable task, refreshing while filtering, narrowing the terminal, receiving an error, retrying a read, cancelling, and exiting. Record the actual implementation and framework version. A prose walkthrough alone is a design review, not a representative execution test.

## Scenario matrix

| Scenario | Observable expectation | Suggested evidence |
| --- | --- | --- |
| Review-only request | Findings without file or remote writes | Before/after workspace and operation record |
| Empty or loading list | Distinct states and usable exit/help | State test and fixed-size frame |
| Refresh reorders rows | Same selected task; input and focus preserved | Synthetic refresh test |
| Late response after cancel | No overwrite of newer state or false success | Event-order test |
| 80 by 24, 60 columns, minimum size | Task remains usable or explicit too-small state | Fixed-size frames and resize path |
| Long, wide, or combining text | Valid truncation and bounded layout | Width cases and render assertions |
| Untrusted terminal or markup controls | Data cannot alter chrome, clipboard, or terminal modes | Sanitizer tests with synthetic bytes |
| Piped output and missing TTY | Clean machine results; no blocking prompt | Captured streams and exit status |
| No color, monochrome, ASCII option | Status and controls remain meaningful | Frames plus manual review |
| Text field and global shortcuts | Typed content preserved; contextual controls work | Input tests |
| Normal exit, interrupt, error, partial setup | Expected cleanup attempted; original failures retained | Lifecycle fault injection and scoped PTY test |
| External editor changes data | Safe handoff, reload, redraw, and error preservation | Fake child plus PTY test where needed |
| Growing stream and slow consumer | Bounded retention and responsive input | Controlled load and cancellation test |
| Approval arguments change | Backend rejects obsolete approval | Fake-executor contract test |
| Reconnect replays an event | No duplicate mutation or false completion | Replay and reconciliation test |
| Secret in a result or export | Redacted before retention and export | Synthetic secret fixture |
| Missing framework or recording tool | Explicit limitation; no unsolicited installation | Operation record |

Do not synthesize real clipboard writes, production commands, live model requests, or remote mutations just to exercise these scenarios. For capability checks that would affect the user's terminal, require a suitable disposable environment and explicit scope.

## Review format

Order findings by user impact: unsafe action or disclosure, unusable interaction or broken output contract, misleading state, then visual friction. Each finding should contain the observation, file and line when available, reproduction or evidence, consequence, and a specific correction. Label assumptions and unverified API claims.

Finish with the checks actually run, unresolved risks, and the smallest next decision. Do not turn the review into an unrequested rewrite. A clean static check cannot certify agent authorization behavior, prompt-injection resistance, accessibility, or every terminal combination.

## Maintaining this skill

Check frontmatter and folder-name agreement, local reference links, absence of runtime dependencies on sibling skills, license inclusion, UTF-8/LF, and removal of drafting placeholders. Copy the complete folder outside the collection and verify that its references still resolve. Review licensing before incorporating any external material.

For behavioral changes, use the representative task in an authorized disposable consumer and record actual outcomes. Until that execution has happened, report the skill as instruction-reviewed and statically checked, not behaviorally validated. Keep raw private transcripts and credentials out of the collection.
