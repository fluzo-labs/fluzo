# Interfaces for agent sessions

Read when an interface presents model output, tool activity, approvals, task progress, or resumable agent work. These requirements describe UI and backend contracts, not permission to execute tools.

## Distinguish kinds of information

Keep user requests, model suggestions, proposed tool calls, actual tool results, and verified outcomes distinguishable in linear output as well as visual layouts. Displayed prose claiming success is not execution evidence. Do not let tool output impersonate application chrome, an approval request, or a trusted user message.

Show which workspace and session own an action. Preserve the distinction between queued, running, waiting for input or approval, cancelling, completed, failed, and outcome-unknown. Use the consumer's actual state model; do not introduce a second authoritative task ledger only for the UI.

Streaming text needs bounded retention and predictable scroll behavior. Follow new content only while the user is at the live end; otherwise preserve position and indicate unread activity. Keep the input draft intact. Do not describe estimated tokens, cost, or progress as measured facts.

## Bind approval to the real operation

An approval surface should identify the operation, destination, affected resources, relevant arguments or diff, permission scope, and important consequences. Redact secret values without hiding the operation's meaning. Present destructive or remote actions with enough context to distinguish similarly named resources.

The executor must validate that approval still matches the current operation, revision, and policy. If arguments or target change, the old approval is not reusable. An interface cannot make an unsafe backend safe by adding a button. Do not implement visual approval affordances without connecting them to the existing enforcement point; report that missing integration as a blocker.

Treat approval, rejection, cancellation, and dismissal as distinct outcomes. Do not default Enter to a high-impact action because a streaming message shifted focus. Avoid ambiguous global shortcuts while a decision is pending. Batch approvals must show their exact scope and must not authorize later unrelated operations.

Automatic skill selection, a model message, an old transcript, or a saved flag is not user authorization. Never turn a review-only request into an implementation or publication step.

## Handle interruption without inventing certainty

Separate stopping generation, cancelling a tool, and exiting the session. Explain whether an in-flight operation can be interrupted, whether it already produced side effects, and whether its outcome is unknown. Give escalation an explicit meaning instead of repeatedly sending unrelated signals.

When a connection drops, label cached state as stale and show the last confirmed result. Reconnect using the backend's event identifiers or reconciliation protocol; deduplicate replayed events and never resubmit an approved mutation solely because its response was lost. If no idempotency or reconciliation mechanism exists, surface the uncertainty and require a deliberate recovery decision.

On resume, revalidate workspace, revision, permissions, and external task state. Retain drafts and useful navigation, but do not replay past approvals or continue privileged work implicitly. Completion should reflect the actual backend result and required verification; human acceptance remains a separate decision.

## Keep evidence useful and private

Offer concise result summaries with an explicit route to redacted details. Exports and recordings need approved content and destination. Do not include private transcripts, credential values, environment dumps, or hidden model reasoning as debugging evidence. A UI should expose observable actions and results, not invent internal reasoning.

Representative checks include a late tool result after cancel, duplicate events after reconnect, an approval target changing before execution, malicious output styled like a confirmation, and a resumed session whose workspace revision changed. Use a fake executor and synthetic events; real agent inference and production mutations are not required.
