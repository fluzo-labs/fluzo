//! Read-only Git inspection that cannot be turned into a program launch.
//!
//! Repository configuration is an execution surface. `core.pager`,
//! `diff.external`, `core.fsmonitor` and `core.hooksPath` all name programs, and
//! a repository is not a trusted author of its own configuration. This module
//! enumerates the inspections that may run, disables every configuration-driven
//! helper before running one, and executes it through the owned-process path so
//! the same group, stdin and capture rules apply as to any other tool.
//!
//! Two choices carry the design:
//!
//! * **The inspectable set is closed at the type level.** A mutating or unlisted
//!   Git operation is not representable as an [`Inspection`]. There is no
//!   `run_git_command(&str)` to reach for.
//! * **Helpers are disabled by precedence, not by hope.** Command-line `-c`
//!   overrides repository config, and `GIT_CONFIG_GLOBAL` pointed at the empty
//!   device removes the user's own configuration from the picture. The
//!   repository cannot widen what a request does because the request never reads
//!   the places that would tell it to.
//!
//! This is not a claim that arbitrary Git is safe. It is a claim that *these*
//! operations do not execute code the operator did not name.

use std::path::Path;

use super::process::{
    EnvironmentSpec, ProcessOutcome, ProcessRequest, SpawnConfig, SpawnError, TerminationMode,
};

/// Environment variable pointing Git's user configuration at the empty device.
///
/// With `env_clear` in the process layer this is what keeps `~/.gitconfig` — and
/// any `credential.helper` or `core.pager` in it — out of the child.
pub const EMPTY_CONFIG_PATH: &str = "/dev/null";

/// Default ceiling on inspection output.
pub const DEFAULT_INSPECTION_LIMIT: usize = 1024 * 1024;

/// A read-only Git inspection.
///
/// Every variant maps to a Git invocation that reads repository state and to
/// nothing else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inspection {
    /// Working tree and index status.
    Status,
    /// Tracked file list.
    TrackedFiles,
    /// Unstaged changes.
    Diff,
    /// Staged changes.
    StagedDiff,
    /// Recent history, bounded by `max_count`.
    Log { max_count: u32 },
    /// A single revision's metadata and diff.
    Show { rev: String },
    /// Resolve a revision expression to an object id.
    RevParse { rev: String },
    /// Unstaged changes restricted to one path inside the repository.
    DiffPath { path: String },
}

/// Reasons an inspection was refused before anything ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InspectionError {
    /// A revision expression contains something outside the accepted character
    /// set, or is empty.
    InvalidRevision { rev: String, reason: &'static str },
    /// A path argument was absolute, empty, escaping, or option-shaped.
    InvalidPath { path: String, reason: &'static str },
    /// A requested bound exceeded the maximum this module will honour.
    BoundTooLarge { requested: u32, maximum: u32 },
    /// The inspection could not be spawned.
    Spawn(SpawnError),
}

impl std::fmt::Display for InspectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRevision { rev, reason } => {
                write!(formatter, "revision {rev:?} rejected: {reason}")
            }
            Self::InvalidPath { path, reason } => {
                write!(formatter, "path {path:?} rejected: {reason}")
            }
            Self::BoundTooLarge { requested, maximum } => write!(
                formatter,
                "requested bound {requested} exceeds the maximum {maximum}"
            ),
            Self::Spawn(error) => write!(formatter, "inspection could not start: {error}"),
        }
    }
}

impl std::error::Error for InspectionError {}

/// Maximum history entries a single log inspection may request.
pub const MAX_LOG_COUNT: u32 = 1000;
/// Maximum length of a revision expression.
pub const MAX_REV_LEN: usize = 256;

/// Builds the hardened environment for a Git invocation.
///
/// The baseline supplies `PATH`, locale and home; everything Git consults for
/// helper programs is then pinned here.
#[must_use]
pub fn hardened_git_environment() -> EnvironmentSpec {
    let mut environment = EnvironmentSpec::baseline();
    // The empty device is not a config file, so Git reads no user configuration.
    pin(&mut environment, "GIT_CONFIG_GLOBAL", EMPTY_CONFIG_PATH);
    pin(&mut environment, "GIT_CONFIG_SYSTEM", EMPTY_CONFIG_PATH);
    pin(&mut environment, "GIT_CONFIG_NOSYSTEM", "1");
    pin(&mut environment, "GIT_ATTR_NOSYSTEM", "1");
    // Never run a pager, and never ask a human for anything.
    pin(&mut environment, "GIT_PAGER", "cat");
    pin(&mut environment, "GIT_TERMINAL_PROMPT", "0");
    pin(&mut environment, "GIT_PAGER_TRACE", "");
    // No external diff driver, even one named by repository config.
    pin(&mut environment, "GIT_EXTERNAL_DIFF", "");
    // Deterministic output: no locale-translated messages, no colour codes.
    pin(&mut environment, "LC_ALL", "C");
    pin(&mut environment, "GIT_COLORS", "never");
    pin(&mut environment, "GIT_NO_REPLACE_OBJECTS", "1");
    environment
}

fn pin(environment: &mut EnvironmentSpec, key: &str, value: &str) {
    let _ = environment.inject(key, value);
}

/// Builds the full argument vector for an inspection.
///
/// The hardening flags are produced here rather than being left to the caller, so
/// there is no way to run one of these inspections without them.
///
/// # Errors
///
/// Returns [`InspectionError`] for a rejected argument or an excessive bound.
pub fn inspection_arguments(inspection: &Inspection) -> Result<Vec<String>, InspectionError> {
    let mut arguments = vec![
        // Command-line config overrides repository config, so a repo cannot put
        // a program behind `core.pager` or `core.fsmonitor`.
        "-c".to_string(),
        "core.pager=cat".to_string(),
        "-c".to_string(),
        "core.fsmonitor=false".to_string(),
        "-c".to_string(),
        "core.hooksPath=/dev/null".to_string(),
        "-c".to_string(),
        "credential.helper=".to_string(),
    ];
    match inspection {
        Inspection::Status => {
            arguments.extend(["status".into(), "--porcelain=v1".into(), "-z".into()]);
        }
        Inspection::TrackedFiles => {
            arguments.extend(["ls-files".into(), "-z".into()]);
        }
        Inspection::Diff | Inspection::StagedDiff => {
            arguments.extend([
                "diff".into(),
                "--no-ext-diff".into(),
                "--no-textconv".into(),
            ]);
            if matches!(inspection, Inspection::StagedDiff) {
                arguments.push("--staged".into());
            }
        }
        Inspection::Log { max_count } => {
            if *max_count > MAX_LOG_COUNT {
                return Err(InspectionError::BoundTooLarge {
                    requested: *max_count,
                    maximum: MAX_LOG_COUNT,
                });
            }
            arguments.extend([
                "log".into(),
                "--no-decorate".into(),
                "--no-ext-diff".into(),
                "--no-textconv".into(),
                format!("--max-count={max_count}"),
            ]);
        }
        Inspection::Show { rev } => {
            validate_revision(rev)?;
            arguments.extend([
                "show".into(),
                "--no-patch".into(),
                "--no-ext-diff".into(),
                "--no-textconv".into(),
                "--".into(),
                rev.clone(),
            ]);
        }
        Inspection::RevParse { rev } => {
            validate_revision(rev)?;
            // No `--` separator: `rev-parse --verify -- <rev>` treats the
            // revision as a path and fails. The validation above is what keeps a
            // value from being read as an option here.
            arguments.extend(["rev-parse".into(), "--verify".into(), rev.clone()]);
        }
        Inspection::DiffPath { path } => {
            validate_path(path)?;
            arguments.extend([
                "diff".into(),
                "--no-ext-diff".into(),
                "--no-textconv".into(),
                "--".into(),
                path.clone(),
            ]);
        }
    }
    Ok(arguments)
}

/// Validates a revision expression.
///
/// The accepted set is the ordinary ref/object-name alphabet. Anything shaped
/// like an option, containing whitespace or control characters, or carrying a
/// path component is refused: those are the shapes that could turn a value into
/// a flag or a filesystem reference.
fn validate_revision(rev: &str) -> Result<(), InspectionError> {
    let reject = |reason: &'static str| InspectionError::InvalidRevision {
        rev: rev.to_string(),
        reason,
    };
    if rev.is_empty() {
        return Err(reject("empty"));
    }
    if rev.len() > MAX_REV_LEN {
        return Err(reject("longer than the accepted maximum"));
    }
    if rev.starts_with('-') {
        return Err(reject("option-shaped"));
    }
    if rev.chars().any(char::is_whitespace) {
        return Err(reject("contains whitespace"));
    }
    if rev.chars().any(|c| !is_revision_char(c)) {
        return Err(reject("contains a character outside the accepted set"));
    }
    if rev.contains("//")
        || rev.contains("..")
        || rev.starts_with('/')
        || rev.ends_with('/')
        || rev.ends_with('.')
        || rev.ends_with(".lock")
    {
        return Err(reject("contains a path-shaped or lock-shaped component"));
    }
    Ok(())
}

fn is_revision_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '.' | '_' | '-' | '/' | '@' | '^' | '~' | '{' | '}' | ':' | '+'
        )
}

/// Validates a path argument for an inspection.
fn validate_path(path: &str) -> Result<(), InspectionError> {
    let reject = |reason: &'static str| InspectionError::InvalidPath {
        path: path.to_string(),
        reason,
    };
    if path.is_empty() {
        return Err(reject("empty"));
    }
    if path.starts_with('-') {
        return Err(reject("option-shaped"));
    }
    if Path::new(path).is_absolute() {
        return Err(reject("absolute"));
    }
    if path.chars().any(char::is_whitespace) {
        return Err(reject("contains whitespace"));
    }
    for component in Path::new(path).components() {
        match component {
            std::path::Component::Normal(_) => {}
            _ => return Err(reject("contains an escaping or non-normal component")),
        }
    }
    Ok(())
}

/// Builds the spawn configuration for one inspection.
///
/// Exposed so a caller, and a test, can see exactly what would run: a direct
/// request for `git` with the hardening flags, never a shell.
pub fn inspection_config(
    workspace_root: &Path,
    inspection: &Inspection,
    limit: usize,
) -> Result<SpawnConfig, InspectionError> {
    let arguments = inspection_arguments(inspection)?;
    Ok(SpawnConfig::new(
        ProcessRequest::direct("git", arguments),
        workspace_root.to_path_buf(),
        hardened_git_environment(),
    )
    .with_capture_limit(limit))
}

/// Runs one inspection in the owned process group.
///
/// `workspace_root` must already be a directory the caller is entitled to read;
/// path arguments are additionally validated to stay inside it.
///
/// # Errors
///
/// Returns [`InspectionError`] if the request was rejected or could not start.
pub fn run_inspection(
    workspace_root: &Path,
    inspection: &Inspection,
    limit: usize,
) -> Result<ProcessOutcome, InspectionError> {
    let config = inspection_config(workspace_root, inspection, limit)?;
    let mut process = super::process::spawn(config).map_err(InspectionError::Spawn)?;
    process
        .run_to_completion(
            std::time::Duration::from_secs(30),
            std::time::Duration::from_secs(2),
            std::time::Duration::from_secs(2),
        )
        .map_err(|refusal| {
            InspectionError::Spawn(SpawnError::SpawnFailed {
                message: refusal.to_string(),
            })
        })
}

/// Runs an inspection and returns stdout as a string when it succeeded.
///
/// # Errors
///
/// Returns [`InspectionError`] if the request was rejected, could not start, or
/// the Git process reported a failure.
pub fn inspect(workspace_root: &Path, inspection: &Inspection) -> Result<String, InspectionError> {
    let outcome = run_inspection(workspace_root, inspection, DEFAULT_INSPECTION_LIMIT)?;
    if outcome.termination != TerminationMode::Exited {
        return Err(InspectionError::Spawn(SpawnError::SpawnFailed {
            message: format!(
                "inspection did not exit normally: {:?}",
                outcome.termination
            ),
        }));
    }
    let ok = outcome.status.map(|s| s.success()).unwrap_or(false);
    if !ok {
        return Err(InspectionError::Spawn(SpawnError::SpawnFailed {
            message: format!(
                "git reported a failure: {}",
                outcome.capture.stderr_string().trim()
            ),
        }));
    }
    Ok(outcome.capture.stdout_string())
}

#[cfg(test)]
#[path = "git_inspect_tests.rs"]
mod tests;
