//! Owned process lifecycle tests.
//!
//! Child processes are the test binary itself running an `#[ignore]`d role, so a
//! role cannot run on its own outside a driving parent. Synchronization uses
//! bounded polling for a file the child writes, never a fixed sleep standing in
//! for "the child reached the point".
//!
//! The environment tests never mutate this process's own environment:
//! `set_var`/`remove_var` are `unsafe` in edition 2024 and the workspace forbids
//! unsafe code. Proving isolation instead works from the stronger direction — a
//! child spawned with an empty spec has *no* environment at all, so nothing the
//! runtime holds, secret or otherwise, can reach it.

use std::io::IsTerminal;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustix::process::{Pid, getpgrp};

use super::*;

const GRACE: Duration = Duration::from_millis(400);
const ESCALATION: Duration = Duration::from_millis(800);
const WAIT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(5);
/// Every line a child role emits deliberately is prefixed, so libtest's own stdout
/// never enters an assertion about what the role reported.
const ROLE_PREFIX: &str = "FLUZO_ROLE|";

/// Extracts the payload of the role's own output lines, discarding libtest noise.
fn role_lines(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| line.strip_prefix(ROLE_PREFIX))
        .map(str::to_string)
        .collect()
}

fn test_exe() -> PathBuf {
    std::env::current_exe().expect("test binary path")
}

fn cwd() -> PathBuf {
    std::env::current_dir().expect("current directory")
}

/// Builds a direct request that runs one ignored child role.
fn role_request(role: &str) -> ProcessRequest {
    ProcessRequest::direct(
        test_exe().to_str().expect("utf-8 test path"),
        vec![
            "--exact".to_string(),
            "--ignored".to_string(),
            "--nocapture".to_string(),
            format!("process::tests::child_role_{role}"),
        ],
    )
}

fn spawn_role_with(role: &str, environment: EnvironmentSpec) -> OwnedProcess {
    spawn(SpawnConfig::new(role_request(role), cwd(), environment)).expect("role child spawns")
}

fn spawn_role(role: &str) -> OwnedProcess {
    spawn_role_with(role, EnvironmentSpec::empty())
}

fn run_role_with(role: &str, environment: EnvironmentSpec, limit: usize) -> ProcessOutcome {
    let config = SpawnConfig::new(role_request(role), cwd(), environment).with_capture_limit(limit);
    let mut process = spawn(config).expect("role child spawns");
    process
        .run_to_completion(WAIT, GRACE, ESCALATION)
        .expect("role completes without a signal refusal")
}

fn run_role(role: &str) -> ProcessOutcome {
    let mut process = spawn_role(role);
    process
        .run_to_completion(WAIT, GRACE, ESCALATION)
        .expect("role completes without a signal refusal")
}

fn unique_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "fluzo-process-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    ))
}

/// Waits until `path` exists, so a parent never guesses when a child got there.
fn wait_for_file(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        std::thread::sleep(POLL);
    }
    false
}

fn spawn_shell(script: &str) -> OwnedProcess {
    spawn(SpawnConfig::new(
        ProcessRequest::explicit_shell("sh", script),
        cwd(),
        EnvironmentSpec::baseline(),
    ))
    .expect("explicit shell spawns")
}

fn is_alive(pid: u32) -> bool {
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    Pid::from_raw(raw).map(|p| rustix::process::test_kill_process(p).is_ok()) == Some(true)
}

fn our_pgid() -> u32 {
    u32::try_from(Pid::as_raw(Some(getpgrp()))).expect("own pgid fits u32")
}

// ---------------------------------------------------------------------------
// Request shapes
// ---------------------------------------------------------------------------

#[test]
fn direct_request_delivers_argv_verbatim_without_shell_interpretation() {
    let awkward = [
        "plain",
        "; rm -rf /tmp/never",
        "$HOME",
        "`id`",
        "a|b && c",
        "star*",
        "'quoted'",
        "tab\there",
    ];
    let mut arguments = vec![
        "--exact".to_string(),
        "--ignored".to_string(),
        "--nocapture".to_string(),
        "process::tests::child_role_print_argv".to_string(),
    ];
    arguments.extend(awkward.iter().map(|value| (*value).to_string()));
    let config = SpawnConfig::new(
        ProcessRequest::direct(test_exe().to_str().unwrap(), arguments),
        cwd(),
        EnvironmentSpec::empty(),
    )
    .with_capture_limit(64 * 1024);

    let outcome = spawn(config)
        .expect("spawn")
        .run_to_completion(WAIT, GRACE, ESCALATION)
        .expect("no signal refusal");

    let lines = role_lines(&outcome.capture.stdout_string());
    for value in awkward {
        assert!(
            lines.iter().any(|line| line == value),
            "argv value {value:?} was not delivered verbatim; got {lines:?}"
        );
    }
    // A shell would have expanded `$HOME` and run the `; rm` chain. The literal
    // surviving both is the point.
    assert!(
        lines.iter().any(|line| line == "$HOME"),
        "$HOME appears to have been expanded"
    );
    assert_eq!(outcome.termination, TerminationMode::Exited);
}

#[test]
fn explicit_shell_is_a_distinct_mode_whose_script_runs() {
    let mut environment = EnvironmentSpec::empty();
    environment
        .inject("FLUZO_TEST_MARKER", "shell-ran")
        .expect("marker key is allowed");
    let config = SpawnConfig::new(
        ProcessRequest::explicit_shell("sh", "printf '%s' \"$FLUZO_TEST_MARKER\""),
        cwd(),
        environment,
    );
    let outcome = spawn(config)
        .expect("explicit shell spawns")
        .run_to_completion(WAIT, GRACE, ESCALATION)
        .expect("no signal refusal");
    assert_eq!(outcome.capture.stdout_string(), "shell-ran");
    assert!(outcome.status.map(|s| s.success()).unwrap_or(false));
}

#[test]
fn a_direct_request_is_never_reported_as_using_a_shell() {
    assert!(
        !ProcessRequest::direct("sh", vec!["-c".into(), "true".into()]).uses_shell(),
        "a direct request must not be reported as shell-backed"
    );
    assert!(ProcessRequest::explicit_shell("sh", "true").uses_shell());
}

#[test]
fn a_failed_direct_spawn_does_not_fall_back_to_a_shell() {
    let missing = test_exe()
        .parent()
        .unwrap()
        .join("definitely-not-a-real-binary-xyz");
    let config = SpawnConfig::new(
        ProcessRequest::direct(missing.to_str().unwrap(), Vec::new()),
        cwd(),
        EnvironmentSpec::baseline(),
    );
    let error = spawn(config).expect_err("the executable does not exist");
    assert!(
        matches!(error, SpawnError::ExecutableNotFound { .. }),
        "expected a typed not-found error, got {error:?}"
    );
}

#[test]
fn a_bare_name_resolves_against_path_to_an_absolute_program() {
    let resolved = resolve_executable("sh", &EnvironmentSpec::baseline()).expect("sh is on PATH");
    assert!(
        resolved.is_absolute(),
        "resolved path {resolved:?} is not absolute"
    );
}

#[test]
fn a_relative_executable_is_refused() {
    let error = resolve_executable("./sh", &EnvironmentSpec::baseline())
        .expect_err("a relative path must be refused");
    assert!(matches!(error, SpawnError::RelativeExecutable { .. }));
    let error =
        resolve_executable("../bin/sh", &EnvironmentSpec::baseline()).expect_err("must be refused");
    assert!(matches!(error, SpawnError::RelativeExecutable { .. }));
}

#[test]
fn a_directory_is_not_accepted_as_an_executable() {
    let error = require_executable(&cwd(), "whatever").expect_err("a directory is not a program");
    assert!(matches!(error, SpawnError::NotExecutable { .. }));
}

// ---------------------------------------------------------------------------
// stdin
// ---------------------------------------------------------------------------

#[test]
fn child_stdin_is_closed_and_reads_return_eof_immediately() {
    let outcome = run_role("read_stdin");
    // The role reports EOF only after read_to_end returned zero bytes.
    assert_eq!(
        role_lines(&outcome.capture.stdout_string()),
        vec!["EOF".to_string()]
    );
    assert_eq!(outcome.termination, TerminationMode::Exited);
}

#[test]
fn no_standard_descriptor_of_the_child_is_a_terminal() {
    let outcome = run_role("tty_probe");
    let lines = role_lines(&outcome.capture.stdout_string());
    for expected in ["stdin_tty=false", "stdout_tty=false", "stderr_tty=false"] {
        assert!(
            lines.iter().any(|line| line == expected),
            "expected {expected} in the child descriptor report; got {lines:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------

#[test]
fn a_child_spawned_with_an_empty_spec_receives_no_environment_at_all() {
    // The strongest form of "built, not inherited": with an empty spec the child
    // has zero variables, so nothing the runtime holds — sentinel credentials or
    // anything else — can travel to it.
    let outcome = run_role("env_dump");
    let lines = role_lines(&outcome.capture.stdout_string());
    assert!(
        lines.is_empty(),
        "the child received an environment despite an empty spec: {lines:?}"
    );
}

#[test]
fn child_environment_is_exactly_the_baseline_plus_injections() {
    let mut environment = EnvironmentSpec::empty();
    environment.inject("PATH", "/usr/bin").expect("allowed");
    environment
        .inject("FLUZO_ALLOWED_VAR", "yes")
        .expect("allowed");
    let outcome = run_role_with("env_dump", environment, 64 * 1024);
    let reported: Vec<String> = role_lines(&outcome.capture.stdout_string())
        .into_iter()
        .map(|line| line.split('=').next().unwrap_or("").to_string())
        .filter(|key| !key.is_empty())
        .collect();
    assert_eq!(
        reported,
        vec!["FLUZO_ALLOWED_VAR".to_string(), "PATH".to_string()],
        "the child environment is not exactly what the spec declared"
    );
}

#[test]
fn the_baseline_contains_only_allowlisted_keys() {
    let baseline = EnvironmentSpec::baseline();
    for key in baseline.entries().keys() {
        assert!(
            BASELINE_KEYS.contains(&key.as_str()),
            "baseline carried {key:?}, which is not on the allowlist"
        );
    }
}

#[test]
fn the_allowlist_excludes_startup_and_credential_variables() {
    for forbidden in FORBIDDEN_INJECTIONS {
        assert!(
            !BASELINE_KEYS.contains(forbidden),
            "{forbidden} must never be part of the baseline"
        );
    }
}

#[test]
fn startup_injection_variables_cannot_be_injected() {
    let mut spec = EnvironmentSpec::empty();
    for key in [
        "BASH_ENV",
        "ENV",
        "PROMPT_COMMAND",
        "LD_PRELOAD",
        "CDPATH",
        "IFS",
    ] {
        let error = spec
            .inject(key, "value")
            .expect_err("startup injection variables are refused");
        assert!(matches!(error, EnvironmentError::ForbiddenKey { .. }));
    }
    // The same hazard spelled differently is still refused.
    let error = spec
        .inject("bash_env", "value")
        .expect_err("case-insensitive refusal");
    assert_eq!(
        error,
        EnvironmentError::ForbiddenKey {
            key: "bash_env".to_string()
        }
    );
    // And a refused key really did not get added.
    assert!(!spec.entries().contains_key("bash_env"));
}

#[test]
fn credential_channel_variables_cannot_be_injected() {
    let mut spec = EnvironmentSpec::empty();
    for key in [
        "SSH_AUTH_SOCK",
        "GIT_SSH_COMMAND",
        "GIT_ASKPASS",
        "SSH_ASKPASS",
        "GPG_AGENT_INFO",
        "DBUS_SESSION_BUS_ADDRESS",
    ] {
        assert!(
            spec.inject(key, "x").is_err(),
            "{key} must not be injectable"
        );
    }
}

// ---------------------------------------------------------------------------
// Bounded capture
// ---------------------------------------------------------------------------

#[test]
fn capture_stops_at_the_limit_and_marks_truncation_without_stalling_the_child() {
    // The role writes 256 KiB per stream against a 1 KiB cap. If draining
    // stopped at the cap the child would block on a full pipe and this would
    // only finish by hitting WAIT, so the elapsed check is the real assertion.
    let started = Instant::now();
    let outcome = run_role_with("chatty", EnvironmentSpec::empty(), 1024);
    assert!(
        started.elapsed() < WAIT,
        "the child stalled on a full pipe after the capture cap"
    );
    assert_eq!(outcome.capture.stdout.len(), 1024);
    assert!(outcome.capture.stdout_truncated);
    assert_eq!(outcome.capture.stderr.len(), 1024);
    assert!(outcome.capture.stderr_truncated);
    assert_eq!(outcome.termination, TerminationMode::Exited);
}

#[test]
fn capture_below_the_limit_is_not_marked_truncated() {
    let outcome = run_role("quiet");
    assert_eq!(
        role_lines(&outcome.capture.stdout_string()),
        vec!["small".to_string()]
    );
    assert!(!outcome.capture.stdout_truncated);
    assert!(!outcome.capture.stderr_truncated);
}

// ---------------------------------------------------------------------------
// Cancellation
//
// Every cancellation test waits for the child to announce that it is in place
// before signalling. Without that, the signal can arrive before the shell has
// installed its trap, and the test would be measuring a race instead of the
// behaviour it names.
// ---------------------------------------------------------------------------

/// Waits for a role or script to report readiness, then returns its marker path.
fn ready_marker(tag: &str) -> PathBuf {
    let path = unique_path(tag);
    let _ = std::fs::remove_file(&path);
    path
}

fn expect_ready(path: &Path) {
    assert!(
        wait_for_file(path, WAIT),
        "the child never announced readiness at {}",
        path.display()
    );
}

/// Waits until `pid` is really gone. A process that has died but not yet been
/// reaped still answers `kill(pid, 0)`, and group signalling is asynchronous, so
/// an immediate check would report a false survival.
fn wait_until_gone(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !is_alive(pid) {
            return true;
        }
        std::thread::sleep(POLL);
    }
    !is_alive(pid)
}

#[test]
fn cancellation_terminates_a_cooperative_child_within_the_grace_period() {
    let ready = ready_marker("coop");
    let mut process = spawn_shell(&format!(
        "trap 'exit 0' TERM; echo ready > {}; sleep 30",
        ready.display()
    ));
    expect_ready(&ready);

    let mode = process
        .cancel(GRACE, ESCALATION)
        .expect("group is signallable");
    assert_eq!(mode, TerminationMode::TerminatedByTerm, "got {mode:?}");
    // The handler exited cleanly, so the raw status still shows the cooperation.
    assert_eq!(process.status.map(|s| s.code()), Some(Some(0)));
    assert!(wait_until_gone(process.pid(), WAIT));
    let _ = std::fs::remove_file(&ready);
}

#[test]
fn a_child_ignoring_sigterm_is_escalated_to_sigkill() {
    let ready = ready_marker("stubborn");
    // An ignored trap is inherited by the child command too, so the group TERM
    // reaches nobody and only escalation can end this.
    let mut process = spawn_shell(&format!(
        "trap '' TERM; echo ready > {}; sleep 30",
        ready.display()
    ));
    expect_ready(&ready);

    let mode = process
        .cancel(Duration::from_millis(250), ESCALATION)
        .expect("group is signallable");
    assert_eq!(mode, TerminationMode::TerminatedByKill, "got {mode:?}");
    assert_eq!(
        process
            .status
            .map(|s| std::os::unix::process::ExitStatusExt::signal(&s)),
        Some(Some(9))
    );
    assert!(wait_until_gone(process.pid(), WAIT));
    let _ = std::fs::remove_file(&ready);
}

#[test]
fn group_cancellation_reaches_a_descendant_that_never_had_a_handler() {
    let ready = ready_marker("descendant");
    let mut process = spawn_shell(&format!("sleep 40 & echo $! > {} ; wait", ready.display()));
    expect_ready(&ready);
    let descendant: u32 = std::fs::read_to_string(&ready)
        .expect("marker readable")
        .trim()
        .parse()
        .expect("pid parses");
    assert_ne!(
        descendant,
        process.pid(),
        "the marker must name the descendant"
    );
    assert!(is_alive(descendant), "descendant was not running yet");
    assert_eq!(
        get_group_of(descendant).expect("descendant group"),
        process.pgid(),
        "the descendant must sit in the owned group for this to test anything"
    );

    let _ = process.cancel(GRACE, ESCALATION);

    assert!(
        wait_until_gone(descendant, WAIT),
        "descendant {descendant} survived group cancellation"
    );
    let _ = std::fs::remove_file(&ready);
}

#[test]
fn cancelling_one_owned_process_leaves_an_unrelated_process_untouched() {
    let mut target = spawn_shell("sleep 30");
    let mut bystander = spawn_shell("sleep 30");
    let bystander_pid = bystander.pid();
    assert_ne!(target.pgid(), bystander.pgid());

    let _ = target.cancel(GRACE, ESCALATION);

    assert!(
        is_alive(bystander_pid),
        "an unrelated owned process was signalled by cancelling another"
    );
    let _ = bystander.cancel(GRACE, ESCALATION);
    assert!(wait_until_gone(bystander_pid, WAIT));
}

#[test]
fn cancelling_an_already_exited_process_refuses_to_use_the_recorded_group() {
    let mut process = spawn_role("quiet");
    assert!(process.wait(WAIT).is_some(), "role did not exit");
    let pgid = process.pgid();
    let error = process
        .cancel(GRACE, ESCALATION)
        .expect_err("the pid is gone, so the group must not be used");
    match error {
        SignalRefusal::GroupDrifted { expected, found } => {
            assert_eq!(expected, pgid);
            let _ = found;
        }
        other => panic!("expected GroupDrifted, got {other:?}"),
    }
}

#[test]
fn cancellation_refuses_an_unverified_group() {
    let mut process = spawn_role("quiet");
    process.group_verified = false;
    let error = process
        .cancel(GRACE, ESCALATION)
        .expect_err("an unverified group must never be signalled");
    assert_eq!(error, SignalRefusal::UnverifiedGroup);
}

#[test]
fn cancellation_refuses_group_zero() {
    let mut process = spawn_role("quiet");
    process.pgid = 0;
    assert_eq!(
        process.cancel(GRACE, ESCALATION).unwrap_err(),
        SignalRefusal::ZeroGroup
    );
}

#[test]
fn cancellation_refuses_the_runtime_own_group() {
    let mut process = spawn_role("quiet");
    process.pgid = our_pgid();
    let error = process
        .cancel(GRACE, ESCALATION)
        .expect_err("the runtime's own group must never be signalled");
    assert_eq!(error, SignalRefusal::OwnGroup { pgid: our_pgid() });
}

#[test]
fn spawn_verifies_group_leadership_before_returning() {
    let process = spawn_role("quiet");
    assert!(process.group_verified());
    assert_eq!(
        get_group_of(process.pid()).expect("child group"),
        process.pgid(),
        "the recorded group is not the child's actual group"
    );
    assert_ne!(
        process.pgid(),
        our_pgid(),
        "the child must not share the runtime's process group"
    );
}

#[test]
fn a_cancellation_never_reports_a_voluntary_exit() {
    // With windows shrunk to nearly nothing the runtime may not confirm the
    // death. What it must never do is report `Exited`, which would claim the
    // process finished on its own when we in fact signalled it.
    let mut process = spawn_shell("sleep 30");
    let mode = process
        .cancel(Duration::from_millis(1), Duration::from_millis(1))
        .expect("group is signallable");
    assert_ne!(
        mode,
        TerminationMode::Exited,
        "a cancelled process was reported as exiting voluntarily"
    );
    if mode == TerminationMode::Uncertain {
        let settled = process
            .cancel(GRACE, ESCALATION)
            .expect("still signallable");
        assert_ne!(settled, TerminationMode::Exited);
    }
    assert!(wait_until_gone(process.pid(), WAIT));
}

// ---------------------------------------------------------------------------
// Ignored child roles. Each is driven by a parent test through the test filter.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "driven as a child process by the parent tests"]
fn child_role_print_argv() {
    for arg in std::env::args().skip(1) {
        println!("{ROLE_PREFIX}{arg}");
    }
}

#[test]
#[ignore = "driven as a child process by the parent tests"]
fn child_role_read_stdin() {
    let mut buffer = Vec::new();
    std::io::stdin()
        .read_to_end(&mut buffer)
        .expect("stdin read");
    assert!(buffer.is_empty(), "stdin delivered unexpected bytes");
    println!("{ROLE_PREFIX}EOF");
}

#[test]
#[ignore = "driven as a child process by the parent tests"]
fn child_role_env_dump() {
    let mut entries: Vec<(String, String)> = std::env::vars().collect();
    entries.sort();
    for (key, value) in entries {
        println!("{ROLE_PREFIX}{key}={value}");
    }
}

#[test]
#[ignore = "driven as a child process by the parent tests"]
fn child_role_tty_probe() {
    println!("{ROLE_PREFIX}stdin_tty={}", std::io::stdin().is_terminal());
    println!(
        "{ROLE_PREFIX}stdout_tty={}",
        std::io::stdout().is_terminal()
    );
    println!(
        "{ROLE_PREFIX}stderr_tty={}",
        std::io::stderr().is_terminal()
    );
}

#[test]
#[ignore = "driven as a child process by the parent tests"]
fn child_role_chatty() {
    let chunk = vec![b'x'; 4096];
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    for _ in 0..64 {
        let _ = stdout.write_all(&chunk);
        let _ = stderr.write_all(&chunk);
    }
    let _ = stdout.flush();
    let _ = stderr.flush();
}

#[test]
#[ignore = "driven as a child process by the parent tests"]
fn child_role_quiet() {
    println!("{ROLE_PREFIX}small");
}
