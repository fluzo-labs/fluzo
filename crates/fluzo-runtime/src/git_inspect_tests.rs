//! Read-only Git inspection tests.
//!
//! The central question is whether repository configuration can turn a read into
//! a program launch. Each sentinel test has a control half that proves the
//! sentinel really fires when the hardening is absent. Without the control the
//! hardened assertion could pass for the wrong reason.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::*;

/// A repository fixture with configuration-driven helpers installed.
struct Fixture {
    root: PathBuf,
    ext_diff_ran: PathBuf,
    fsmonitor_ran: PathBuf,
    /// Marker touched by the script the fake *user* configuration pulls in by
    /// `[include]`. Its presence means user config reached the invocation.
    user_config_ran: PathBuf,
    fake_global_config: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        Self::build(tag, true)
    }

    /// A fixture whose repository does **not** set `diff.external`, so a value
    /// coming from user-level configuration is the only candidate and its
    /// precedence can be observed.
    fn without_repo_external_diff(tag: &str) -> Self {
        Self::build(tag, false)
    }

    fn build(tag: &str, set_repo_external_diff: bool) -> Self {
        let base = std::env::temp_dir().join(format!(
            "fluzo-git-inspect-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&base).expect("fixture base");
        let root = base.join("repo");
        git_at(&root, &["init", "-q", "."]);
        git_at(&root, &["config", "user.email", "fixture@example.invalid"]);
        git_at(&root, &["config", "user.name", "fixture"]);
        std::fs::write(root.join("tracked.txt"), "original\n").expect("write tracked");
        git_at(&root, &["add", "-A"]);
        git_at(&root, &["commit", "-qm", "fixture baseline"]);
        // A pending change so diff-based inspections have something to look at.
        std::fs::write(root.join("tracked.txt"), "modified\n").expect("modify tracked");

        let ext_diff_ran = base.join("EXTDIFF_RAN");
        let fsmonitor_ran = base.join("FSMON_RAN");
        let user_sentinel = base.join("user-sentinel.sh");
        let included_config = base.join("included-config");
        let fake_global_config = base.join("fake-global.gitconfig");
        let user_config_ran = base.join("FROM_USER_CONFIG_RAN");

        write_sentinel(
            &root.join("sent-ext.sh"),
            &ext_diff_ran,
            "ext-diff output\n",
        );
        write_sentinel(&root.join("sent-fsm.sh"), &fsmonitor_ran, "");
        write_sentinel(&user_sentinel, &user_config_ran, "user cfg diff\n");
        // A config file (not a script) that names the sentinel as the external
        // diff, so the include chain is a real configuration path to a program.
        std::fs::write(
            &included_config,
            format!("[diff]\n\texternal = {}\n", user_sentinel.display()),
        )
        .expect("included config");

        // Repository-supplied helpers: exactly what an untrusted repo could set.
        if set_repo_external_diff {
            git_at(
                &root,
                &[
                    "config",
                    "diff.external",
                    root.join("sent-ext.sh").to_str().unwrap(),
                ],
            );
        }
        git_at(
            &root,
            &[
                "config",
                "core.fsmonitor",
                root.join("sent-fsm.sh").to_str().unwrap(),
            ],
        );
        git_at(
            &root,
            &["config", "core.pager", "/nonexistent-sentinel-pager"],
        );

        // A fake user configuration that pulls in an external diff by include,
        // so "the user's config is not read" is testable through something that
        // would otherwise execute.
        std::fs::write(
            &fake_global_config,
            format!(
                "[include]\n\tpath = {}\n[core]\n\tpager = {}\n",
                included_config.display(),
                root.join("sent-ext.sh").display()
            ),
        )
        .expect("fake global config");

        Self {
            root,
            ext_diff_ran,
            fsmonitor_ran,
            user_config_ran,
            fake_global_config,
        }
    }

    fn cleanup(&self) {
        let base = self.root.parent().expect("fixture has a parent");
        let _ = std::fs::remove_dir_all(base);
    }
}

fn write_sentinel(path: &Path, touch_target: &Path, output: &str) {
    let script = format!(
        "#!/bin/sh\ntouch {}\nprintf '%s' '{}'\n",
        touch_target.display(),
        output
    );
    std::fs::write(path, script).expect("sentinel written");
    #[allow(unused_mut)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)
            .expect("sentinel meta")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).expect("sentinel executable");
    }
}

/// Test scaffolding only: sets up a repository with plain git.
fn git_at(dir: &Path, args: &[&str]) {
    std::fs::create_dir_all(dir).ok();
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("HOME", dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git available for fixture setup");
    assert!(status.success(), "fixture git {args:?} failed");
}

/// Control: run git with the repository's own helpers and nothing overriding
/// them, to prove the sentinel is live.
fn unhardened_git(root: &Path, args: &[&str], home_override: &Path) -> bool {
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("HOME", home_override)
        .env_remove("GIT_CONFIG_GLOBAL")
        .env_remove("GIT_CONFIG_NOSYSTEM")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git available");
    status.success()
}

fn ran(marker: &Path) -> bool {
    marker.exists()
}

// ---------------------------------------------------------------------------
// The closed set
// ---------------------------------------------------------------------------

#[test]
fn every_inspection_maps_to_a_read_only_git_subcommand() {
    let inspections = all_inspections();
    let read_only = ["status", "ls-files", "diff", "log", "show", "rev-parse"];
    for inspection in &inspections {
        let arguments = inspection_arguments(inspection).expect("arguments build");
        // Skip the leading `-c <key=value>` pairs to reach the subcommand.
        let mut index = 0;
        while index < arguments.len() {
            if arguments[index] == "-c" {
                index += 2;
                continue;
            }
            break;
        }
        let actual = arguments.get(index).cloned().unwrap_or_default();
        assert!(
            read_only.contains(&actual.as_str()),
            "{inspection:?} produced the subcommand {actual:?}, which is not read-only"
        );
    }
}

#[test]
fn every_inspection_carries_the_config_precedence_overrides() {
    for inspection in all_inspections() {
        let arguments = inspection_arguments(&inspection).expect("arguments build");
        let joined = arguments.join(" ");
        for required in [
            "-c core.pager=cat",
            "-c core.fsmonitor=false",
            "-c core.hooksPath=/dev/null",
            "-c credential.helper=",
        ] {
            assert!(
                joined.contains(required),
                "{inspection:?} is missing the override {required:?}"
            );
        }
    }
}

#[test]
fn an_inspection_is_a_direct_request_and_never_a_shell() {
    let fixture = Fixture::new("shape");
    for inspection in all_inspections() {
        let config = inspection_config(&fixture.root, &inspection, 4096).expect("config builds");
        assert!(
            !config.request.uses_shell(),
            "{inspection:?} must not run through a shell"
        );
        match &config.request {
            ProcessRequest::Direct { executable, .. } => assert_eq!(executable, "git"),
            other => panic!("{inspection:?} built a non-direct request: {other:?}"),
        }
    }
    fixture.cleanup();
}

fn all_inspections() -> Vec<Inspection> {
    vec![
        Inspection::Status,
        Inspection::TrackedFiles,
        Inspection::Diff,
        Inspection::StagedDiff,
        Inspection::Log { max_count: 5 },
        Inspection::Show {
            rev: "HEAD".to_string(),
        },
        Inspection::RevParse {
            rev: "HEAD".to_string(),
        },
        Inspection::DiffPath {
            path: "tracked.txt".to_string(),
        },
    ]
}

// ---------------------------------------------------------------------------
// Hardened environment
// ---------------------------------------------------------------------------

#[test]
fn the_hardened_environment_pins_config_prompting_and_pager() {
    let environment = hardened_git_environment();
    let entries = environment.entries();
    assert_eq!(
        entries.get("GIT_CONFIG_GLOBAL").map(String::as_str),
        Some(EMPTY_CONFIG_PATH)
    );
    assert_eq!(
        entries.get("GIT_CONFIG_SYSTEM").map(String::as_str),
        Some("/dev/null")
    );
    assert_eq!(
        entries.get("GIT_CONFIG_NOSYSTEM").map(String::as_str),
        Some("1")
    );
    assert_eq!(
        entries.get("GIT_ATTR_NOSYSTEM").map(String::as_str),
        Some("1")
    );
    assert_eq!(entries.get("GIT_PAGER").map(String::as_str), Some("cat"));
    assert_eq!(
        entries.get("GIT_TERMINAL_PROMPT").map(String::as_str),
        Some("0")
    );
    assert!(entries.contains_key("GIT_EXTERNAL_DIFF"));
    assert_eq!(entries.get("GIT_COLORS").map(String::as_str), Some("never"));
}

#[test]
fn the_hardened_environment_carries_no_credential_channel() {
    let hardened = hardened_git_environment();
    let entries = hardened.entries();
    for key in ["GIT_SSH_COMMAND", "GIT_SSH", "GIT_ASKPASS", "SSH_AUTH_SOCK"] {
        assert!(
            !entries.contains_key(key),
            "{key} must not be present in a hardened inspection environment"
        );
    }
}

// ---------------------------------------------------------------------------
// Sentinel helpers: control first, then the hardened run
// ---------------------------------------------------------------------------

#[test]
fn control_repository_external_diff_really_runs_without_hardening() {
    let fixture = Fixture::new("ctl-ext");
    // The repo names an external diff. With nothing overriding it, git runs it.
    unhardened_git(&fixture.root, &["diff"], &fixture.root);
    assert!(
        ran(&fixture.ext_diff_ran),
        "the control failed: the repository's external diff never ran, so the \
         hardened test below would prove nothing"
    );
    fixture.cleanup();
}

#[test]
fn inspection_does_not_run_the_repository_external_diff() {
    let fixture = Fixture::new("hard-ext");
    // Same repository state, hardened invocation.
    for inspection in [
        Inspection::Diff,
        Inspection::DiffPath {
            path: "tracked.txt".into(),
        },
    ] {
        let _ = run_inspection(&fixture.root, &inspection, 64 * 1024).expect("inspection runs");
        assert!(
            !ran(&fixture.ext_diff_ran),
            "the repository's external diff ran during {inspection:?}"
        );
    }
    fixture.cleanup();
}

#[test]
fn control_repository_fsmonitor_really_runs_without_hardening() {
    let fixture = Fixture::new("ctl-fsm");
    unhardened_git(&fixture.root, &["status"], &fixture.root);
    assert!(
        ran(&fixture.fsmonitor_ran),
        "the control failed: the repository's fsmonitor never ran"
    );
    fixture.cleanup();
}

#[test]
fn inspection_does_not_run_the_repository_fsmonitor() {
    let fixture = Fixture::new("hard-fsm");
    for inspection in [Inspection::Status, Inspection::TrackedFiles] {
        let _ = run_inspection(&fixture.root, &inspection, 64 * 1024).expect("inspection runs");
        assert!(
            !ran(&fixture.fsmonitor_ran),
            "the repository's fsmonitor ran during {inspection:?}"
        );
    }
    fixture.cleanup();
}

#[test]
fn control_user_configuration_really_reaches_git() {
    let fixture = Fixture::without_repo_external_diff("ctl-user");
    // A fake user config that pulls in an external diff by [include]. With the
    // global config pointed at it, git really does use it.
    let home = fixture.root.parent().unwrap().join("fakehome");
    std::fs::create_dir_all(&home).expect("fake home");
    std::fs::copy(&fixture.fake_global_config, home.join(".gitconfig"))
        .expect("install fake global");
    let status = Command::new("git")
        .args(["diff"])
        .current_dir(&fixture.root)
        .env("HOME", &home)
        // Set the global config directly rather than relying on HOME: git also
        // consults XDG paths, so HOME alone does not pin which file is read.
        .env("GIT_CONFIG_GLOBAL", &fixture.fake_global_config)
        .env_remove("GIT_CONFIG_NOSYSTEM")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git available");
    let _ = status;
    assert!(
        ran(&fixture.user_config_ran),
        "the control failed: the included user-config helper never ran, so the \
         hardened test below would prove nothing"
    );
    fixture.cleanup();
}

#[test]
fn inspection_does_not_read_the_user_configuration() {
    let fixture = Fixture::new("hard-user");
    // Even with a fake home present, the hardened environment points the global
    // config at the empty device, so nothing in it is consulted.
    let hardened = hardened_git_environment();
    let entries = hardened.entries();
    assert_eq!(
        entries.get("GIT_CONFIG_GLOBAL").map(String::as_str),
        Some("/dev/null"),
        "the user configuration must be redirected away, not merely left unset"
    );
    let _ = run_inspection(&fixture.root, &Inspection::Diff, 64 * 1024).expect("inspection runs");
    assert!(
        !ran(&fixture.ext_diff_ran),
        "a helper reachable through user or repository configuration ran anyway"
    );
    fixture.cleanup();
}

// ---------------------------------------------------------------------------
// Read-only guarantee
// ---------------------------------------------------------------------------

#[test]
fn inspection_leaves_the_working_tree_index_and_refs_unchanged() {
    let fixture = Fixture::new("readonly");
    let before = repository_state(&fixture.root);
    for inspection in all_inspections() {
        let _ = run_inspection(&fixture.root, &inspection, 1024 * 1024);
        let after = repository_state(&fixture.root);
        assert_eq!(before, after, "{inspection:?} changed repository state");
    }
    fixture.cleanup();
}

/// HEAD, ref list, porcelain status and the index digest.
fn repository_state(root: &Path) -> (String, String, String, String) {
    let head = capture_git(root, &["rev-parse", "HEAD"]);
    let refs = capture_git(root, &["for-each-ref"]);
    let status = capture_git(root, &["status", "--porcelain=v1"]);
    let index = std::fs::read(root.join(".git/index")).unwrap_or_default();
    let digest = format!("{:02x}", index.len());
    (head, refs, status, digest)
}

fn capture_git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .stdin(Stdio::null())
        .output()
        .expect("git available");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

// ---------------------------------------------------------------------------
// Bounded output
// ---------------------------------------------------------------------------

#[test]
fn inspection_output_is_bounded_and_reports_truncation() {
    let fixture = Fixture::new("bounded");
    // Make a diff far larger than the cap.
    let mut big = String::new();
    for i in 0..4000 {
        big.push_str(&format!("line {i} of a large change\n"));
    }
    std::fs::write(fixture.root.join("tracked.txt"), big).expect("large write");
    let outcome = run_inspection(&fixture.root, &Inspection::Diff, 2048).expect("inspection runs");
    assert_eq!(outcome.capture.stdout.len(), 2048);
    assert!(
        outcome.capture.stdout_truncated,
        "a 4000-line diff against a 2 KiB cap must report truncation"
    );
    fixture.cleanup();
}

// ---------------------------------------------------------------------------
// Argument validation
// ---------------------------------------------------------------------------

#[test]
fn option_shaped_revisions_are_refused() {
    for rev in ["-h", "--help", "--output=/tmp/pwned", "-c", "--ext-diff"] {
        let error = inspection_arguments(&Inspection::Show { rev: rev.into() })
            .expect_err("option-shaped revisions must be refused");
        assert!(
            matches!(error, InspectionError::InvalidRevision { .. }),
            "{rev:?} was accepted: {error:?}"
        );
    }
}

#[test]
fn whitespace_and_control_characters_in_revisions_are_refused() {
    for rev in ["HEAD main", "HEAD\t", "HEAD\n", "ref\nname"] {
        assert!(
            inspection_arguments(&Inspection::RevParse { rev: rev.into() }).is_err(),
            "{rev:?} must be refused"
        );
    }
}

#[test]
fn path_shaped_and_lock_shaped_revisions_are_refused() {
    for rev in [
        "/etc/passwd",
        "refs//heads",
        "../HEAD",
        "refs/heads/x.lock",
        "trailing.",
    ] {
        assert!(
            inspection_arguments(&Inspection::Show { rev: rev.into() }).is_err(),
            "{rev:?} must be refused"
        );
    }
}

#[test]
fn ordinary_revision_expressions_are_accepted() {
    for rev in [
        "HEAD",
        "HEAD~3",
        "HEAD^",
        "abc1234",
        "refs/heads/main",
        "v1.2.3",
        "HEAD@{0}",
        "feature/thing_1",
    ] {
        assert!(
            inspection_arguments(&Inspection::RevParse { rev: rev.into() }).is_ok(),
            "{rev:?} should be a valid revision"
        );
    }
}

#[test]
fn path_arguments_are_validated() {
    for path in [
        "",
        "-p",
        "--output=/tmp/x",
        "/etc/passwd",
        "../outside",
        "./a b.txt",
    ] {
        let error = inspection_arguments(&Inspection::DiffPath { path: path.into() })
            .expect_err("path must be refused");
        assert!(
            matches!(error, InspectionError::InvalidPath { .. }),
            "{path:?} was accepted: {error:?}"
        );
    }
    for path in ["tracked.txt", "src/main.rs", "a/b/c.txt"] {
        assert!(
            inspection_arguments(&Inspection::DiffPath { path: path.into() }).is_ok(),
            "{path:?} should be a valid repository path"
        );
    }
}

#[test]
fn the_log_bound_is_enforced() {
    let error = inspection_arguments(&Inspection::Log {
        max_count: MAX_LOG_COUNT + 1,
    })
    .expect_err("the bound must be enforced");
    assert_eq!(
        error,
        InspectionError::BoundTooLarge {
            requested: MAX_LOG_COUNT + 1,
            maximum: MAX_LOG_COUNT,
        }
    );
    assert!(
        inspection_arguments(&Inspection::Log {
            max_count: MAX_LOG_COUNT
        })
        .is_ok()
    );
}

// ---------------------------------------------------------------------------
// End-to-end against a real repository
// ---------------------------------------------------------------------------

#[test]
fn inspections_return_repository_facts() {
    let fixture = Fixture::new("facts");
    let status = inspect(&fixture.root, &Inspection::Status).expect("status inspects");
    assert!(
        status.contains("tracked.txt"),
        "status did not report the modified file: {status:?}"
    );
    let files = inspect(&fixture.root, &Inspection::TrackedFiles).expect("ls-files inspects");
    assert!(files.contains("tracked.txt"));
    let head = inspect(&fixture.root, &Inspection::RevParse { rev: "HEAD".into() })
        .expect("rev-parse inspects");
    assert!(
        head.trim().len() >= 40,
        "rev-parse did not return an object id: {head:?}"
    );
    let log = inspect(&fixture.root, &Inspection::Log { max_count: 1 }).expect("log inspects");
    assert!(log.contains("fixture baseline"));
    fixture.cleanup();
}

#[test]
fn an_inspection_outside_a_repository_fails_rather_than_hanging() {
    let base =
        std::env::temp_dir().join(format!("fluzo-git-inspect-notarepo-{}", std::process::id()));
    std::fs::create_dir_all(&base).expect("temp dir");
    let result = inspect(&base, &Inspection::Status);
    assert!(result.is_err(), "inspecting a non-repository must fail");
    let _ = std::fs::remove_dir_all(&base);
}
