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
                "Fluzo development bootstrap\nUsage: fluzo --help | --version | demo [--interactive]\nInteractive options: --animation-fps <0..60> (default 60; alternatives 30/15/0)\n  --reduced-motion --theme <default|high-contrast> --ascii\n  --dev-menu | --no-dev-menu (disable wins)\n  --desktop-notifications (opt-in; Ghostty OSC 777, unfocused only)\n  Developer menu: Ctrl+T schedules a notification test in 3 seconds\nRead-only demo: no .fluzo loading or saving; visual options require demo --interactive."
            );
            println!(
                "Setup: fluzo [init] [--config <workspace-relative path>] [visual options]\nMissing configuration opens offline setup interactively; headless returns configuration_required.\nExisting files are preserved: replacement requires a controlled host. No connectivity probes."
            );
            println!(
                "{}",
                fluzo_tui::availability_text(fluzo_runtime::availability())
            );
            return ExitCode::SUCCESS;
        }
    }
    startup::run(&arguments)
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
