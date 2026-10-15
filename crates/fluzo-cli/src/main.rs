mod startup;

use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() >= 2 && arguments[0] == "demo" && arguments[1] == "--interactive" {
        let visual_arguments: Option<Vec<_>> = arguments[2..]
            .iter()
            .map(|value| value.to_str().map(str::to_owned))
            .collect();
        let mut options = match visual_arguments
            .as_deref()
            .ok_or("Visual options must be valid UTF-8.")
            .and_then(fluzo_tui::visual::VisualOptions::parse)
        {
            Ok(options) => options,
            Err(error) => {
                eprintln!("Configuration error: {error}");
                return ExitCode::FAILURE;
            }
        };
        let driver = match fluzo_runtime::scenario::demo_driver() {
            Ok(driver) => driver,
            Err(error) => {
                eprintln!("Demo unavailable: {error}");
                return ExitCode::FAILURE;
            }
        };
        let scenes = match demo_scenes(&driver) {
            Ok(scenes) => scenes,
            Err(error) => {
                eprintln!("Demo unavailable: {error}");
                return ExitCode::FAILURE;
            }
        };
        options.repository = std::env::current_dir()
            .map(|path| {
                display_directory(
                    &path,
                    std::env::var_os("HOME")
                        .as_deref()
                        .map(std::path::Path::new),
                )
            })
            .unwrap_or_else(|_| "Directory unavailable".into());
        return match fluzo_tui::terminal::run_demo(&driver, options, &scenes) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Demo unavailable: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if arguments.len() == 1 {
        if arguments[0] == "demo" {
            use fluzo_core::application::{
                ApplicationPort, PROTOCOL_VERSION, Query, QueryRequest, QueryResponse,
            };

            let driver = match fluzo_runtime::scenario::demo_driver() {
                Ok(driver) => driver,
                Err(error) => {
                    eprintln!("Demo unavailable: {error}");
                    return ExitCode::FAILURE;
                }
            };
            let response = driver.query(QueryRequest {
                protocol_version: PROTOCOL_VERSION,
                query: Query::Tasks {
                    after: None,
                    limit: 8,
                },
            });
            match response {
                Ok(QueryResponse::Snapshot(snapshot)) => {
                    println!(
                        "Fluzo protocol demo: synthetic fixtures only; no agent or tools executed."
                    );
                    for task in snapshot.tasks {
                        println!(
                            "Task {}: {:?} (version {})",
                            task.id.0, task.state, task.version
                        );
                    }
                    return ExitCode::SUCCESS;
                }
                Ok(_) => eprintln!("Demo returned an unexpected response."),
                Err(error) => eprintln!("Demo unavailable: {error}"),
            }
            return ExitCode::FAILURE;
        }
        if arguments[0] == "--version" {
            println!("fluzo {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        if arguments[0] == "--help" {
            println!(
                "Fluzo development bootstrap\nUsage: fluzo --help | --version | demo [--interactive]\nInteractive options: --animation-fps <0..60> (default 60; alternatives 30/15/0)\n  --reduced-motion --theme <default|high-contrast> --ascii\n  --dev-menu | --no-dev-menu (MVP default: enabled; explicit disable wins)\n  --desktop-notifications (opt-in; Ghostty OSC 777, unfocused only)\n  Developer menu: Ctrl+T schedules a notification test in 3 seconds\nRead-only demo: no .fluzo loading or saving; visual options require demo --interactive."
            );
            println!(
                "Setup: fluzo [init] [--config <workspace-relative path>] [visual options]\nMissing configuration opens offline setup interactively; headless returns configuration_required.\nScreen preferences use defaults; change them later in Configuration.\n--safe-screen-settings uses ASCII, no color, FPS 0, reduced motion and no desktop notifications for this invocation only. Saved screen values are unchanged; restart without the flag to apply normal presentation.\nExisting files are preserved: replacement requires a controlled host. No automatic connectivity probes.\nF3 opens guided model dialogs: Local/Frontier, provider, optional Authorization environment reference, URL and multiple models. Leaving URL resolves the endpoint first; a non-local server shows a cleartext warning that must be accepted before the catalog is fetched. HTTP with localhost, private IP or DNS name; no HTTPS or inference.\nValid configuration opens the normal shell: Ctrl+P opens typed Configuration views; F1 shows editor help.\nAfter ordinary setup creation, Enter or Ctrl+P opens the Configuration menu. Ctrl+S saves future values only where permitted; Ctrl+A applies presentation.\nNo tasks execute. The explicit demo remains isolated from user configuration."
            );
            println!(
                "Telemetry: fluzo telemetry purge --local --dry-run | --yes\nPreviews or purges this repository's local diagnostic archive and pending export buffers under .fluzodrive/telemetry/. --dry-run deletes nothing; --yes is required to delete even in YOLO. Quiesces the diagnostic writer or refuses naming it; never touches the database, .fluzo or remote backend data."
            );
            println!(
                "{}",
                fluzo_tui::availability_text(fluzo_runtime::availability())
            );
            return ExitCode::SUCCESS;
        }
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "telemetry")
    {
        return telemetry_command(&arguments[1..]);
    }
    startup::run(&arguments)
}

/// `fluzo telemetry purge --local (--dry-run | --yes)`.
///
/// DAT-02e: previews or purges this repository's local diagnostic
/// archive and pending export buffers under `.fluzodrive/telemetry/`.
/// The purge is a local, synchronous filesystem operation through the
/// store's fd-anchored discipline; it never opens a database write, never
/// touches `.fluzo`, credentials or unrelated volumes, and never deletes
/// under a live writer. A dry-run performs no deletion; `--yes` is
/// required to delete even in YOLO, and a caller that cannot confirm
/// deletes nothing and receives a structured outcome.
fn telemetry_command(arguments: &[std::ffi::OsString]) -> ExitCode {
    use fluzo_runtime::telemetry_purge::PurgeConfirmation;

    if arguments.first().is_none_or(|argument| argument != "purge") {
        return fail(
            "invalid_arguments",
            "Usage: fluzo telemetry purge --local (--dry-run | --yes)",
        );
    }
    let mut local = false;
    let mut dry_run = false;
    let mut yes = false;
    for argument in &arguments[1..] {
        if argument == "--local" {
            local = true;
        } else if argument == "--dry-run" {
            dry_run = true;
        } else if argument == "--yes" {
            yes = true;
        } else {
            return fail(
                "invalid_arguments",
                &format!(
                    "Unknown telemetry purge option: {}",
                    argument.to_string_lossy()
                ),
            );
        }
    }
    if !local {
        return fail(
            "invalid_arguments",
            "telemetry purge requires --local (only this repository's local stores are supported)",
        );
    }
    if dry_run && yes {
        return fail(
            "invalid_arguments",
            "--dry-run and --yes are mutually exclusive",
        );
    }
    let workspace = match std::env::current_dir() {
        Ok(path) => path,
        Err(_) => {
            return fail(
                "workspace_unavailable",
                "Current working directory unavailable; nothing deleted.",
            );
        }
    };

    if dry_run {
        return match fluzo_runtime::telemetry_purge::preview(&workspace, None) {
            Ok(preview) => {
                println!(
                    "Local telemetry purge preview for repository '{}'",
                    preview.repository
                );
                println!("  path: {}", preview.telemetry_path.display());
                println!(
                    "  diagnostic archive: {} segment(s), {} byte(s)",
                    preview.archive_segments, preview.archive_bytes
                );
                println!(
                    "  pending export buffers: {} buffer(s), {} byte(s)",
                    preview.pending_buffers, preview.pending_bytes
                );
                println!("  total: {} byte(s)", preview.total_bytes());
                println!("Nothing deleted. Re-run with --yes to purge.");
                ExitCode::SUCCESS
            }
            Err(error) => fail(purge_code(&error), &purge_message(&error)),
        };
    }

    if !yes {
        return fail(
            "confirmation_required",
            "telemetry purge requires --yes to delete; nothing was deleted.",
        );
    }

    match fluzo_runtime::telemetry_purge::purge(
        &workspace,
        None,
        PurgeConfirmation::Yes,
        std::time::Duration::from_secs(5),
    ) {
        Ok(result) => {
            print!("{result}");
            ExitCode::SUCCESS
        }
        Err(error) => fail(purge_code(&error), &purge_message(&error)),
    }
}

fn purge_code(error: &fluzo_runtime::telemetry_purge::PurgeError) -> &'static str {
    match error {
        fluzo_runtime::telemetry_purge::PurgeError::ConfirmationRequired => "confirmation_required",
        fluzo_runtime::telemetry_purge::PurgeError::ActiveWriter { .. } => "active_writer",
        fluzo_runtime::telemetry_purge::PurgeError::UnsafeRoot => "unsafe_root",
        fluzo_runtime::telemetry_purge::PurgeError::Io(_) => "io_error",
    }
}

fn purge_message(error: &fluzo_runtime::telemetry_purge::PurgeError) -> String {
    match error {
        fluzo_runtime::telemetry_purge::PurgeError::ActiveWriter { component } => {
            format!("diagnostic component '{component}' is still active; nothing deleted")
        }
        other => other.to_string(),
    }
}

fn fail(code: &str, detail: &str) -> ExitCode {
    let detail = fluzo_tui::setup::safe_text(detail);
    let escaped: String = detail
        .chars()
        .flat_map(|character| match character {
            '"' => vec!['\\', '"'],
            '\\' => vec!['\\', '\\'],
            other => vec![other],
        })
        .collect();
    eprintln!("{{\"code\":\"{code}\",\"message\":\"{escaped}\"}}");
    ExitCode::FAILURE
}

fn demo_scenes(
    port: &dyn fluzo_core::application::ApplicationPort,
) -> Result<Vec<fluzo_core::application::Snapshot>, fluzo_core::application::ApplicationError> {
    use fluzo_core::application::*;
    let QueryResponse::Snapshot(base) = port.query(QueryRequest {
        protocol_version: PROTOCOL_VERSION,
        query: Query::Tasks {
            after: None,
            limit: 64,
        },
    })?
    else {
        return Err(ApplicationError::InvalidRequest);
    };
    let mut scenes = Vec::new();
    for (index, state) in [
        TaskState::Running,
        TaskState::Waiting,
        TaskState::Blocked,
        TaskState::Completed,
        TaskState::Failed,
        TaskState::Cancelled,
        TaskState::Pending,
    ]
    .into_iter()
    .enumerate()
    {
        let mut snapshot = base.clone();
        let task = snapshot
            .tasks
            .first_mut()
            .ok_or(ApplicationError::InvalidRequest)?;
        task.state = state;
        task.version = index as u64 + 2;
        task.reason = match state {
            TaskState::Waiting => Some(WaitReason::UserInput),
            TaskState::Blocked => Some(WaitReason::Permission),
            _ => None,
        };
        task.verification = if state == TaskState::Completed {
            VerificationState::Passed
        } else {
            VerificationState::NotRun
        };
        task.validate()?;
        snapshot.cursor.sequence = index as u64 + 1;
        scenes.push(snapshot);
    }
    Ok(scenes)
}

fn display_directory(path: &std::path::Path, home: Option<&std::path::Path>) -> String {
    if let Some(home) = home.filter(|home| home.is_absolute())
        && let Ok(relative) = path.strip_prefix(home)
    {
        return if relative.as_os_str().is_empty() {
            "~".into()
        } else {
            format!("~/{}", relative.to_string_lossy())
        };
    }
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn display_directory_abbreviates_only_complete_home_components() {
        use std::path::Path;
        let home = Some(Path::new("/home/demo"));
        assert_eq!(
            super::display_directory(Path::new("/home/demo/projects/fluzo"), home),
            "~/projects/fluzo"
        );
        assert_eq!(super::display_directory(Path::new("/home/demo"), home), "~");
        assert_eq!(
            super::display_directory(Path::new("/home/demo-other/repo"), home),
            "/home/demo-other/repo"
        );
        assert_eq!(
            super::display_directory(Path::new("/tmp/repo"), None),
            "/tmp/repo"
        );
        assert_eq!(
            super::display_directory(Path::new("/tmp/repo"), Some(Path::new(""))),
            "/tmp/repo"
        );
    }

    #[test]
    fn scenes_are_valid_synthetic_protocol_data_without_commands() {
        use fluzo_core::application::*;
        let driver = fluzo_runtime::scenario::demo_driver().unwrap();
        let scenes = super::demo_scenes(&driver).unwrap();
        assert_eq!(scenes.len(), 7);
        for scene in scenes {
            assert!(scene.demo);
            assert_eq!(scene.protocol_version, PROTOCOL_VERSION);
            scene.tasks[0].validate().unwrap();
        }
        let QueryResponse::Snapshot(snapshot) = driver
            .query(QueryRequest {
                protocol_version: PROTOCOL_VERSION,
                query: Query::Tasks {
                    after: None,
                    limit: 64,
                },
            })
            .unwrap()
        else {
            panic!("snapshot expected");
        };
        assert_eq!(snapshot.tasks[0].state, TaskState::Pending);
    }
}
