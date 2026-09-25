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
