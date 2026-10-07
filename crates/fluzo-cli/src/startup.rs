use std::ffi::OsString;
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use fluzo_core::configuration::{ConfigurationError, ConfigurationPort, Discovery, Edit};
use fluzo_runtime::configuration::{ConfigurationService, WritePolicy};

fn result(code: &str, detail: &str) -> ExitCode {
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

pub fn run(arguments: &[OsString]) -> ExitCode {
    let explicit = arguments.first().is_some_and(|argument| argument == "init");
    let mut config = None;
    let mut visual = Vec::new();
    let mut index = usize::from(explicit);
    while index < arguments.len() {
        if arguments[index] == "--config" {
            index += 1;
            let Some(path) = arguments.get(index) else {
                return result(
                    "invalid_arguments",
                    "--config requires a path; agent runtime is not implemented.",
                );
            };
            if config.is_some() {
                return result(
                    "invalid_arguments",
                    "Duplicate --config; agent runtime is not implemented.",
                );
            }
            config = Some(PathBuf::from(path));
        } else if let Some(value) = arguments[index].to_str() {
            visual.push(value.to_owned());
        } else {
            return result(
                "invalid_arguments",
                "Options must be valid UTF-8; agent runtime is not implemented.",
            );
        }
        index += 1;
    }
    let options = match fluzo_tui::visual::VisualOptions::parse(&visual) {
        Ok(options) => options,
        Err(error) => {
            return result(
                "invalid_arguments",
                &format!("{error} Agent runtime is not implemented."),
            );
        }
    };
    let workspace = match std::env::current_dir() {
        Ok(path) => path,
        Err(_) => {
            return result(
                "configuration_inaccessible",
                "Selected workspace unavailable; agent runtime is not implemented.",
            );
        }
    };
    let target = config
        .as_ref()
        .map(|path| {
            if path.is_absolute() {
                path.clone()
            } else {
                workspace.join(path)
            }
        })
        .unwrap_or_else(|| workspace.join(".fluzo"));
    let settings = fluzo_core::settings::Settings {
        tui: options.settings.clone(),
        ..Default::default()
    };
    let overrides = settings
        .entries()
        .into_iter()
        .filter(|(descriptor, _)| options.locked.contains(&descriptor.key))
        .map(|(descriptor, value)| Edit::Set {
            key: descriptor.key,
            value,
        })
        .collect();
    let mut service = match ConfigurationService::start(
        workspace,
        config,
        overrides,
        WritePolicy::LocalWorkspace,
    ) {
        Ok(service) => service,
        Err(error) => {
            return result(
                "configuration_unavailable",
                &fluzo_tui::setup::diagnostic(&error),
            );
        }
    };
    if io::stdin().is_terminal() && io::stdout().is_terminal() {
        let mut discovery = match fluzo_runtime::model_discovery::ModelDiscoveryService::start() {
            Ok(service) => service,
            Err(_) => {
                return result(
                    "discovery_unavailable",
                    "Model discovery worker unavailable; no network request sent.",
                );
            }
        };
        let outcome = fluzo_tui::terminal::run_configuration(
            &mut service,
            &mut discovery,
            target.to_string_lossy().into_owned(),
            explicit,
            options.ascii,
            options.safe_screen_settings,
        );
        service.quiesce();
        return match outcome {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => result(
                "setup_closed",
                "Setup closed without confirmed completion; dispatched operations are not rolled back. Agent runtime is not implemented.",
            ),
            Err(_) => result(
                "terminal_error",
                "Terminal setup failed; any dispatched write requires reconciliation.",
            ),
        };
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let snapshot = loop {
        match service.snapshot() {
            Ok(snapshot) => break snapshot,
            Err(ConfigurationError::Busy) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1))
            }
            Err(error) => {
                return result(
                    "configuration_unavailable",
                    &format!(
                        "{}: {}",
                        fluzo_tui::setup::safe_text(&target.to_string_lossy()),
                        fluzo_tui::setup::diagnostic(&error)
                    ),
                );
            }
        }
    };
    service.quiesce();
    let path = fluzo_tui::setup::safe_text(&target.to_string_lossy());
    if let Some(error) = snapshot.problem {
        return result(
            if snapshot.discovery == Discovery::Inaccessible {
                "configuration_inaccessible"
            } else {
                "configuration_invalid"
            },
            &format!(
                "{path}: {}; original preserved; agent runtime is not implemented.",
                fluzo_tui::setup::diagnostic(&error)
            ),
        );
    }
    match snapshot.discovery {
        Discovery::Missing => result(
            "configuration_required",
            &format!(
                "{path}: run fluzo init in a terminal; no file created. Agent runtime is not implemented."
            ),
        ),
        Discovery::Valid if explicit => result(
            "replacement_unavailable",
            &format!(
                "{path}: existing configuration preserved; replacement requires a controlled host."
            ),
        ),
        Discovery::Valid => result(
            "runtime_unavailable",
            "Configuration valid; agent runtime is not implemented. No provider contacted.",
        ),
        _ => result(
            "configuration_inaccessible",
            &format!("{path}: configuration unavailable; original preserved."),
        ),
    }
}
