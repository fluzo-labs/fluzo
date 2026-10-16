//! RUN-01b: native file confinement for agent-directed paths.
//!
//! The property under test is that a path supplied by an agent cannot
//! reach outside the workspace or touch a protected name, and that a
//! refusal leaves the filesystem alone. The fixtures use a real
//! filesystem in a temp root and real symlinks, because a string-level
//! test would test the wrong thing: the failure mode this jail exists
//! for is a path that *looks* contained.
//!
//! The race cases are tested in their deterministic form. A jail is
//! opened, an intermediate component is then replaced with a symlink by
//! "another process" (the test itself, after the jail exists), and the
//! later open must still refuse. That is the window the design closes,
//! reproduced without needing to win a race.

use crate::confinement::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

struct Root(PathBuf);

impl Root {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir();
        let base = base.canonicalize().unwrap_or(base);
        let path = base.join(format!(
            "fluzo-jail-{tag}-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn jail(root: &Root) -> WorkspaceJail {
    WorkspaceJail::open(&root.0).expect("open jail")
}

// ---------------------------------------------------------------------------
// The contained case
// ---------------------------------------------------------------------------

#[test]
fn a_plain_relative_path_is_written_and_read_back() {
    let root = Root::new("plain");
    let jail = jail(&root);
    let mut file = jail.create("notes.md").expect("create");
    assert_eq!(file.name(), "notes.md");
    file.write_all(b"first line").expect("write");
    file.sync_all().expect("sync");
    let mut reopened = jail.open_read("notes.md").expect("reopen");
    assert_eq!(reopened.read_to_string().expect("read"), "first line");
    assert!(
        root.0.join("notes.md").exists(),
        "the file is in the workspace"
    );
}

#[test]
fn nested_relative_paths_work_below_created_directories() {
    let root = Root::new("nested");
    let jail = jail(&root);
    let dir = jail.create_dir("src").expect("mkdir");
    dir.sync_all().expect("sync dir");
    jail.create_dir("src/lib").expect("mkdir nested");
    let mut file = jail.create("src/lib/mod.rs").expect("create nested");
    file.write_all(b"fn main() {}").expect("write");
    assert_eq!(
        jail.open_read("src/lib/mod.rs")
            .expect("read nested")
            .read_to_string()
            .expect("read"),
        "fn main() {}"
    );
}

#[test]
fn a_confined_name_never_carries_an_absolute_path() {
    let root = Root::new("no-leak");
    let jail = jail(&root);
    jail.create_dir("a").expect("mkdir");
    let file = jail.create("a/b.txt").expect("create");
    let name = file.name();
    assert!(!name.starts_with('/'), "name must be relative: {name}");
    let workspace_path = root.0.to_string_lossy().into_owned();
    assert!(
        !name.contains(&workspace_path),
        "the confined name must not contain the workspace path: {name}"
    );
    let rendered = format!("{file:?}");
    assert!(
        !rendered.contains(&workspace_path),
        "a debug render must not leak the workspace path"
    );
}

// ---------------------------------------------------------------------------
// Malformed paths
// ---------------------------------------------------------------------------

#[test]
fn an_absolute_path_is_refused() {
    let root = Root::new("absolute");
    let jail = jail(&root);
    for path in ["/etc/passwd", "/tmp/outside", "/"] {
        let error = jail
            .open_read(path)
            .err()
            .unwrap_or_else(|| panic!("{path} must be refused"));
        assert!(
            matches!(error, ConfinementError::UnsafePath { .. }),
            "expected unsafe-path for {path}, got {error:?}"
        );
    }
}

#[test]
fn traversal_components_are_refused() {
    let root = Root::new("traversal");
    let jail = jail(&root);
    for path in [
        "../escape",
        "a/../../escape",
        "./relative",
        "a/./b",
        "a//b",
        "..",
        "a/..",
    ] {
        let error = jail
            .open_read(path)
            .err()
            .unwrap_or_else(|| panic!("{path} must be refused"));
        assert!(
            matches!(error, ConfinementError::UnsafePath { .. }),
            "expected unsafe-path for {path}, got {error:?}"
        );
    }
}

#[test]
fn an_empty_path_and_a_nul_byte_are_refused() {
    let root = Root::new("empty");
    let jail = jail(&root);
    assert!(matches!(
        jail.open_read(""),
        Err(ConfinementError::UnsafePath { .. })
    ));
    assert!(matches!(
        jail.create("bad\0name"),
        Err(ConfinementError::UnsafePath { .. })
    ));
}

// ---------------------------------------------------------------------------
// Protected names
// ---------------------------------------------------------------------------

#[test]
fn protected_names_are_refused_at_the_top_level() {
    let root = Root::new("protected-top");
    let jail = jail(&root);
    for name in PROTECTED_NAMES {
        let expected = ConfinementError::Protected {
            name: (*name).to_owned(),
        };
        assert_eq!(
            jail.open_read(name).expect_err("open_read refused"),
            expected,
            "open_read of {name} must report the protected name"
        );
        assert_eq!(
            jail.create(name).expect_err("create refused"),
            expected,
            "create of {name} must report the protected name"
        );
        assert_eq!(
            jail.open_write(name).expect_err("open_write refused"),
            expected,
            "open_write of {name} must report the protected name"
        );
        assert_eq!(
            jail.remove_file(name).expect_err("remove refused"),
            expected,
            "remove of {name} must report the protected name"
        );
        assert_eq!(
            jail.read_dir(name).expect_err("read_dir refused"),
            expected,
            "listing {name} must report the protected name"
        );
    }
}

#[test]
fn protected_names_are_refused_at_depth() {
    let root = Root::new("protected-depth");
    let jail = jail(&root);
    for path in [
        "sub/.git/config",
        "sub/deeper/.fluzo/settings.toml",
        "a/b/c/.fluzodrive/state.sqlite3",
    ] {
        let error = jail
            .open_write(path)
            .err()
            .unwrap_or_else(|| panic!("{path} must be refused"));
        let protected = PROTECTED_NAMES
            .iter()
            .find(|name| path.split('/').any(|part| part == **name))
            .expect("the path contains a protected component");
        assert_eq!(
            error,
            ConfinementError::Protected {
                name: (*protected).to_owned()
            }
        );
    }
}

#[test]
fn a_rename_cannot_overwrite_a_protected_target() {
    let root = Root::new("rename-protected");
    let jail = jail(&root);
    jail.create("innocent.txt").expect("create source");
    let error = jail
        .rename("innocent.txt", ".git/HOOKS")
        .expect_err("renaming onto a protected name must be refused");
    assert_eq!(
        error,
        ConfinementError::Protected {
            name: ".git".to_owned()
        }
    );
    assert!(
        jail.symlink_metadata("innocent.txt")
            .expect("stat")
            .is_some(),
        "the refused rename must leave the source where it was"
    );
}

#[test]
fn a_rename_cannot_escape_the_jail() {
    let root = Root::new("rename-escape");
    let jail = jail(&root);
    jail.create("inside.txt").expect("create");
    let error = jail
        .rename("inside.txt", "../../outside.txt")
        .expect_err("a traversal destination must be refused");
    assert!(
        matches!(error, ConfinementError::UnsafePath { .. }),
        "expected unsafe-path, got {error:?}"
    );
    assert!(jail.symlink_metadata("inside.txt").expect("stat").is_some());
}

// ---------------------------------------------------------------------------
// Symlinks
// ---------------------------------------------------------------------------

#[test]
fn a_symlink_at_the_target_is_refused_not_followed() {
    let root = Root::new("target-symlink");
    let jail = jail(&root);
    // Put a real file at the path, then replace it with a link out.
    fs::write(root.0.join("real.txt"), b"workspace content").unwrap();
    fs::remove_file(root.0.join("real.txt")).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", root.0.join("real.txt")).unwrap();
    let error = jail
        .open_read("real.txt")
        .expect_err("a symlink target must be refused");
    assert_eq!(error, ConfinementError::Escape);
}

#[test]
fn a_symlink_pointing_back_inside_is_still_refused() {
    let root = Root::new("inside-symlink");
    let jail = jail(&root);
    fs::write(root.0.join("target.txt"), b"inside").unwrap();
    std::os::unix::fs::symlink(root.0.join("target.txt"), root.0.join("link.txt")).unwrap();
    // The agent named a link, not the file. Following it would mean
    // trusting a pointer another process can replace, so it is refused
    // even though it lands inside.
    let error = jail.open_read("link.txt").expect_err("refused");
    assert_eq!(error, ConfinementError::Escape);
}

#[test]
fn a_symlinked_intermediate_directory_is_refused() {
    let root = Root::new("intermediate");
    fs::create_dir(root.0.join("realdir")).unwrap();
    fs::write(root.0.join("realdir/file.txt"), b"x").unwrap();
    let jail = jail(&root);
    // After the jail is open, swap the intermediate directory for a
    // link to /etc. This is the check-then-use window, made
    // deterministic: the jail must not follow it.
    fs::remove_file(root.0.join("realdir/file.txt")).unwrap();
    fs::remove_dir(root.0.join("realdir")).unwrap();
    std::os::unix::fs::symlink("/etc", root.0.join("realdir")).unwrap();
    let error = jail
        .open_read("realdir/passwd")
        .expect_err("a symlinked intermediate must be refused");
    assert_eq!(error, ConfinementError::Escape);
}

#[test]
fn a_rename_cannot_replace_a_symlink_destination() {
    let root = Root::new("rename-onto-symlink");
    let jail = jail(&root);
    jail.create("source.txt").expect("create");
    std::os::unix::fs::symlink("/etc/shadow", root.0.join("dest.txt")).unwrap();
    let error = jail
        .rename("source.txt", "dest.txt")
        .expect_err("renaming onto a symlink must be refused");
    assert_eq!(error, ConfinementError::Escape);
    assert!(
        root.0.join("source.txt").exists(),
        "the source must not have moved"
    );
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

#[test]
fn create_refuses_when_something_is_already_there() {
    let root = Root::new("exists");
    let jail = jail(&root);
    jail.create("taken.txt").expect("first create");
    let error = jail.create("taken.txt").expect_err("second create refused");
    assert_eq!(error, ConfinementError::Exists);
}

#[test]
fn opening_a_missing_file_reports_not_found_rather_than_creating_it() {
    let root = Root::new("missing");
    let jail = jail(&root);
    let error = jail.open_read("ghost.txt").expect_err("refused");
    assert_eq!(error, ConfinementError::NotFound);
    assert!(!root.0.join("ghost.txt").exists());
    let error = jail.open_write("ghost.txt").expect_err("refused");
    assert_eq!(error, ConfinementError::NotFound);
    assert!(!root.0.join("ghost.txt").exists());
}

#[test]
fn a_missing_parent_is_not_invented_by_a_create() {
    let root = Root::new("missing-parent");
    let jail = jail(&root);
    let error = jail
        .create("nope/deep/file.txt")
        .expect_err("a create must not invent parent directories");
    assert_eq!(error, ConfinementError::NotFound);
    assert!(!root.0.join("nope").exists());
}

#[test]
fn remove_file_refuses_a_directory() {
    let root = Root::new("remove-dir");
    let jail = jail(&root);
    jail.create_dir("folder").expect("mkdir");
    let error = jail.remove_file("folder").expect_err("refused");
    assert_eq!(
        error,
        ConfinementError::NotARegularFile,
        "a directory is not a file, so remove_file must say so"
    );
    assert!(root.0.join("folder").is_dir(), "the directory survives");
}

#[test]
fn remove_file_removes_a_regular_file() {
    let root = Root::new("remove-file");
    let jail = jail(&root);
    jail.create("gone.txt").expect("create");
    jail.remove_file("gone.txt").expect("remove");
    assert!(!root.0.join("gone.txt").exists());
    assert_eq!(
        jail.symlink_metadata("gone.txt").expect("stat"),
        None,
        "the jail must agree the file is gone"
    );
}

#[test]
fn remove_dir_removes_only_an_empty_directory() {
    let root = Root::new("remove-empty-dir");
    let jail = jail(&root);
    jail.create_dir("empty").expect("mkdir");
    jail.remove_dir("empty").expect("rmdir");
    assert!(!root.0.join("empty").exists());
    jail.create_dir("full").expect("mkdir again");
    jail.create("full/child.txt").expect("create child");
    let error = jail
        .remove_dir("full")
        .expect_err("non-empty rmdir refused");
    assert_eq!(error, ConfinementError::NotFound);
    assert!(root.0.join("full").is_dir());
}

#[test]
fn rename_moves_within_the_jail() {
    let root = Root::new("rename-inside");
    let jail = jail(&root);
    jail.create_dir("dir").expect("mkdir");
    let mut file = jail.create("before.txt").expect("create");
    file.write_all(b"payload").expect("write");
    jail.rename("before.txt", "dir/after.txt").expect("rename");
    assert!(!root.0.join("before.txt").exists());
    assert_eq!(
        jail.open_read("dir/after.txt")
            .expect("read after rename")
            .read_to_string()
            .expect("read"),
        "payload"
    );
}

#[test]
fn a_hardlinked_target_is_refused() {
    let root = Root::new("hardlink");
    let jail = jail(&root);
    jail.create("original.txt").expect("create");
    // A second name for the same inode. Writing through either one
    // changes both, so a hard link is a channel out of the workspace
    // that O_NOFOLLOW says nothing about.
    fs::hard_link(root.0.join("original.txt"), root.0.join("twin.txt")).unwrap();
    let error = jail
        .open_write("original.txt")
        .expect_err("a shared inode must be refused");
    assert_eq!(error, ConfinementError::SharedLink { links: 2 });
    let error = jail
        .open_write("twin.txt")
        .expect_err("the other name is equally shared");
    assert_eq!(error, ConfinementError::SharedLink { links: 2 });
}

#[test]
fn a_fifo_target_is_refused() {
    let root = Root::new("fifo");
    let jail = jail(&root);
    let fifo = root.0.join("pipe");
    let dir = std::fs::File::open(&root.0).expect("open root");
    rustix::fs::mknodat(
        &dir,
        "pipe",
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_bits_truncate(0o600),
        0,
    )
    .expect("mkfifo");
    let error = jail
        .open_write(&fifo.file_name().unwrap().to_string_lossy())
        .expect_err("a FIFO must be refused");
    assert_eq!(error, ConfinementError::NotARegularFile);
}

#[test]
fn a_socket_target_is_refused() {
    let root = Root::new("socket");
    let jail = jail(&root);
    let listener =
        std::os::unix::net::UnixListener::bind(root.0.join("sock")).expect("bind socket");
    let error = jail
        .open_write("sock")
        .expect_err("a socket must be refused");
    assert_eq!(error, ConfinementError::NotARegularFile);
    drop(listener);
}

#[test]
fn a_directory_opened_as_a_file_is_refused() {
    let root = Root::new("dir-as-file");
    let jail = jail(&root);
    jail.create_dir("folder").expect("mkdir");
    let error = jail
        .open_write("folder")
        .expect_err("a directory is not a file");
    assert!(
        matches!(
            error,
            ConfinementError::NotADirectory | ConfinementError::NotARegularFile
        ),
        "expected a refusal, got {error:?}"
    );
}

#[test]
fn a_plain_regular_file_still_has_exactly_one_link() {
    // Guards against the shared-link check firing on the normal case,
    // which would make the jail useless rather than safe.
    let root = Root::new("single-link");
    let jail = jail(&root);
    let mut file = jail.create("solo.txt").expect("create");
    file.write_all(b"mine").expect("write");
    assert_eq!(file.len().expect("len"), 4);
    assert!(jail.open_write("solo.txt").is_ok());
}

#[test]
fn read_dir_lists_sorted_entries_with_kinds() {
    let root = Root::new("listing");
    let jail = jail(&root);
    jail.create("b.txt").expect("create b");
    jail.create("a.txt").expect("create a");
    jail.create_dir("zdir").expect("mkdir");
    std::os::unix::fs::symlink("/etc", root.0.join("linky")).unwrap();
    let entries = jail.read_dir(".").expect("read_dir");
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names, ["a.txt", "b.txt", "linky", "zdir"]);
    let kinds: Vec<EntryKind> = entries.iter().map(|entry| entry.kind).collect();
    assert_eq!(
        kinds,
        [
            EntryKind::File,
            EntryKind::File,
            EntryKind::Symlink,
            EntryKind::Directory
        ]
    );
    assert!(
        !names.contains(&".") && !names.contains(&".."),
        "the listing must not include dot entries"
    );
}

#[test]
fn read_dir_refuses_a_file_and_a_missing_path() {
    let root = Root::new("listing-bad");
    let jail = jail(&root);
    jail.create("file.txt").expect("create");
    assert_eq!(
        jail.read_dir("file.txt").expect_err("refused"),
        ConfinementError::NotADirectory
    );
    assert_eq!(
        jail.read_dir("absent").expect_err("refused"),
        ConfinementError::NotFound
    );
}

#[test]
fn symlink_metadata_does_not_follow_the_final_component() {
    let root = Root::new("stat-nofollow");
    let jail = jail(&root);
    jail.create("plain.txt").expect("create");
    let meta = jail
        .symlink_metadata("plain.txt")
        .expect("stat")
        .expect("present");
    assert!(meta.is_file && !meta.is_symlink);
    std::os::unix::fs::symlink("/etc/passwd", root.0.join("link.txt")).unwrap();
    let meta = jail
        .symlink_metadata("link.txt")
        .expect("stat")
        .expect("present");
    assert!(meta.is_symlink && !meta.is_file);
}

#[test]
fn a_missing_root_does_not_open_a_jail() {
    let missing = std::env::temp_dir().join(format!(
        "fluzo-jail-missing-{}-{}",
        std::process::id(),
        NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
    ));
    let error = WorkspaceJail::open(&missing).expect_err("opening a missing root must fail");
    assert_eq!(error, ConfinementError::NotFound);
    assert!(!missing.exists(), "a failed open must not create the root");
}

#[test]
fn a_jail_over_a_file_is_refused() {
    let root = Root::new("root-is-file");
    let file = root.0.join("not-a-dir");
    fs::write(&file, b"x").unwrap();
    let error = WorkspaceJail::open(&file).expect_err("a file is not a jail root");
    assert!(
        matches!(
            error,
            ConfinementError::Escape | ConfinementError::UnsafePath { .. }
        ),
        "expected a refusal, got {error:?}"
    );
}

#[test]
fn a_jail_root_reached_through_a_symlink_is_the_resolved_directory() {
    // The root itself may be named through a symlink: it is resolved
    // once, at open, and the jail then holds the real directory.
    let real = Root::new("resolve-root");
    let alias = Root::new("alias-root");
    let link = alias.0.join("link-to-real");
    std::os::unix::fs::symlink(&real.0, &link).unwrap();
    let jail = WorkspaceJail::open(&link).expect("open through alias");
    jail.create("written.txt").expect("create via aliased jail");
    assert!(
        real.0.join("written.txt").exists(),
        "the write landed in the resolved root"
    );
}

#[test]
fn a_refused_operation_changed_nothing_on_disk() {
    let root = Root::new("inert");
    let jail = jail(&root);
    jail.create("keep.txt").expect("create");
    let before = snapshot(&root.0);
    // Every refusal class, attempted against this jail.
    let _ = jail.open_read("/etc/passwd");
    let _ = jail.open_read("../outside");
    let _ = jail.open_read(".git/config");
    let _ = jail.open_read("absent.txt");
    let _ = jail.create("keep.txt");
    let _ = jail.rename("keep.txt", "../escape");
    let _ = jail.read_dir("absent");
    let _ = jail.remove_file("a/../keep.txt");
    let _ = jail.create_dir(".fluzodrive");
    let after = snapshot(&root.0);
    assert_eq!(before, after, "refusals must be inert");
}

/// A whole-file replacement must not leave the previous tail behind.
///
/// `open_write` is a read/write handle, so writing a shorter buffer over a
/// longer file would otherwise splice rather than replace. The tool layer
/// relies on this for edit semantics.
#[test]
fn replacing_contents_truncates_the_previous_tail() {
    let root = Root::new("replace");
    fs::write(root.0.join("file.txt"), b"a much longer original body").expect("fixture");
    let jail = WorkspaceJail::open(&root.0).expect("jail opens");

    let mut file = jail.open_write("file.txt").expect("open_write");
    file.replace_all(b"short").expect("replace_all");
    file.sync_all().expect("sync");

    assert_eq!(
        fs::read_to_string(root.0.join("file.txt")).expect("read back"),
        "short",
        "the old tail must be gone"
    );
}

fn snapshot(root: &Path) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read_dir") {
            let entry = entry.expect("entry");
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).expect("metadata");
            out.push((
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string(),
                metadata.len(),
            ));
            if metadata.is_dir() {
                stack.push(path);
            }
        }
    }
    out.sort();
    out
}
