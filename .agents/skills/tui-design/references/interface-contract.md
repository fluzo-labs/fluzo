# Interaction and presentation contract

Read this reference when designing a new terminal flow or reviewing an existing one. Start from observable behavior rather than a component inventory.

## Choose the smallest useful interaction

| Need | Appropriate starting point | Contract to preserve |
| --- | --- | --- |
| Produce a result and exit | Plain CLI | Parseable output, meaningful exit status, no unsolicited prompt |
| Ask for a small decision | Inline interaction | Shell context remains visible; cancellation is distinguishable from success |
| Search or select from data | Bounded picker, with a preview only if useful | Selected value is separate from interface output |
| Monitor or coordinate ongoing work | Persistent interface | Stable focus, explicit freshness, recovery, and terminal restoration |

Do not introduce full-screen mode for a simple command merely because a framework supports it. Conversely, do not squeeze a persistent multi-step workspace into a chain of prompts that hides task context.

Write down who consumes stdout. Route machine results there without spinners, ANSI styling, or status messages. Diagnostics normally use stderr. When an interactive picker also emits a result, verify an independent usable interaction channel rather than assuming stdout is a terminal. If required interaction is unavailable, explain the existing non-interactive alternative and exit predictably; never hang in CI.

## Define actions and states

For each essential action, record its trigger, preconditions, visible feedback, success condition, failure recovery, and cancellation semantics. A spinner is not a success condition. Avoid treating an empty result as a network error or a stale cached result as fresh data.

Use stable identifiers for selection. When items move, retain the same selected object where possible, not the same row number. If it disappears, choose and announce a deterministic nearby fallback. Preserve typed input during refresh. Do not steal focus when a notification, asynchronous result, or new item arrives.

Maintain a single keyboard focus owner. Local bindings take precedence only within their documented context. Character shortcuts must not intercept text entry; arrow keys, Tab, Enter, and Escape should have clear contextual meanings. Expose a keyboard route to every operation. Mouse input is optional assistance, not an exclusive path.

Show relevant shortcuts near the current task and provide contextual help when the action set grows. A modal must have an obvious completion and cancellation route and restore focus to a valid control on dismissal. Explain unsaved-work handling before discarding input. Preserve interrupt and suspend behavior where supported, distinguishing cancellation of a task from termination of the application.

## Make layout degradation explicit

Evaluate at the consumer's normal size, 80 by 24 cells, 60 columns, and a declared minimum height and width. These are review cases, not mandatory fixed breakpoints. Derive actual transitions from required content and controls.

For each size, specify what remains visible, moves to a detail view, truncates, or disappears. Keep the primary task and recovery controls reachable. Multi-pane layouts should offer a useful single-pane path. At an unusable size, show an honest minimum-size state with a way to exit; do not calculate negative or overflowing rectangles.

Count space spent on navigation, borders, labels, and repeated metadata. Remove redundant enclosure and repeated row markers that communicate no distinction. Preserve labels or symbols that make status understandable without color. Do not remove useful accessibility redundancy merely to reduce visual noise.

## Render meaning consistently

Use semantic styles for focus, selection, warning, failure, muted context, and actionable controls. Integrate with the existing theme rather than adding an unrelated palette. Keep focus distinct from selection and severity.

Honor non-empty `NO_COLOR` in automatic mode, provide a documented explicit color preference, and define conflicting-option precedence. Check monochrome and limited-color output; do not convey a result only through hue. Provide text alternatives to icons and a documented ASCII option where required. Do not assume a patched font is installed or reliably detectable.

Measure terminal cells using the framework's width-aware tools after handling untrusted control sequences. Test CJK, combining marks, emoji clusters, tabs, and multiline values. Truncation must preserve valid text and styling boundaries. Long paths and identifiers need a detail or copy route, not an ambiguous truncated value used for a destructive decision.

For long collections, bound rendering and retained data. Prefer viewport-based rendering or paging according to the framework. Keep column roles stable during streaming updates, and expose full values separately when a table cannot fit them. Report partial data and dropped-history limits explicitly.

## Accessible and scriptable alternatives

Where users or automation need linear interaction, provide the consumer's existing plain mode or propose one as an explicit contract change. Use ordered text, useful error messages, and structured output when appropriate. Never claim that keyboard support or plain mode alone proves screen-reader accessibility; record the assistive technology and terminal combinations actually checked.
