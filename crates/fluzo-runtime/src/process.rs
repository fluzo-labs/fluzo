//! Owned process lifecycle.
//!
//! A tool request names an executable and its arguments. This module runs it in
//! a process group whose ownership is established at spawn and re-verified
//! before any signal is sent, with closed stdin and an environment that is
//! built rather than inherited.
//!
//! Three properties carry the design:
//!
//! * **Signals go to a verified group, never to a bare pid and never to a
//!   group supplied by a caller.** The group used for cancellation is read back
//!   from the child we actually spawned. A caller cannot ask this module to kill
//!   a process group it did not create, and cannot aim it at group `0` or at
//!   the runtime's own group.
//! * **The environment is built, not filtered.** Starting from the parent
//!   environment and removing secrets is a denylist, and a denylist misses the
//!   variable nobody thought of. [`EnvironmentSpec::baseline`] starts from an
//!   explicit allowlist and adds only authorized injections.
//! * **Bounded capture keeps draining after the cap.** Stopping reads at the
//!   limit deadlocks the child on a full pipe. Past the limit the reader keeps
//!   draining and discarding, and records that it truncated.
//!
//! ## Known limitation: controlling-terminal detachment
//!
//! PRD 21.3 requires that a child not inherit the TUI's controlling terminal.
//! This module closes stdin and passes no PTY, so the child receives no
//! terminal on its standard descriptors and terminal-generated signals do not
//! reach it, because it sits in its own process group. Full session detachment
//! requires `setsid()` executed in the child between `fork` and `exec`.
//! `std::process` exposes no post-fork hook that is safe, and the workspace
//! forbids `unsafe` code, so the child remains in the runtime's login session
//! and could in principle open `/dev/tty` itself. That residual is documented
//! rather than claimed away; see `WORKSPACE.md`.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, getpgid, getpgrp, kill_process, kill_process_group};

/// Default per-stream capture ceiling.
pub const DEFAULT_CAPTURE_LIMIT: usize = 256 * 1024;
/// Default wait between `SIGTERM` and escalation to `SIGKILL`.
pub const DEFAULT_GRACE_PERIOD: Duration = Duration::from_secs(5);
/// Default wait after escalation before the outcome is declared uncertain.
pub const DEFAULT_ESCALATION_WINDOW: Duration = Duration::from_secs(5);

/// Environment variables copied from the runtime's own environment when
/// present. Everything else is absent unless explicitly injected.
///
/// These are locale, home and path settings with no credential content. The list
/// is the contract: a variable not named here does not reach a child.
const BASELINE_KEYS: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
    "TZ",
    "TMPDIR",
    "TERM",
];

/// Environment variables that may never be injected through a tool request.
///
/// Two classes live here. The first is startup and execution injection:
/// variables a program reads before it runs anything else, so setting them
/// redirects execution without naming a program. The second is credential and
/// session channels that would hand a child an identity the operator never
/// approved.
const FORBIDDEN_INJECTIONS: &[&str] = &[
    "BASH_ENV",
    "ENV",
    "PROMPT_COMMAND",
    "CDPATH",
    "IFS",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "NODE_OPTIONS",
    "RUBYOPT",
    "PERL5OPT",
    "PYTHONSTARTUP",
    "GIT_SSH",
    "GIT_SSH_COMMAND",
    "GIT_ASKPASS",
    "SSH_ASKPASS",
    "SSH_AUTH_SOCK",
    "GPG_AGENT_INFO",
    "DBUS_SESSION_BUS_ADDRESS",
];

/// An environment constructed rather than inherited.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnvironmentSpec {
    values: BTreeMap<String, String>,
}

impl EnvironmentSpec {
    /// Builds the documented baseline from the current process environment.
    ///
    /// Only [`BASELINE_KEYS`] are read. A variable outside the list cannot leak
    /// into a child no matter what the runtime happens to have set.
    #[must_use]
    pub fn baseline() -> Self {
        let mut values = BTreeMap::new();
        for key in BASELINE_KEYS {
            if let Ok(value) = std::env::var(key) {
                values.insert((*key).to_string(), value);
            }
        }
        Self { values }
    }

    /// Builds a baseline that ignores the ambient environment entirely.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Adds one authorized variable.
    ///
    /// # Errors
    ///
    /// Returns [`EnvironmentError::ForbiddenKey`] for a startup-injection or
    /// credential-channel name.
    pub fn inject(&mut self, key: &str, value: &str) -> Result<(), EnvironmentError> {
        if is_forbidden_injection(key) {
            return Err(EnvironmentError::ForbiddenKey {
                key: key.to_string(),
            });
        }
        self.values.insert(key.to_string(), value.to_string());
        Ok(())
    }

    /// Returns the variable set the child will receive.
    #[must_use]
    pub fn entries(&self) -> &BTreeMap<String, String> {
        &self.values
    }

    /// The entries beyond the documented baseline.
    ///
    /// These are the explicit overrides a caller supplied, and therefore the
    /// part of the environment an approval has to cover. The baseline is the
    /// runtime's own doing and carries no operator-visible override.
    #[must_use]
    pub fn overrides(&self) -> BTreeMap<String, String> {
        self.values
            .iter()
            .filter(|(key, _)| {
                !BASELINE_KEYS
                    .iter()
                    .any(|base| base.eq_ignore_ascii_case(key))
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// Applies the spec to a command, clearing any inherited environment first.
    fn apply(&self, command: &mut Command) {
        command.env_clear();
        for (key, value) in &self.values {
            command.env(key, value);
        }
    }
}

fn is_forbidden_injection(key: &str) -> bool {
    // Compared case-insensitively: `LD_PRELOAD` and `ld_preload` are the same
    // hazard on a case-insensitive filesystem.
    FORBIDDEN_INJECTIONS
        .iter()
        .any(|forbidden| forbidden.eq_ignore_ascii_case(key))
}

/// Reasons an environment injection was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnvironmentError {
    /// The name is a startup-injection or credential-channel variable.
    ForbiddenKey { key: String },
}

impl std::fmt::Display for EnvironmentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ForbiddenKey { key } => {
                write!(
                    formatter,
                    "{key:?} may not be injected into a child process"
                )
            }
        }
    }
}

impl std::error::Error for EnvironmentError {}

/// A request to run a program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessRequest {
    /// Direct executable plus argument vector. No shell is involved and no
    /// metacharacter is interpreted.
    Direct {
        executable: String,
        arguments: Vec<String>,
    },
    /// An explicitly selected shell running a script that is part of the
    /// request.
    ///
    /// This is a separate mode chosen by the caller, never a consequence of a
    /// direct request failing.
    ExplicitShell { shell: String, script: String },
}

impl ProcessRequest {
    /// Builds a direct request.
    #[must_use]
    pub fn direct(executable: impl Into<String>, arguments: Vec<String>) -> Self {
        Self::Direct {
            executable: executable.into(),
            arguments,
        }
    }

    /// Builds an explicit-shell request.
    #[must_use]
    pub fn explicit_shell(shell: impl Into<String>, script: impl Into<String>) -> Self {
        Self::ExplicitShell {
            shell: shell.into(),
            script: script.into(),
        }
    }

    /// Whether this request runs through a shell.
    #[must_use]
    pub fn uses_shell(&self) -> bool {
        matches!(self, Self::ExplicitShell { .. })
    }
}

/// Everything needed to spawn one owned process.
#[derive(Clone, Debug)]
pub struct SpawnConfig {
    pub request: ProcessRequest,
    pub working_directory: PathBuf,
    pub environment: EnvironmentSpec,
    pub capture_limit: usize,
}

impl SpawnConfig {
    /// Builds a configuration with the default capture limit.
    #[must_use]
    pub fn new(
        request: ProcessRequest,
        working_directory: impl Into<PathBuf>,
        environment: EnvironmentSpec,
    ) -> Self {
        Self {
            request,
            working_directory: working_directory.into(),
            environment,
            capture_limit: DEFAULT_CAPTURE_LIMIT,
        }
    }

    #[must_use]
    pub fn with_capture_limit(mut self, limit: usize) -> Self {
        self.capture_limit = limit;
        self
    }
}

/// Reasons a request could not be spawned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpawnError {
    /// The request names a relative path. A bare name resolved against `PATH`
    /// is accepted; anything containing a `/` must be absolute, so an implicit
    /// current-directory search cannot pick up an unexpected program.
    RelativeExecutable { executable: String },
    /// No executable was found at the requested path or on `PATH`.
    ExecutableNotFound { executable: String },
    /// The resolved path is not a regular executable file.
    NotExecutable { path: String },
    /// The OS refused to create the process.
    SpawnFailed { message: String },
    /// The process started but is not the leader of its own process group, so
    /// the runtime cannot safely signal it. The child is terminated by its own
    /// pid before this is returned; no group is signalled.
    GroupOwnershipUnverifiable { pid: u32, reason: String },
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RelativeExecutable { executable } => write!(
                formatter,
                "relative executable {executable:?} is refused; use an absolute path or a bare name resolved against PATH"
            ),
            Self::ExecutableNotFound { executable } => {
                write!(formatter, "executable {executable:?} was not found")
            }
            Self::NotExecutable { path } => {
                write!(formatter, "{path:?} is not a regular executable file")
            }
            Self::SpawnFailed { message } => write!(formatter, "spawn failed: {message}"),
            Self::GroupOwnershipUnverifiable { pid, reason } => write!(
                formatter,
                "process {pid} did not become leader of its own process group: {reason}"
            ),
        }
    }
}

impl std::error::Error for SpawnError {}

/// Reasons a signal was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignalRefusal {
    /// Group ownership was never verified for this process.
    UnverifiedGroup,
    /// Group `0` would signal the sender's own group.
    ZeroGroup,
    /// The target is the runtime's own process group.
    OwnGroup { pgid: u32 },
    /// The child's group changed since spawn, or the process is already gone.
    /// The recorded group is no longer known to be ours, so nothing is
    /// signalled against it.
    GroupDrifted { expected: u32, found: Option<u32> },
}

impl std::fmt::Display for SignalRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnverifiedGroup => write!(
                formatter,
                "refusing to signal a process whose group ownership was never verified"
            ),
            Self::ZeroGroup => write!(formatter, "refusing to signal group 0"),
            Self::OwnGroup { pgid } => write!(
                formatter,
                "refusing to signal the runtime's own process group {pgid}"
            ),
            Self::GroupDrifted { expected, found } => write!(
                formatter,
                "refusing to signal group {expected}: the process is now in group {found:?}"
            ),
        }
    }
}

impl std::error::Error for SignalRefusal {}

/// Captured output with the truncation flags that make the bound visible.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Capture {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

impl Capture {
    /// stdout decoded as UTF-8, replacing invalid sequences.
    #[must_use]
    pub fn stdout_string(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// stderr decoded as UTF-8, replacing invalid sequences.
    #[must_use]
    pub fn stderr_string(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// How the process came to an end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminationMode {
    /// The process exited on its own before any signal was sent.
    Exited,
    /// The runtime sent `SIGTERM` to the owned group and the process ended
    /// within the grace period. Whether it died from the signal or exited from a
    /// handler it installed is carried by the raw status, not by the mode: the
    /// mode records what this runtime did.
    TerminatedByTerm,
    /// `SIGKILL` to the owned group ended it after escalation.
    TerminatedByKill,
    /// A signal was sent and the process was still present after the escalation
    /// window. Nothing is claimed about it.
    Uncertain,
}

/// The result of running a request to completion or cancellation.
#[derive(Clone, Debug)]
pub struct ProcessOutcome {
    pub pid: u32,
    pub termination: TerminationMode,
    /// The exit status, when one was observed.
    pub status: Option<ExitStatus>,
    pub capture: Capture,
}

/// A running process the runtime owns.
///
/// Ownership means: we spawned it, we read back its process group, and that
/// group is its own. Cancellation is only ever aimed at that recorded group.
#[derive(Debug)]
pub struct OwnedProcess {
    pid: u32,
    pgid: u32,
    group_verified: bool,
    exit_rx: Receiver<ExitStatus>,
    stdout_rx: Receiver<(Vec<u8>, bool)>,
    stderr_rx: Receiver<(Vec<u8>, bool)>,
    reader_threads: Option<(JoinHandle<()>, JoinHandle<()>)>,
    waiter: Option<JoinHandle<()>>,
    /// The status observed when a cancellation ended the process.
    status: Option<ExitStatus>,
}

/// Resolves the request's executable to an absolute path.
///
/// A bare name is looked up on the effective `PATH`. A path containing a `/`
/// must already be absolute, which removes the implicit current-directory
/// search where a co-located file could be picked up instead of the intended
/// program.
fn resolve_executable(
    executable: &str,
    environment: &EnvironmentSpec,
) -> Result<PathBuf, SpawnError> {
    let path_field = Path::new(executable);
    if path_field.components().count() == 0 {
        return Err(SpawnError::ExecutableNotFound {
            executable: executable.to_string(),
        });
    }
    if path_field.is_absolute() {
        let resolved = require_executable(path_field, executable)?;
        return Ok(resolved.canonicalize().unwrap_or(resolved));
    }
    if executable.contains('/') {
        return Err(SpawnError::RelativeExecutable {
            executable: executable.to_string(),
        });
    }
    let search_path = environment
        .entries()
        .get("PATH")
        .cloned()
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    for directory in std::env::split_paths(&search_path) {
        if directory.as_os_str().is_empty() {
            continue;
        }
        let candidate = directory.join(executable);
        if is_executable_file(&candidate) {
            // Canonicalize so the recorded identity is the program that will
            // actually run, not the link that pointed at it.
            return Ok(candidate.canonicalize().unwrap_or(candidate));
        }
    }
    Err(SpawnError::ExecutableNotFound {
        executable: executable.to_string(),
    })
}

fn require_executable(path: &Path, original: &str) -> Result<PathBuf, SpawnError> {
    if is_executable_file(path) {
        return Ok(path.to_path_buf());
    }
    if path.metadata().is_ok() {
        return Err(SpawnError::NotExecutable {
            path: path.display().to_string(),
        });
    }
    Err(SpawnError::ExecutableNotFound {
        executable: original.to_string(),
    })
}

/// Whether `path` resolves to a regular executable file.
///
/// This follows symlinks on purpose: `/bin/sh -> bash` is the normal layout and a
/// PATH lookup that refused symlinks could not find any shell at all. This is a
/// program-identity check, not agent-directed file access; the RUN-01 confinement
/// rules that reject symlinks apply to workspace paths, not to resolving the
/// interpreter the operator named.
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn get_group_of(pid: u32) -> Result<u32, String> {
    let Ok(raw) = i32::try_from(pid) else {
        return Err("pid does not fit the platform pid range".to_string());
    };
    let Some(target) = Pid::from_raw(raw) else {
        return Err("pid is not representable on this platform".to_string());
    };
    match getpgid(Some(target)) {
        Ok(pgid) => Ok(u32::try_from(Pid::as_raw(Some(pgid))).unwrap_or(0)),
        Err(error) => Err(error.to_string()),
    }
}

/// Spawns the request in its own verified process group.
///
/// # Errors
///
/// See [`SpawnError`]. A failure here means no process is left running.
pub fn spawn(config: SpawnConfig) -> Result<OwnedProcess, SpawnError> {
    let mut command = match &config.request {
        ProcessRequest::Direct {
            executable,
            arguments,
        } => {
            let resolved = resolve_executable(executable, &config.environment)?;
            let mut command = Command::new(resolved);
            command.args(arguments);
            command
        }
        ProcessRequest::ExplicitShell { shell, script } => {
            let resolved = resolve_executable(shell, &config.environment)?;
            let mut command = Command::new(resolved);
            // The script travels as one argument, so the shell receives exactly
            // what the request carried.
            command.arg("-c").arg(script);
            command
        }
    };

    command.current_dir(&config.working_directory);
    config.environment.apply(&mut command);
    // Closed stdin. A child that reads gets EOF immediately rather than
    // reaching a terminal or a descriptor the runtime still wants.
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    // A new process group led by the child. Terminal-generated signals do not
    // reach it, and cancellation can reach its whole tree.
    command.process_group(0);

    let mut child = command.spawn().map_err(|error| SpawnError::SpawnFailed {
        message: error.to_string(),
    })?;
    let pid = child.id();

    // Drop our copy of the child's stdin immediately. Holding it open would let
    // the child wait on a pipe nobody intends to write to.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.flush();
    }

    // Verify group ownership before anything could signal. If the child is not
    // its own group leader we must not use group signalling against it, and we
    // must not leave it running either: it is terminated by the single pid we
    // know we created, never by a group.
    if let Err(reason) = verify_group_leader(pid) {
        terminate_single_process(pid);
        let _ = child.wait();
        return Err(SpawnError::GroupOwnershipUnverifiable { pid, reason });
    }
    let pgid = pid;

    let Some(stdout) = child.stdout.take() else {
        terminate_single_process(pid);
        let _ = child.wait();
        return Err(SpawnError::SpawnFailed {
            message: "stdout pipe was not available".to_string(),
        });
    };
    let Some(stderr) = child.stderr.take() else {
        terminate_single_process(pid);
        let _ = child.wait();
        return Err(SpawnError::SpawnFailed {
            message: "stderr pipe was not available".to_string(),
        });
    };

    let (exit_tx, exit_rx) = mpsc::channel();
    let (stdout_tx, stdout_rx) = mpsc::channel();
    let (stderr_tx, stderr_rx) = mpsc::channel();
    let limit = config.capture_limit;

    let stdout_thread = std::thread::spawn(move || {
        let _ = stdout_tx.send(drain_bounded(stdout, limit));
    });
    let stderr_thread = std::thread::spawn(move || {
        let _ = stderr_tx.send(drain_bounded(stderr, limit));
    });
    let waiter = std::thread::spawn(move || {
        if let Ok(status) = child.wait() {
            let _ = exit_tx.send(status);
        }
    });

    Ok(OwnedProcess {
        pid,
        pgid,
        group_verified: true,
        exit_rx,
        stdout_rx,
        stderr_rx,
        reader_threads: Some((stdout_thread, stderr_thread)),
        waiter: Some(waiter),
        status: None,
    })
}

fn verify_group_leader(pid: u32) -> Result<(), String> {
    match get_group_of(pid) {
        Ok(pgid) if pgid == pid => Ok(()),
        Ok(found) => Err(format!("group is {found}, expected {pid}")),
        Err(reason) => Err(reason),
    }
}

/// Signals one pid, never a group. Used only for a process we just created and
/// can no longer own as a group.
fn terminate_single_process(pid: u32) {
    if let Ok(raw) = i32::try_from(pid)
        && let Some(target) = Pid::from_raw(raw)
    {
        let _ = kill_process(target, Signal::Kill);
    }
}

/// Reads a stream up to `limit` bytes, then keeps draining and discarding.
fn drain_bounded(mut reader: impl Read, limit: usize) -> (Vec<u8>, bool) {
    let mut buffer = [0u8; 8192];
    let mut captured = Vec::new();
    let mut truncated = false;
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                if captured.len() < limit {
                    let keep = (limit - captured.len()).min(read);
                    captured.extend_from_slice(&buffer[..keep]);
                }
                if read > 0 && captured.len() >= limit {
                    // Still draining. Discarding is the point: the child must
                    // not block on a pipe nobody is emptying.
                    truncated = true;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    (captured, truncated)
}

impl OwnedProcess {
    /// The pid of the process we spawned.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The verified owned process group.
    #[must_use]
    pub fn pgid(&self) -> u32 {
        self.pgid
    }

    #[must_use]
    pub fn group_verified(&self) -> bool {
        self.group_verified
    }

    /// Checks that the recorded group is still safe to signal.
    pub(crate) fn signal_target(&self) -> Result<u32, SignalRefusal> {
        if !self.group_verified {
            return Err(SignalRefusal::UnverifiedGroup);
        }
        if self.pgid == 0 {
            return Err(SignalRefusal::ZeroGroup);
        }
        let own_pgid = u32::try_from(Pid::as_raw(Some(getpgrp()))).unwrap_or(0);
        if self.pgid == own_pgid {
            return Err(SignalRefusal::OwnGroup { pgid: own_pgid });
        }
        match get_group_of(self.pid) {
            Ok(current) if current == self.pgid => Ok(self.pgid),
            // The process changed group, or is already gone and its pid could be
            // recycled. Either way the recorded group is not ours to signal.
            Ok(current) => Err(SignalRefusal::GroupDrifted {
                expected: self.pgid,
                found: Some(current),
            }),
            Err(_) => Err(SignalRefusal::GroupDrifted {
                expected: self.pgid,
                found: None,
            }),
        }
    }

    /// Waits for the process to exit on its own within `timeout`.
    ///
    /// Returns `None` if the timeout elapsed with the process still running.
    #[must_use]
    pub fn wait(&mut self, timeout: Duration) -> Option<ExitStatus> {
        self.exit_rx.recv_timeout(timeout).ok()
    }

    /// Cancels the owned process: `SIGTERM` to the group, then `SIGKILL` after
    /// the grace period, then a bounded wait before declaring uncertainty.
    ///
    /// # Errors
    ///
    /// Returns the [`SignalRefusal`] if the group is not safe to signal. In that
    /// case no signal of any kind was sent.
    pub fn cancel(
        &mut self,
        grace: Duration,
        escalation: Duration,
    ) -> Result<TerminationMode, SignalRefusal> {
        let target = self.signal_target()?;
        let raw = i32::try_from(target).map_err(|_| SignalRefusal::UnverifiedGroup)?;
        let target_pid = Pid::from_raw(raw).ok_or(SignalRefusal::UnverifiedGroup)?;

        // Graceful first. A process that cooperates is ended without loss.
        let _ = kill_process_group(target_pid, Signal::Term);
        if let Some(status) = self.wait(grace) {
            self.status = Some(status);
            return Ok(TerminationMode::TerminatedByTerm);
        }

        // Escalate. The group is still ours: the leader has not exited, so the
        // pid cannot have been recycled.
        let _ = kill_process_group(target_pid, Signal::Kill);
        if let Some(status) = self.wait(escalation) {
            self.status = Some(status);
            return Ok(TerminationMode::TerminatedByKill);
        }
        Ok(TerminationMode::Uncertain)
    }

    /// Collects captured output. Both readers must have finished; if one has
    /// not, the stream is reported as truncated rather than waited on forever,
    /// because a descendant holding the pipe open is exactly the case that must
    /// not stall the caller.
    fn collect_capture(&mut self, budget: Duration) -> Capture {
        let deadline = Instant::now() + budget;
        let (stdout, stdout_truncated) = recv_capture(&self.stdout_rx, deadline);
        let (stderr, stderr_truncated) = recv_capture(&self.stderr_rx, deadline);
        Capture {
            stdout,
            stderr,
            stdout_truncated,
            stderr_truncated,
        }
    }

    /// Runs the process to completion or cancellation and returns the outcome.
    ///
    /// On timeout the process is cancelled with the given grace and escalation
    /// windows. A refusal to signal is returned rather than swallowed, because
    /// an unowned process that is still running is information the caller needs.
    ///
    /// # Errors
    ///
    /// Returns [`SignalRefusal`] if cancellation was needed but the group was
    /// not safe to signal.
    pub fn run_to_completion(
        &mut self,
        timeout: Duration,
        grace: Duration,
        escalation: Duration,
    ) -> Result<ProcessOutcome, SignalRefusal> {
        let (termination, status) = match self.wait(timeout) {
            Some(observed) => (TerminationMode::Exited, Some(observed)),
            None => (self.cancel(grace, escalation)?, self.status),
        };
        let capture = self.collect_capture(grace.max(escalation));
        Ok(ProcessOutcome {
            pid: self.pid,
            termination,
            status,
            capture,
        })
    }
}

fn recv_capture(receiver: &Receiver<(Vec<u8>, bool)>, deadline: Instant) -> (Vec<u8>, bool) {
    loop {
        match receiver.try_recv() {
            Ok((bytes, truncated)) => return (bytes, truncated),
            Err(TryRecvError::Disconnected) => return (Vec::new(), true),
            Err(TryRecvError::Empty) => {
                if Instant::now() >= deadline {
                    return (Vec::new(), true);
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
}

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        // Joining here keeps a dropped handle from leaving orphaned reader
        // threads. It does not cancel the child: cancellation is an explicit
        // decision with a recorded outcome, not a side effect of scope exit.
        if let Some((stdout, stderr)) = self.reader_threads.take() {
            let _ = stdout.join();
            let _ = stderr.join();
        }
        if let Some(waiter) = self.waiter.take() {
            let _ = waiter.join();
        }
    }
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
