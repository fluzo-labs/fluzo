//! AGT-02 P2: deterministic discovery over a real confined workspace.
//!
//! Each test runs against a real temporary Git repository so the
//! tracked-only rule is exercised against Git's own index rather than a
//! stand-in, and the secret-exclusion rule is exercised against files that
//! really exist and really are tracked.

use std::path::{Path, PathBuf};
use std::process::Command;

use fluzo_core::context::{
    ContextBudget, ContextError, OmissionReason, Retention, SkillDescriptor, SourceKind, SourceRef,
    TrustTier,
};
use fluzo_core::settings::ContextSettings;

use super::context::*;

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

struct Repo {
    root: PathBuf,
}

impl Repo {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "fluzo-context-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&root).expect("fixture base");
        git(&root, &["init", "-q", "."]);
        git(&root, &["config", "user.email", "fixture@example.invalid"]);
        git(&root, &["config", "user.name", "fixture"]);
        Self { root }
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent dirs");
        }
        std::fs::write(path, content).expect("write fixture file");
    }

    fn commit(&self, message: &str) {
        git(&self.root, &["add", "-A"]);
        git(&self.root, &["commit", "-qm", message]);
    }

    fn remove_worktree_file(&self, relative: &str) {
        std::fs::remove_file(self.root.join(relative)).expect("remove fixture file");
    }

    fn touch(&self, relative: &str, content: &str) {
        std::fs::write(self.root.join(relative), content).expect("modify fixture file");
    }
}

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .expect("git runs");
    assert!(output.status.success(), "git {args:?}: {output:?}");
}

/// A repository with root, nested and sibling instruction files, a README,
/// an ignored file and tracked secret-bearing files.
fn standard_repo(tag: &str) -> Repo {
    let repo = Repo::new(tag);
    repo.write(".gitignore", "ignored/\n");
    repo.write("AGENTS.md", "root instructions\n");
    repo.write("crates/AGENTS.md", "crates instructions\n");
    repo.write("crates/core/AGENTS.md", "core instructions\n");
    repo.write("README.md", "# Fluzo\n\nproject overview\n");
    repo.write("crates/core/src/lib.rs", "pub fn thing() {}\n");
    // Tracked but secret-bearing: must never enter automatic context.
    repo.write(".env", "API_KEY=sk-should-never-appear\n");
    repo.write("deploy/tls.pem", "-----BEGIN PRIVATE KEY-----\n");
    repo.write("id_rsa", "ssh private key material\n");
    repo.commit("baseline");
    // Ignored file that exists on disk and would otherwise look readable.
    repo.write("ignored/notes.md", "ignored notes\n");
    // Never-added instruction file: untracked, so not read automatically.
    repo.write("extras/AGENTS.md", "extras instructions\n");
    repo
}

fn settings() -> ContextSettings {
    ContextSettings::default()
}

fn roomy_budget() -> ContextBudget {
    ContextBudget::new(100_000, 4_096, 512, 1_024)
}

fn request(target: Option<&str>) -> DiscoveryRequest {
    DiscoveryRequest {
        task_id: "t-1".to_owned(),
        user_request: "add a recovery path to the core crate".to_owned(),
        target_paths: target.into_iter().map(str::to_owned).collect(),
        skills: Vec::new(),
    }
}

fn run(repo: &Repo, req: DiscoveryRequest) -> DiscoveredContext {
    discover(&repo.root, &req, roomy_budget(), &settings()).expect("discover")
}

fn identities(ctx: &DiscoveredContext) -> Vec<String> {
    ctx.pack
        .included
        .iter()
        .map(|item| item.source.identity.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Rule units
// ---------------------------------------------------------------------------

#[test]
fn secret_detection_covers_names_prefixes_and_suffixes() {
    assert!(is_secret_bearing(".env"));
    assert!(is_secret_bearing("deploy/.env"));
    assert!(is_secret_bearing(".npmrc"));
    assert!(is_secret_bearing("auth.json"));
    assert!(is_secret_bearing("deploy/tls.pem"));
    assert!(is_secret_bearing("keys/server.key"));
    assert!(is_secret_bearing("id_rsa"));
    assert!(is_secret_bearing("deploy/id_ed25519"));
    assert!(is_secret_bearing("aws_access_key.txt"));
}

#[test]
fn ordinary_files_are_not_secret_bearing() {
    assert!(!is_secret_bearing("AGENTS.md"));
    assert!(!is_secret_bearing("README.md"));
    assert!(!is_secret_bearing("crates/core/src/lib.rs"));
    assert!(!is_secret_bearing("keymap.toml"), "'key' is not a key file");
    assert!(!is_secret_bearing("credentials.txt.bak"));
    assert!(!is_secret_bearing(""));
    assert!(
        !is_secret_bearing("src/env.rs"),
        "a source file named env is not .env"
    );
}

#[test]
fn instruction_files_apply_root_outward_for_a_target() {
    assert_eq!(
        applicable_instruction_files(Some("crates/core/src/lib.rs")),
        vec![
            "AGENTS.md",
            "crates/AGENTS.md",
            "crates/core/AGENTS.md",
            "crates/core/src/AGENTS.md",
        ]
    );
}

#[test]
fn without_a_target_only_the_root_instruction_file_applies() {
    assert_eq!(applicable_instruction_files(None), vec!["AGENTS.md"]);
    assert_eq!(
        applicable_instruction_files(Some("README.md")),
        vec!["AGENTS.md"]
    );
}

// ---------------------------------------------------------------------------
// The recipe
// ---------------------------------------------------------------------------

#[test]
fn the_recipe_produces_request_version_instructions_readme_and_git_evidence() {
    let repo = standard_repo("recipe");
    let ctx = run(&repo, request(Some("crates/core/src/lib.rs")));
    let ids = identities(&ctx);

    assert!(ids.contains(&"task:t-1".to_owned()), "the request: {ids:?}");
    assert!(
        ids.contains(&"workspace:revision".to_owned()),
        "the observed version: {ids:?}"
    );
    assert!(ids.contains(&"AGENTS.md".to_owned()));
    assert!(ids.contains(&"crates/AGENTS.md".to_owned()));
    assert!(ids.contains(&"crates/core/AGENTS.md".to_owned()));
    assert!(ids.contains(&"README.md".to_owned()));
    assert!(ids.contains(&"git:status".to_owned()));
}

#[test]
fn the_request_is_user_intent_and_the_observed_version_is_runtime_authority() {
    let repo = standard_repo("tiers");
    let ctx = run(&repo, request(Some("crates/core/src/lib.rs")));
    let by_id: std::collections::BTreeMap<&str, (TrustTier, Retention)> = ctx
        .pack
        .included
        .iter()
        .map(|i| (i.source.identity.as_str(), (i.tier, i.retention)))
        .collect();

    assert_eq!(
        by_id["task:t-1"],
        (TrustTier::UserIntent, Retention::Required),
        "the request is required user intent"
    );
    assert_eq!(
        by_id["workspace:revision"],
        (TrustTier::RuntimeAuthority, Retention::Required),
        "the observed version is runtime-owned, never summary-restatable"
    );
    assert_eq!(
        by_id["AGENTS.md"],
        (TrustTier::Instruction, Retention::Required),
        "applicable instructions are required"
    );
    assert_eq!(
        by_id["README.md"],
        (TrustTier::RepositoryContent, Retention::Optional),
        "the overview is nice to have, not required"
    );
}

#[test]
fn only_runtime_records_are_authoritative_in_a_discovered_pack() {
    let repo = standard_repo("authority");
    let ctx = run(&repo, request(Some("crates/core/src/lib.rs")));
    let authoritative = ctx.pack.authoritative_sources();
    let identities: Vec<&str> = authoritative
        .iter()
        .map(|source| source.identity.as_str())
        .collect();
    assert_eq!(
        identities,
        vec!["workspace:revision"],
        "instructions, repository content and the request grant nothing"
    );
}

#[test]
fn discovery_is_deterministic_across_repeated_runs() {
    let repo = standard_repo("determinism");
    let req = request(Some("crates/core/src/lib.rs"));
    let first = run(&repo, req.clone());
    for _ in 0..4 {
        assert_eq!(
            run(&repo, req.clone()),
            first,
            "same workspace and request must give the same pack"
        );
    }
}

#[test]
fn every_git_evidence_item_carries_the_observed_revision() {
    let repo = standard_repo("versions");
    repo.touch("crates/core/src/lib.rs", "pub fn thing() {}\n// changed\n");
    let ctx = run(&repo, request(Some("crates/core/src/lib.rs")));
    let revision = ctx.observed_revision.clone().expect("revision observed");
    assert_eq!(revision.len(), 40, "a full git object id");

    for item in &ctx.pack.included {
        if item.source.identity.starts_with("git:") || item.source.identity == "workspace:revision"
        {
            assert_eq!(
                item.source.observed_version.as_deref(),
                Some(revision.as_str()),
                "{} must record the revision it was read at",
                item.source.identity
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Exclusion rules
// ---------------------------------------------------------------------------

#[test]
fn tracked_secret_bearing_files_never_enter_automatic_context() {
    let repo = standard_repo("secrets");
    // A modification puts the secret value in the worktree diff, which is
    // evidence the recipe really does read. Tracked-only cannot help here:
    // the file is tracked.
    repo.touch(
        ".env",
        "API_KEY=sk-should-never-appear\nROTATED=also-secret\n",
    );
    repo.touch("deploy/tls.pem", "-----BEGIN PRIVATE KEY-----\nLEAKED\n");
    let ctx = run(&repo, request(Some("crates/core/src/lib.rs")));

    assert!(
        ctx.redacted.iter().any(|p| p == ".env"),
        "the tracked .env is pulled out of git evidence: {:?}",
        ctx.redacted
    );
    assert!(
        ctx.redacted.iter().any(|p| p.ends_with("tls.pem")),
        "the tracked key file is pulled out of git evidence: {:?}",
        ctx.redacted
    );

    let leaked = identities(&ctx)
        .into_iter()
        .filter(|id| id.contains(".env") || id.contains(".pem") || id.contains("id_rsa"))
        .collect::<Vec<_>>();
    assert!(
        leaked.is_empty(),
        "no secret-bearing source in the pack: {leaked:?}"
    );

    let body = ctx
        .pack
        .included
        .iter()
        .map(|i| i.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !body.contains("sk-should-never-appear"),
        "secret value must not reach context content"
    );
    assert!(!body.contains("ROTATED"));
    assert!(!body.contains("BEGIN PRIVATE KEY"));
    assert!(!body.contains("LEAKED"));
    assert!(!body.contains("ssh private key material"));
}

#[test]
fn redaction_leaves_ordinary_diff_and_status_evidence_intact() {
    let repo = standard_repo("redaction-control");
    // The control half: without a secret in the diff, nothing is redacted
    // and the evidence survives. Proves the previous test passes because of
    // redaction, not because the diff was empty all along.
    repo.touch("crates/core/src/lib.rs", "pub fn thing() {}\n// changed\n");
    let ctx = run(&repo, request(Some("crates/core/src/lib.rs")));
    assert!(ctx.redacted.is_empty(), "nothing secret, nothing removed");
    let ids = identities(&ctx);
    assert!(ids.contains(&"git:status".to_owned()));
    assert!(ids.contains(&"git:diff:worktree".to_owned()));
    let diff = ctx
        .pack
        .included
        .iter()
        .find(|i| i.source.identity == "git:diff:worktree")
        .expect("worktree diff present");
    println!("WORKTREE_DIFF_CONTENT={:?}", diff.content);
    assert!(
        diff.content.contains("changed"),
        "the real change is still there"
    );
}

#[test]
fn ignored_files_are_not_read_because_they_are_not_tracked() {
    let repo = standard_repo("ignored");
    // The file really exists on disk; only the index says it is not tracked.
    assert!(repo.root.join("ignored/notes.md").exists());
    let ctx = run(&repo, request(None));
    let ids = identities(&ctx);
    assert!(
        !ids.iter().any(|id| id.contains("ignored/notes.md")),
        "ignored material must not appear: {ids:?}"
    );
}

#[test]
fn an_untracked_instruction_file_is_not_read_automatically() {
    let repo = standard_repo("untracked-instructions");
    let ctx = run(&repo, request(Some("extras/deep/file.rs")));
    let considered: std::collections::BTreeMap<&str, FileDecision> = ctx
        .considered
        .iter()
        .map(|c| (c.path.as_str(), c.decision.clone()))
        .collect();
    assert_eq!(
        considered.get("extras/AGENTS.md").cloned(),
        Some(FileDecision::SkippedNotTracked),
        "never-added instructions are out by the tracked-only rule"
    );
}

#[test]
fn a_tracked_instruction_deleted_from_the_worktree_is_recorded_as_missing() {
    let repo = standard_repo("deleted-instruction");
    repo.remove_worktree_file("crates/AGENTS.md");
    let ctx = run(&repo, request(Some("crates/core/src/lib.rs")));
    let considered: std::collections::BTreeMap<&str, FileDecision> = ctx
        .considered
        .iter()
        .map(|c| (c.path.as_str(), c.decision.clone()))
        .collect();
    assert_eq!(
        considered.get("crates/AGENTS.md").cloned(),
        Some(FileDecision::SkippedMissing),
        "deleted-but-tracked is visible as missing, not silently absent"
    );
    // The deeper instruction file still applies.
    assert!(identities(&ctx).contains(&"crates/core/AGENTS.md".to_owned()));
}

// ---------------------------------------------------------------------------
// Bounding
// ---------------------------------------------------------------------------

#[test]
fn oversized_context_content_is_truncated_at_the_configured_ceiling() {
    let repo = Repo::new("truncation");
    repo.write(".gitignore", "/placeholder\n");
    let huge = "z".repeat(settings().tool_context_max_bytes as usize * 3);
    repo.write("README.md", &huge);
    repo.commit("baseline");

    let ctx = run(&repo, request(None));
    let readme = ctx
        .pack
        .included
        .iter()
        .find(|i| i.source.identity == "README.md")
        .expect("readme included");
    assert!(readme.truncated, "the ceiling was declared and applied");
    assert_eq!(
        readme.content.len(),
        settings().tool_context_max_bytes as usize,
        "cut exactly at the typed setting"
    );
}

#[test]
fn required_content_that_cannot_fit_blocks_rather_than_dropping_instructions() {
    let repo = standard_repo("tight-budget");
    // Effective input 30: request (10) + version (18) = 28 fits, adding
    // the first instruction file (5) does not.
    let tight = ContextBudget::new(4_096, 1_024, 512, 2_530);
    let error = discover(
        &repo.root,
        &request(Some("crates/core/src/lib.rs")),
        tight,
        &settings(),
    )
    .expect_err("must not silently drop instructions");
    match error {
        DiscoveryError::Context(ContextError::RequiredDoesNotFit {
            identity,
            estimated_tokens,
            effective_input,
        }) => {
            assert_eq!(
                identity, "AGENTS.md",
                "the first required item that did not fit"
            );
            assert_eq!(
                effective_input, 30,
                "the effective input the item was measured against"
            );
            assert!(estimated_tokens > 0);
        }
        other => panic!("expected a required-does-not-fit refusal, got {other:?}"),
    }
}

#[test]
fn optional_git_evidence_is_recorded_as_omitted_when_it_cannot_fit() {
    let repo = standard_repo("omitted-evidence");
    repo.touch("crates/core/src/lib.rs", &"m".repeat(4_000));
    // Effective input 45: the required items total 43, so the README and
    // all Git evidence overflow.
    let tight = ContextBudget::new(12_000, 1_024, 512, 10_419);
    let ctx = discover(
        &repo.root,
        &request(Some("crates/core/src/lib.rs")),
        tight,
        &settings(),
    )
    .expect("pack fits the required items");

    assert!(
        ctx.pack
            .included
            .iter()
            .all(|i| i.retention == Retention::Required),
        "only required items survived"
    );
    let omitted: Vec<&str> = ctx
        .pack
        .omitted
        .iter()
        .map(|o| o.source.identity.as_str())
        .collect();
    assert!(
        omitted.contains(&"git:status") || omitted.contains(&"git:diff:worktree"),
        "git evidence omitted with a recorded reason: {omitted:?}"
    );
    assert!(
        ctx.pack
            .omitted
            .iter()
            .all(|o| o.reason == OmissionReason::BudgetExceeded)
    );
}

// ---------------------------------------------------------------------------
// Skills
// ---------------------------------------------------------------------------

fn skill(id: &str) -> SkillDescriptor {
    SkillDescriptor::new(
        id,
        SourceRef::new(
            SourceKind::Skill,
            format!("https://example.invalid/{id}/SKILL.md@abc123"),
        ),
        format!("{id} instructions"),
    )
}

#[test]
fn no_skills_requested_means_no_skill_content_in_the_pack() {
    let repo = standard_repo("no-skills");
    let ctx = run(&repo, request(None));
    assert!(
        !identities(&ctx).iter().any(|id| id.contains("SKILL.md")),
        "nothing selected, nothing loaded"
    );
}

#[test]
fn explicitly_selected_skills_enter_as_instruction_tier_with_their_source() {
    let repo = standard_repo("with-skills");
    let mut req = request(None);
    req.skills = vec![skill("rust"), skill("testing")];
    let ctx = run(&repo, req);

    let skills: Vec<&str> = ctx
        .pack
        .included
        .iter()
        .filter(|i| i.source.kind == SourceKind::Skill)
        .map(|i| i.source.identity.as_str())
        .collect();
    assert_eq!(
        skills,
        vec![
            "https://example.invalid/rust/SKILL.md@abc123",
            "https://example.invalid/testing/SKILL.md@abc123"
        ]
    );
    for item in ctx
        .pack
        .included
        .iter()
        .filter(|i| i.source.kind == SourceKind::Skill)
    {
        assert_eq!(item.tier, TrustTier::Instruction);
        assert_eq!(item.retention, Retention::Required);
        assert!(!item.tier.can_authorize());
    }
}

#[test]
fn skill_content_never_grants_authority_even_beside_real_grants() {
    let repo = standard_repo("skill-authority");
    let mut req = request(None);
    req.skills = vec![skill("shell-tricks")];
    let ctx = run(&repo, req);
    assert_eq!(
        ctx.pack
            .authoritative_sources()
            .iter()
            .map(|s| s.identity.as_str())
            .collect::<Vec<_>>(),
        vec!["workspace:revision"],
        "a skill's prose is never a grant"
    );
}

// ---------------------------------------------------------------------------
// Budget derivation
// ---------------------------------------------------------------------------

#[test]
fn the_safety_reserve_combines_the_floor_and_the_window_fraction() {
    let settings = ContextSettings::default();
    let budget = budget_from_settings(32_768, 4_096, 512, &settings).expect("budget");
    let expected_fraction = (32_768.0 * settings.safety_reserve_fraction) as u64;
    assert_eq!(
        budget.safety_reserve,
        settings.safety_reserve_tokens + expected_fraction
    );
    assert_eq!(
        budget.effective_input(),
        32_768 - 4_096 - 512 - budget.safety_reserve
    );
    assert!(budget.effective_input() > 0);
}

#[test]
fn a_window_swallowed_by_reserves_is_refused_before_any_read() {
    let settings = ContextSettings::default();
    let error = budget_from_settings(1_000, 900, 200, &settings).expect_err("no input room left");
    assert!(matches!(
        error,
        fluzo_core::context::BudgetError::ReservesLeaveNoRoom { .. }
    ));
}

#[test]
fn a_zero_window_is_refused_rather_than_treated_as_unlimited() {
    let settings = ContextSettings::default();
    assert_eq!(
        budget_from_settings(0, 100, 10, &settings),
        Err(fluzo_core::context::BudgetError::WindowNotPositive { model_window: 0 })
    );
}
