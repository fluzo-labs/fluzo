# Terminal ownership and untrusted data

Read before implementing terminal setup, external content rendering, clipboard operations, subprocesses, or cleanup. A terminal is an active interpreter, not a passive text surface.

## Restrict what displayed data can control

Separate trusted application styling from untrusted text. Filenames, branch names, logs, remote messages, tool output, and model responses may contain control bytes, escape sequences, bidirectional controls, or framework markup. Use the framework's documented plain-text or escaping path and a reviewed terminal-control sanitizer at the display boundary. Do not concatenate arbitrary strings into ANSI, OSC, markup, or shell commands.

Allow only formatting the application intentionally supports. Handle ESC, C0/C1 controls, carriage returns, backspaces, and line breaks according to the field's contract. A single-line label must not gain extra rows or overwrite an approval prompt. Preserve a clearly labeled escaped representation when exact bytes matter. Unicode width calculation and stripping color alone do not provide this protection.

Validate hyperlink schemes and encode or reject terminators before generating terminal hyperlinks. Visible text must not imply a different trusted destination. Do not automatically open links emitted by tools or models. In remote sessions, explain whether an action affects the remote host or the user's local machine.

Clipboard writes can replace valuable content or prepare a dangerous later paste. Require a deliberate copy action, use the reviewed exact value, and report best-effort failure. Never issue automatic clipboard reads or hidden writes. Do not automatically enable emulator or multiplexer passthrough to make clipboard or notification features work.

## Own and restore only the modes you change

Identify which component owns raw mode, screen buffers, cursor visibility, mouse reporting, and paste modes. Use framework-managed lifecycle where available. Avoid installing competing signal handlers or bypassing existing cleanup.

Prepare cleanup for partial initialization as well as normal exit. On setup or teardown failure, attempt remaining safe restoration steps independently and preserve the original error along with cleanup failures. Avoid an early return that leaves a successfully enabled mode active. Uncatchable termination or process failure may prevent cleanup; document recovery rather than promising every exit can be handled.

Check normal shutdown, interrupt, returned error, panic or exception, resize, and supported suspend/resume. Do not assume POSIX job control on Windows. Diagnostic output must not corrupt the active interface or spill secrets into a debug file; prefer existing bounded, redacted logging with appropriate permissions.

## Temporarily transfer terminal ownership

For an editor or interactive child process:

1. Authorize the command and its file scope independently of UI implementation.
2. Stop conflicting input readers and background screen writes.
3. Use the installed framework's supported temporary handoff mechanism.
4. Launch with an argument vector, not a shell string containing untrusted paths or model text. Interpret editor configuration through the consumer's existing trusted configuration mechanism.
5. Wait for completion or record interruption; preserve child exit and re-entry errors independently.
6. Restore input ownership and terminal modes, reload externally mutable data, reconcile selection, and repaint.

Repainting does not reload files. Final application shutdown is not a substitute for temporary handoff. If the framework or platform lacks safe handoff support, provide an explicit exit-and-resume flow instead of improvising raw escape sequences.

## Keep effects bounded and cancellable

Do not perform blocking disk, network, or child-process work on the input or rendering path. Use the framework's supported effect or worker model. Associate results with a request or session identifier so a late response cannot overwrite newer state.

Bound concurrency, retained logs, output queue size, and work per frame. Define timeouts where appropriate and propagate cancellation to the actual operation, not just its spinner. A cancelled task may have completed an external mutation before acknowledgement; reconcile its outcome before offering retry. Retries must not duplicate writes.

Redact secrets before retention or export, not only at final display. Create any authorized credential file with restrictive access from the outset, respecting platform ACLs; fixing permissions after writing leaves an exposure window. Do not fall back silently from secure storage to plaintext.

## Defensive fixtures

Use synthetic values, not live attacks or real secrets: literal representations of ESC and OSC, a name containing newline or carriage return, markup delimiters, a very long line, a misleading link label, a mock secret, and a stale result arriving after cancellation. Assert that they cannot move the cursor, change a prompt, set the clipboard, launch a command, or contaminate machine output.
