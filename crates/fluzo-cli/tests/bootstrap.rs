use std::process::Command;

#[test]
fn version_is_available_without_configuration() {
    let output = Command::new(env!("CARGO_BIN_EXE_fluzo"))
        .arg("--version")
        .output()
        .expect("bootstrap executable must start");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("version is UTF-8"),
        format!("fluzo {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_discloses_unimplemented_runtime() {
    let output = Command::new(env!("CARGO_BIN_EXE_fluzo"))
        .arg("--help")
        .output()
        .expect("bootstrap executable must start");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("not implemented"));
}

#[test]
fn demo_is_explicit_isolated_and_never_reads_project_configuration() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "fluzo-demo-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&root).unwrap();
    struct Fixture(std::path::PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let fixture = Fixture(root);
    let configuration = "invalid TOML; must not be parsed or rewritten";
    std::fs::write(fixture.0.join(".fluzo"), configuration).unwrap();
    std::fs::write(fixture.0.join("source.rs"), "unchanged fixture").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_fluzo"))
        .arg("demo")
        .env_clear()
        .env("HOME", &fixture.0)
        .env("XDG_CONFIG_HOME", &fixture.0)
        .current_dir(&fixture.0)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("demo exceeded its safety deadline");
        }
        std::thread::yield_now();
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("synthetic fixtures only; no agent or tools executed"));
    assert!(text.contains("Task 1: Pending (version 1)"));
    assert_eq!(
        std::fs::read_to_string(fixture.0.join(".fluzo")).unwrap(),
        configuration
    );
    assert_eq!(
        std::fs::read_to_string(fixture.0.join("source.rs")).unwrap(),
        "unchanged fixture"
    );
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 2);
}

#[test]
fn execution_and_unknown_arguments_never_report_success() {
    for arguments in [vec![], vec!["--unknown"], vec!["--version", "--unknown"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_fluzo"))
            .args(arguments)
            .output()
            .expect("bootstrap executable must start");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("not implemented"));
    }
}
