//! AGT-02 P2: deterministic context discovery over the confined workspace.
//!
//! PRD v0.2 section 16 fixes the initial-context recipe: the user request,
//! the assigned workspace, Git status and the applicable nested `AGENTS.md`
//! instructions. That is what this builds — a fixed recipe, not a
//! relevance score, so the same workspace and request produce the same pack
//! byte for byte.
//!
//! The whole repository is never sent. Anything beyond this recipe arrives
//! later through the policy-gated read/search tools, which is a different
//! authority path from what is assembled here.
//!
//! Two exclusion rules govern every file this module reads automatically:
//!
//! * **Tracked only.** A candidate must appear in `git ls-files`. Ignored
//!   and never-added files are therefore out by construction rather than by
//!   a pattern list that can drift.
//! * **Secret-bearing names excluded**, even when tracked.
//!
//! Both rules are recorded per file in [`DiscoveredContext::considered`],
//! so an operator can see what was looked at and why it was left out.
//!
//! Neither rule is a security boundary. PRD 16 is explicit that explicit
//! reads still require the applicable credential and filesystem policy;
//! these only keep automatic assembly from volunteering sensitive material.

use std::collections::BTreeSet;
use std::path::Path;

use fluzo_core::context::{
    self, ContextBudget, ContextError, ContextItem, ContextPack, Retention, SkillDescriptor,
    SourceKind, SourceRef, TrustTier,
};
use fluzo_core::settings::ContextSettings;

use crate::confinement::{ConfinementError, WorkspaceJail};
use crate::git_inspect::{self, Inspection};

/// The instruction file name recognized at every directory level.
pub const INSTRUCTION_FILE_NAME: &str = "AGENTS.md";
/// The repository overview read as optional context.
pub const README_FILE_NAME: &str = "README.md";

/// Exact file names treated as secret-bearing.
///
/// A closed, reviewable list rather than a heuristic: membership is
/// decidable from the name alone, which is what makes the exclusion
/// assertable. Kept deliberately short and well-known.
pub const SECRET_FILE_NAMES: &[&str] = &[
    ".env",
    ".env.local",
    ".env.production",
    ".npmrc",
    ".yarnrc.yml",
    ".netrc",
    "_netrc",
    ".netrc.gpg",
    ".pgpass",
    ".git-credentials",
    ".aws_credentials",
    ".aws-credentials",
    "auth.json",
    "credentials",
    "credentials.json",
    "credentials.yml",
    "credentials.yaml",
    "service_account.json",
    "secrets.json",
    "secrets.yaml",
    ".fluzo-token",
];

/// Extensions treated as secret-bearing on any file.
pub const SECRET_FILE_SUFFIXES: &[&str] = &[
    ".pem",
    ".key",
    ".p12",
    ".pfx",
    ".jks",
    ".keystore",
    ".crt.key",
    ".asc.key",
    ".pkcs12",
];

/// Name prefixes treated as secret-bearing (SSH and access-key material).
pub const SECRET_NAME_PREFIXES: &[&str] = &[
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "aws_access_key",
    "gcp-service-account",
];

/// Whether a path is excluded from automatic context as secret-bearing.
///
/// Checks the final path component only: the decision is about what the file
/// is, not where it sits, so `crates/x/.env` is excluded exactly like
/// `.env`.
pub fn is_secret_bearing(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name.is_empty() {
        return false;
    }
    if SECRET_FILE_NAMES.contains(&name) {
        return true;
    }
    if SECRET_NAME_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
    {
        return true;
    }
    SECRET_FILE_SUFFIXES
        .iter()
        .any(|suffix| name.len() > suffix.len() && name.ends_with(suffix))
}

/// The instruction files that apply to a target path, root-outward.
///
/// PRD 16: nested `AGENTS.md` files apply within their directory subtree
/// and must be considered before editing there. A target of
/// `crates/core/src/lib.rs` therefore considers the repository root file,
/// then `crates/AGENTS.md`, then `crates/core/AGENTS.md`, then
/// `crates/core/src/AGENTS.md` — the directories the edit actually sits
/// inside. Ordering is root-first so a deeper file is read as a
/// specialization of what came before it.
///
/// With no target, only the repository-root instruction file applies.
pub fn applicable_instruction_files(target: Option<&str>) -> Vec<String> {
    let mut files = vec![INSTRUCTION_FILE_NAME.to_owned()];
    let Some(target) = target else {
        return files;
    };
    let mut prefix = String::new();
    let components: Vec<&str> = target
        .split('/')
        .filter(|component| !component.is_empty())
        .collect();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(component);
        files.push(format!("{prefix}/{INSTRUCTION_FILE_NAME}"));
    }
    files
}

/// A file the recipe looked at and what it decided about it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileDecision {
    /// Read and included as a context item.
    Included,
    /// Not in the Git index, so not read. Covers ignored and never-added.
    SkippedNotTracked,
    /// Matches the secret-bearing rules, so not read.
    SkippedSecret,
    /// Not present in the working tree.
    SkippedMissing,
}

/// One consideration record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConsideredFile {
    pub path: String,
    pub decision: FileDecision,
}

/// What the caller wants context for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryRequest {
    /// Stable task identity, used so the request item can be cited later.
    pub task_id: String,
    /// The user's request text.
    pub user_request: String,
    /// Paths inside the workspace the task targets. Nested instructions are
    /// collected for the first one; empty means repository-wide only.
    pub target_paths: Vec<String>,
    /// Skills the user or authorized configuration explicitly selected,
    /// already resolved through `fluzo_core::context::resolve_explicit`.
    pub skills: Vec<SkillDescriptor>,
}

impl DiscoveryRequest {
    /// The first target path, if any. Instruction nesting follows it.
    pub fn primary_target(&self) -> Option<&str> {
        self.target_paths.first().map(String::as_str)
    }
}

/// Why discovery could not produce a pack.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiscoveryError {
    /// A Git inspection the recipe needs failed.
    Git(String),
    /// The workspace could not be opened under confinement.
    Confinement(String),
    /// An instruction file is present but unreadable. Distinct from missing:
    /// an applicable instruction that exists must be honoured or the failure
    /// surfaced, never treated as if it were absent.
    InstructionUnreadable { path: String, reason: String },
    /// The recipe's required content does not fit the budget.
    Context(ContextError),
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Git(reason) => write!(formatter, "git inspection failed: {reason}"),
            Self::Confinement(reason) => {
                write!(formatter, "workspace confinement failed: {reason}")
            }
            Self::InstructionUnreadable { path, reason } => {
                write!(
                    formatter,
                    "instruction file '{path}' could not be read: {reason}"
                )
            }
            Self::Context(error) => write!(formatter, "context assembly refused: {error}"),
        }
    }
}

/// The result of one discovery pass.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredContext {
    pub pack: ContextPack,
    /// Every file the recipe considered, in consideration order.
    pub considered: Vec<ConsideredFile>,
    /// Paths removed from Git evidence as secret-bearing.
    ///
    /// The tracked-only rule cannot help with a tracked `.env`: the file
    /// really is tracked, and a modification puts its value in the diff.
    /// These are the paths the name rules pulled out of that evidence, so a
    /// redaction is visible rather than inferred from absence.
    pub redacted: Vec<String>,
    /// The Git revision the workspace was observed at, when resolvable.
    pub observed_revision: Option<String>,
}

/// Derive the input budget from the typed settings.
///
/// The safety reserve is the configured absolute floor plus the configured
/// fraction of the window: both knobs exist because a fixed floor alone is
/// meaningless on a large window and a fraction alone leaves no floor on a
/// small one.
pub fn budget_from_settings(
    model_window: u64,
    output_reserve: u64,
    protocol_overhead: u64,
    settings: &ContextSettings,
) -> Result<ContextBudget, fluzo_core::context::BudgetError> {
    let fractional = (model_window as f64 * settings.safety_reserve_fraction) as u64;
    let safety = settings.safety_reserve_tokens.saturating_add(fractional);
    ContextBudget::new(model_window, output_reserve, protocol_overhead, safety).validate()?;
    Ok(ContextBudget::new(
        model_window,
        output_reserve,
        protocol_overhead,
        safety,
    ))
}

/// Read the workspace revision the context is observed at.
///
/// `None` rather than an error when the repository has no commits yet: an
/// empty workspace is a real state the recipe handles, not a failure.
pub fn observed_revision(workspace_root: &Path) -> Option<String> {
    git_inspect::inspect(
        workspace_root,
        &Inspection::RevParse {
            rev: "HEAD".to_owned(),
        },
    )
    .ok()
    .map(|value| value.trim().to_owned())
    .filter(|value| !value.is_empty())
}

/// Assemble the initial context pack for a task.
///
/// Deterministic: the same workspace state, settings and request produce an
/// identical [`DiscoveredContext`], including the consideration records.
pub fn discover(
    workspace_root: &Path,
    request: &DiscoveryRequest,
    budget: ContextBudget,
    settings: &ContextSettings,
) -> Result<DiscoveredContext, DiscoveryError> {
    let jail = WorkspaceJail::open(workspace_root)
        .map_err(|error| DiscoveryError::Confinement(format!("{error:?}")))?;
    let revision = observed_revision(workspace_root);
    let tracked = tracked_files(workspace_root)?;
    let ceiling = settings.tool_context_max_bytes as usize;

    let mut items = Vec::new();
    let mut considered = Vec::new();

    // 1. The user's own request. Required: without it there is no task.
    items.push(ContextItem::new(
        SourceRef::new(SourceKind::UserRequest, format!("task:{}", request.task_id)),
        TrustTier::UserIntent,
        Retention::Required,
        request.user_request.clone(),
    ));

    // 2. The observed workspace version. Runtime authority: this is the
    //    version everything else in the pack was read against, and the value
    //    a later check re-verifies rather than something a summary may
    //    restate.
    if let Some(revision) = revision.as_ref() {
        items.push(ContextItem::new(
            SourceRef::new(SourceKind::RuntimeRecord, "workspace:revision").with_version(revision),
            TrustTier::RuntimeAuthority,
            Retention::Required,
            format!("workspace observed at revision {revision}"),
        ));
    }

    // 3. Applicable nested instructions, root-outward.
    let instruction_paths = applicable_instruction_files(request.primary_target());
    for path in instruction_paths {
        match read_candidate(&jail, &tracked, &path)? {
            Candidate::Read(content) => {
                considered.push(ConsideredFile {
                    path: path.clone(),
                    decision: FileDecision::Included,
                });
                items.push(
                    ContextItem::new(
                        SourceRef::new(SourceKind::InstructionFile, path.clone()).with_version(
                            revision.clone().unwrap_or_else(|| "unversioned".to_owned()),
                        ),
                        TrustTier::Instruction,
                        Retention::Required,
                        content,
                    )
                    .truncated_at(ceiling),
                );
            }
            Candidate::Skipped(decision) => considered.push(ConsideredFile { path, decision }),
        }
    }

    // 4. The repository overview, if it is tracked and not secret-bearing.
    match read_candidate(&jail, &tracked, README_FILE_NAME)? {
        Candidate::Read(content) => {
            considered.push(ConsideredFile {
                path: README_FILE_NAME.to_owned(),
                decision: FileDecision::Included,
            });
            items.push(
                ContextItem::new(
                    SourceRef::new(SourceKind::WorkspaceFile, README_FILE_NAME),
                    TrustTier::RepositoryContent,
                    Retention::Optional,
                    content,
                )
                .truncated_at(ceiling),
            );
        }
        Candidate::Skipped(decision) => considered.push(ConsideredFile {
            path: README_FILE_NAME.to_owned(),
            decision,
        }),
    }

    // 5. Git evidence: status, then staged and unstaged diffs.
    let mut redacted: Vec<String> = Vec::new();
    for (identity, inspection, redactor) in [
        (
            "git:status",
            Inspection::Status,
            redact_status as fn(&str) -> (String, Vec<String>),
        ),
        (
            "git:diff:staged",
            Inspection::StagedDiff,
            redact_diff as fn(&str) -> (String, Vec<String>),
        ),
        (
            "git:diff:worktree",
            Inspection::Diff,
            redact_diff as fn(&str) -> (String, Vec<String>),
        ),
    ] {
        let raw = git_inspect::inspect(workspace_root, &inspection)
            .map_err(|error| DiscoveryError::Git(format!("{identity}: {error:?}")))?;
        let (content, removed) = redactor(&raw);
        redacted.extend(removed);
        if content.trim().is_empty() {
            continue;
        }
        let kind = if identity == "git:status" {
            SourceKind::GitStatus
        } else {
            SourceKind::GitDiff
        };
        let mut source = SourceRef::new(kind, identity);
        if let Some(revision) = revision.as_ref() {
            source = source.with_version(revision);
        }
        items.push(
            ContextItem::new(
                source,
                TrustTier::RepositoryContent,
                Retention::Optional,
                content,
            )
            .truncated_at(ceiling),
        );
    }

    // 6. Explicitly selected skills. Nothing lands here that was not asked
    //    for by id upstream.
    for skill in &request.skills {
        items.push(
            skill
                .clone()
                .into_context_item(Retention::Required, ceiling),
        );
    }

    let pack = context::assemble(budget, items).map_err(DiscoveryError::Context)?;
    redacted.sort();
    redacted.dedup();
    Ok(DiscoveredContext {
        pack,
        considered,
        redacted,
        observed_revision: revision,
    })
}

/// Drop secret-bearing entries from `git status --porcelain=v1 -z` output.
///
/// Entries are `XY <path>`, NUL-separated; a rename adds the old path as
/// its own NUL-delimited token with no status prefix. An entry that does
/// not look like `XY ` is therefore treated as a bare path.
fn redact_status(content: &str) -> (String, Vec<String>) {
    let mut kept = Vec::new();
    let mut removed = Vec::new();
    for entry in content.split('\0') {
        if entry.is_empty() {
            continue;
        }
        let bytes = entry.as_bytes();
        let looks_prefixed = bytes.len() > 3 && bytes[2] == b' ';
        let path = if looks_prefixed { &entry[3..] } else { entry };
        if is_secret_bearing(path) {
            removed.push(path.to_owned());
        } else {
            kept.push(entry);
        }
    }
    (kept.join("\n"), removed)
}

/// Drop secret-bearing file segments from a unified diff.
///
/// Segmentation is on the `diff --git a/<path> b/<path>` header, which is
/// what Git itself uses to delimit a file's hunks. Dropping a whole segment
/// rather than filtering lines keeps the remaining diff well formed: a
/// partially filtered hunk would carry a header for a file whose body was
/// removed, or vice versa.
fn redact_diff(content: &str) -> (String, Vec<String>) {
    let mut kept = String::new();
    let mut removed = Vec::new();
    // `split` drops the separator, so each segment is re-prefixed on the way
    // out. `split_inclusive` would attach the separator to the *previous*
    // segment instead, putting the header at the end of the wrong block and
    // silently discarding every file's body.
    let mut segments = content.split("diff --git ");
    let preamble = segments.next().unwrap_or("");
    if !preamble.trim().is_empty() {
        // Text before the first file header carries no per-file content, so
        // it is kept rather than dropped.
        kept.push_str(preamble);
    }
    for segment in segments {
        let header = segment.lines().next().unwrap_or("");
        let paths: Vec<&str> = header.split_whitespace().skip(1).collect();
        let secret: Vec<&str> = paths
            .iter()
            .copied()
            .filter(|path| is_secret_bearing(path))
            .collect();
        if !secret.is_empty() {
            removed.extend(secret.iter().map(|path| (*path).to_owned()));
            continue;
        }
        kept.push_str("diff --git ");
        kept.push_str(segment);
    }
    (kept, removed)
}

/// The set of paths Git tracks in this workspace.
///
/// `git ls-files -z` separates entries with NUL so that names containing
/// newlines survive; the split must match that, or every path arrives as one
/// unsplit blob and nothing is ever found in the index.
fn tracked_files(workspace_root: &Path) -> Result<BTreeSet<String>, DiscoveryError> {
    let listing = git_inspect::inspect(workspace_root, &Inspection::TrackedFiles)
        .map_err(|error| DiscoveryError::Git(format!("tracked files: {error:?}")))?;
    Ok(listing
        .split(['\0', '\n'])
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

enum Candidate {
    Read(String),
    Skipped(FileDecision),
}

/// Apply the tracked-only and secret-bearing rules, then read through the jail.
fn read_candidate(
    jail: &WorkspaceJail,
    tracked: &BTreeSet<String>,
    path: &str,
) -> Result<Candidate, DiscoveryError> {
    if is_secret_bearing(path) {
        return Ok(Candidate::Skipped(FileDecision::SkippedSecret));
    }
    if !tracked.contains(path) {
        return Ok(Candidate::Skipped(FileDecision::SkippedNotTracked));
    }
    match jail.open_read(path) {
        Ok(mut file) => match file.read_to_string() {
            Ok(content) => Ok(Candidate::Read(content)),
            Err(error) => Err(DiscoveryError::InstructionUnreadable {
                path: path.to_owned(),
                reason: format!("{error:?}"),
            }),
        },
        Err(ConfinementError::NotFound) => Ok(Candidate::Skipped(FileDecision::SkippedMissing)),
        Err(error) => Err(DiscoveryError::Confinement(format!("{path}: {error:?}"))),
    }
}
