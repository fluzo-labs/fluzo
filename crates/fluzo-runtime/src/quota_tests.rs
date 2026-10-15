use crate::artifact::ARTIFACTS_DIRECTORY;
use crate::quota::*;
use crate::store::{DRIVE_DIRECTORY, StoreError};
use fluzo_core::settings::{SettingValue, Settings};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(1);

/// Isolated temporary managed root. `Drop` removes it so a failing test
/// cannot leave private data behind.
struct Root(PathBuf);

impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "fluzo-quota-{}-{}",
            process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn drive(&self) -> PathBuf {
        self.0.join(DRIVE_DIRECTORY)
    }

    fn ensure_drive(&self) {
        fs::create_dir_all(self.drive()).unwrap();
        let _ = fs::set_permissions(
            self.drive(),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        );
    }

    fn write(&self, relative: &str, len: usize) -> PathBuf {
        let path = self.drive().join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(&vec![b'x'; len]).unwrap();
        file.flush().unwrap();
        path
    }

    fn write_outside(&self, relative: &str, len: usize) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, vec![b'y'; len]).unwrap();
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Small synthetic limits: nothing here writes gigabytes.
fn limits(session: u64, diagnostics: u64, reserve: u64) -> QuotaLimits {
    QuotaLimits {
        session_max_bytes: session,
        diagnostic_max_bytes: diagnostics,
        min_free_bytes: reserve,
    }
}

fn ledger(root: &Root, l: QuotaLimits, free: u64) -> QuotaLedger {
    root.ensure_drive();
    QuotaLedger::new(root.drive(), l, FixedFreeSpace(free))
}

#[test]
fn defaults_match_the_prd_guardrails() {
    let defaults = QuotaLimits::default();
    assert_eq!(defaults.session_max_bytes, 10 * 1024 * 1024 * 1024);
    assert_eq!(defaults.diagnostic_max_bytes, 2 * 1024 * 1024 * 1024);
    assert_eq!(defaults.min_free_bytes, 1024 * 1024 * 1024);
    assert!(
        !Settings::default().storage.auto_expire,
        "retained history must not be configurable to expire automatically"
    );
}

#[test]
fn limits_come_from_the_repository_typed_settings() {
    let mut settings = Settings::default();
    settings.storage.session_max_bytes = 4096;
    settings.storage.diagnostic_max_bytes = 512;
    settings.storage.min_free_bytes = 128;
    let mapped = QuotaLimits::from_storage(&settings.storage);
    assert_eq!(mapped.session_max_bytes, 4096);
    assert_eq!(mapped.diagnostic_max_bytes, 512);
    assert_eq!(mapped.min_free_bytes, 128);
    assert_eq!(mapped.limit_for(QuotaClass::SessionData), 4096);
    assert_eq!(mapped.limit_for(QuotaClass::Diagnostics), 512);

    let root = Root::new();
    let l = ledger(&root, mapped, 1_000_000);
    root.write("state.sqlite3", 4_000);
    let status = l.status(QuotaClass::SessionData).unwrap();
    assert!(status.warning && status.used_bytes == 4_000 && status.limit_bytes == 4_096);
    // The descriptor exposes the same default the ledger uses.
    let descriptor = Settings::default()
        .entries()
        .into_iter()
        .find(|(descriptor, _)| descriptor.key == "storage.session_max_bytes")
        .expect("storage.session_max_bytes is a registered setting");
    assert_eq!(descriptor.1, SettingValue::Integer(10 * 1024 * 1024 * 1024));
}

#[test]
fn measurement_covers_the_journal_wal_side_files_and_artifacts() {
    let root = Root::new();
    let l = ledger(&root, limits(100_000, 100_000, 1), 1_000_000);
    root.write("state.sqlite3", 1_000);
    root.write("state.sqlite3-wal", 400);
    root.write("state.sqlite3-shm", 32);
    root.write("artifacts/a", 700);
    root.write("artifacts/b", 60);
    let usage = l.measure().unwrap();
    assert_eq!(usage.session_bytes, 1_000 + 400 + 32 + 700 + 60);
    assert_eq!(usage.diagnostics_bytes, 0);
}

#[test]
fn the_two_accounted_classes_never_bleed_into_each_other() {
    let root = Root::new();
    let l = ledger(&root, limits(100_000, 100_000, 1), 1_000_000);
    root.write("state.sqlite3", 1_000);
    root.write("artifacts/a", 500);
    root.write("telemetry/run-1.log", 300);
    root.write("telemetry/run-2.log", 30);
    let usage = l.measure().unwrap();
    assert_eq!(usage.session_bytes, 1_500);
    assert_eq!(usage.diagnostics_bytes, 330);
    assert_eq!(usage.of(QuotaClass::SessionData), 1_500);
    assert_eq!(usage.of(QuotaClass::Diagnostics), 330);
}

#[test]
fn absent_class_directories_measure_zero() {
    let root = Root::new();
    let l = ledger(&root, limits(100, 100, 1), 1_000_000);
    let usage = l.measure().unwrap();
    assert_eq!(usage, QuotaUsage::default());
    assert_eq!(
        l.status(QuotaClass::Diagnostics).unwrap().used_bytes,
        0,
        "an unopened store holds nothing"
    );
}

#[test]
fn files_outside_the_managed_root_are_not_accounted() {
    let root = Root::new();
    let l = ledger(&root, limits(100_000, 100_000, 1), 1_000_000);
    root.write_outside("src/main.rs", 50_000);
    root.write_outside("unrelated.bin", 50_000);
    let usage = l.measure().unwrap();
    assert_eq!(usage.session_bytes, 0);
    assert_eq!(usage.diagnostics_bytes, 0);
}

#[test]
fn a_symlinked_class_directory_is_a_conflict_not_a_measurement() {
    let root = Root::new();
    let l = ledger(&root, limits(100_000, 100_000, 1), 1_000_000);
    let outside = root.0.join("outside-artifacts");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("big"), vec![b'z'; 9_000]).unwrap();
    std::os::unix::fs::symlink(&outside, root.drive().join(ARTIFACTS_DIRECTORY)).unwrap();
    assert_eq!(l.measure().err(), Some(StoreError::Conflict));
}

#[test]
fn a_symlinked_entry_inside_a_class_is_a_conflict() {
    let root = Root::new();
    let l = ledger(&root, limits(100_000, 100_000, 1), 1_000_000);
    let outside = root.0.join("outside.bin");
    fs::write(&outside, vec![b'z'; 9_000]).unwrap();
    fs::create_dir_all(root.drive().join("telemetry")).unwrap();
    std::os::unix::fs::symlink(&outside, root.drive().join("telemetry/link.bin")).unwrap();
    assert_eq!(l.measure().err(), Some(StoreError::Conflict));
}

#[test]
fn warning_is_surfaced_at_eighty_percent_and_admission_still_succeeds() {
    let root = Root::new();
    // 100 KiB session limit: 80 KiB used is exactly the warning point.
    let l = ledger(&root, limits(102_400, 102_400, 1), 1_000_000);
    root.write("state.sqlite3", 81_920);
    let status = l.status(QuotaClass::SessionData).unwrap();
    assert!(status.warning, "80% utilization must warn");
    // A tiny request still fits, and the caller sees the warning first.
    let admission = l.admit_session_with_headroom(1, 1).unwrap();
    assert!(matches!(admission, Admission::Warning(_)));
    assert!(admission.admitted());

    // Just below the threshold: no warning at all.
    let quiet = Root::new();
    let l = ledger(&quiet, limits(102_400, 102_400, 1), 1_000_000);
    quiet.write("state.sqlite3", 81_919);
    assert!(!l.status(QuotaClass::SessionData).unwrap().warning);
    assert!(matches!(
        l.admit_session_with_headroom(1, 1).unwrap(),
        Admission::Admitted(_)
    ));
}

#[test]
fn session_admission_refuses_when_the_projected_total_reaches_the_limit() {
    let root = Root::new();
    let l = ledger(&root, limits(10_000, 10_000, 1), 1_000_000);
    root.write("state.sqlite3", 9_500);
    let admission = l.admit_session_with_headroom(500, 0).unwrap();
    match admission {
        Admission::Refused(refusal) => {
            assert_eq!(refusal.violation, QuotaViolation::SessionLimit);
            assert_eq!(refusal.status.used_bytes, 9_500);
            assert_eq!(refusal.projected_bytes, 500);
        }
        other => panic!("expected a session-limit refusal, got {other:?}"),
    }
    assert!(!admission.admitted());
}

#[test]
fn the_audit_headroom_refuses_before_the_audit_state_is_at_risk() {
    let root = Root::new();
    let l = ledger(&root, limits(10_000, 10_000, 1), 1_000_000);
    root.write("state.sqlite3", 9_500);
    // The payload alone would fit inside the limit...
    assert!(l.admit_session_with_headroom(400, 0).unwrap().admitted());
    // ...but the unit's own audit state would not, so it is refused now,
    // before any transaction opens.
    let with_audit = l.admit_session_with_headroom(400, 200).unwrap();
    assert!(!with_audit.admitted());
    assert_eq!(
        match with_audit {
            Admission::Refused(refusal) => refusal.violation,
            _ => QuotaViolation::DiagnosticLimit,
        },
        QuotaViolation::SessionLimit
    );
}

#[test]
fn a_reserve_shortfall_refuses_session_admission_ahead_of_the_class_limit() {
    let root = Root::new();
    // Session usage is low; the filesystem is the problem.
    let l = ledger(&root, limits(10_000, 10_000, 1_024), 512);
    root.write("state.sqlite3", 10);
    match l.admit_session(1).unwrap() {
        Admission::Refused(refusal) => {
            assert_eq!(refusal.violation, QuotaViolation::FreeSpaceReserve)
        }
        other => panic!("expected a reserve refusal, got {other:?}"),
    }
}

#[test]
fn diagnostics_drop_at_the_limit_instead_of_refusing_work() {
    let root = Root::new();
    let l = ledger(&root, limits(1_000_000, 1_000, 1), 1_000_000);
    root.write("telemetry/a.log", 900);
    match l.admit_diagnostic(100).unwrap() {
        DiagnosticDisposition::Dropped { reason, status } => {
            assert_eq!(reason, QuotaViolation::DiagnosticLimit);
            assert_eq!(status.class, QuotaClass::Diagnostics);
            assert_eq!(status.used_bytes, 900);
        }
        other => panic!("expected a drop, got {other:?}"),
    }
    // Below the limit the same call accepts.
    assert!(l.admit_diagnostic(50).unwrap().accepted());
}

#[test]
fn a_reserve_shortfall_drops_diagnostics_without_an_error() {
    let root = Root::new();
    let l = ledger(&root, limits(1_000_000, 1_000, 1_024), 10);
    match l.admit_diagnostic(1).unwrap() {
        DiagnosticDisposition::Dropped {
            reason: QuotaViolation::FreeSpaceReserve,
            ..
        } => {}
        other => panic!("expected a reserve drop, got {other:?}"),
    }
}

#[test]
fn a_failing_probe_is_a_typed_error_not_a_silent_pass() {
    #[derive(Debug)]
    struct BrokenProbe;
    impl FileSystemProbe for BrokenProbe {
        fn available_bytes(&self, _path: &Path) -> Result<u64, StoreError> {
            Err(StoreError::Inaccessible)
        }
    }
    let root = Root::new();
    root.ensure_drive();
    let l = QuotaLedger::new(root.drive(), limits(10_000, 10_000, 1), BrokenProbe);
    assert_eq!(
        l.admit_session(1).err(),
        Some(StoreError::Inaccessible),
        "an unknown filesystem state must not be read as admitted"
    );
    assert_eq!(l.admit_diagnostic(1).err(), Some(StoreError::Inaccessible));
}

#[test]
fn the_real_probe_reports_a_plausible_free_space_value() {
    let root = Root::new();
    root.ensure_drive();
    let l = QuotaLedger::new(root.drive(), limits(10_000, 10_000, 1), MountedFileSystem);
    let available = l.reserve_available().unwrap();
    assert!(
        available > 0 && available < 1 << 60,
        "statvfs returned {available}, which is not a usable byte count"
    );
}

#[test]
fn measurement_is_stable_across_repeated_reads() {
    let root = Root::new();
    let l = ledger(&root, limits(100_000, 100_000, 1), 1_000_000);
    root.write("state.sqlite3", 1_234);
    root.write("artifacts/a", 5);
    root.write("telemetry/a.log", 67);
    let first = l.measure().unwrap();
    for _ in 0..5 {
        assert_eq!(l.measure().unwrap(), first);
    }
}
