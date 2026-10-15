//! `fluzo history` -- read-only listing and confirmed purge.
//!
//! The surface is deliberately narrow: it reads durable records and, when
//! confirmed, deletes them. It never starts an agent, never runs a tool
//! and never contacts a provider.
//!
//! Confirmation is required for any actual deletion and is independent
//! of any YOLO-style bypass: the only ways through are `--yes` or an
//! explicit affirmative answer at a terminal prompt. In a non-interactive
//! context with no `--yes` the command returns a structured
//! `confirmation_required` immediately rather than waiting for input.

use fluzo_runtime::history::{self};
use fluzo_runtime::purge::{Confirmation, PurgePlan};
use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "Usage: fluzo history list\n  \
                    fluzo history purge --session <id> [--dry-run]\n  \
                    fluzo history purge --all [--yes]\n\
                    --dry-run previews and deletes nothing. An actual purge requires --yes \
                    or an affirmative answer at the prompt; a non-interactive run without \
                    --yes returns confirmation_required. Nothing here starts an agent, \
                    runs a tool or contacts a provider.";

struct Request {
    purge: bool,
    ids: Vec<String>,
    all: bool,
    dry_run: bool,
    yes: bool,
}

pub fn run(arguments: &[OsString]) -> ExitCode {
    let request = match parse(arguments) {
        Ok(request) => request,
        Err(message) => return failure("invalid_arguments", &message),
    };
    let workspace = match std::env::current_dir() {
        Ok(path) => path,
        Err(_) => {
            return failure(
                "workspace_unavailable",
                "Selected workspace unavailable; nothing deleted.",
            );
        }
    };
    if !request.purge {
        return list(&workspace);
    }
    purge(&workspace, request)
}

fn list(workspace: &Path) -> ExitCode {
    if !history::drive_present(workspace) {
        println!("No sessions to purge.");
        return ExitCode::SUCCESS;
    }
    let sessions = match history::list(workspace) {
        Ok(sessions) => sessions,
        Err(error) => return failure(error.code(), &error.to_string()),
    };
    if sessions.is_empty() {
        println!("No sessions to purge.");
        return ExitCode::SUCCESS;
    }
    println!("Sessions in {}", workspace.to_string_lossy());
    for session in &sessions {
        let attached = if session.attached { " (attached)" } else { "" };
        println!(
            "  {:<24} {:<12} {:>9}  {} events  {} artifacts{}",
            session.id,
            session.kind,
            human(session.total_bytes()),
            session.event_count,
            session.artifact_count,
            attached
        );
    }
    let total: i64 = sessions.iter().map(|s| s.total_bytes()).sum();
    println!("Total: {} sessions, {}", sessions.len(), human(total));
    ExitCode::SUCCESS
}

fn purge(workspace: &Path, request: Request) -> ExitCode {
    if request.all && !request.yes {
        return failure(
            "confirmation_required",
            "history purge --all requires --yes; nothing deleted.",
        );
    }
    let ids: Vec<String> = if request.all {
        match history::list(workspace) {
            Ok(sessions) => sessions.iter().map(|s| s.id.clone()).collect(),
            Err(error) => return failure(error.code(), &error.to_string()),
        }
    } else {
        request.ids.clone()
    };
    if ids.is_empty() {
        println!("No sessions to purge.");
        return ExitCode::SUCCESS;
    }
    // The preview shown to the operator is the same computation the
    // delete will redo; the delete never trusts this one.
    let plan = match history::plan(workspace, &ids) {
        Ok(plan) => plan,
        Err(error) => return failure(error.code(), &error.to_string()),
    };
    if request.dry_run {
        print_plan(&plan, "Dry run: nothing was deleted.");
        return ExitCode::SUCCESS;
    }
    let confirmation = if request.yes {
        Confirmation::ExplicitYes
    } else {
        match prompt(&plan) {
            Prompt::Confirmed => Confirmation::Prompted,
            Prompt::Declined => {
                println!("Purge cancelled; nothing deleted.");
                return ExitCode::SUCCESS;
            }
            Prompt::Unavailable => {
                return failure(
                    "confirmation_required",
                    "history purge needs confirmation and this is not an interactive terminal; \
                     pass --yes to confirm. Nothing deleted.",
                );
            }
        }
    };
    let outcome = match history::purge(workspace, &ids, confirmation) {
        Ok(outcome) => outcome,
        Err(error) => return failure(error.code(), &error.to_string()),
    };
    print_plan(&outcome.plan, "Purged.");
    if outcome.files_pending.is_empty() {
        println!(
            "Released {} across {} session(s).",
            human(outcome.released_bytes),
            outcome.plan.sessions.len()
        );
    } else {
        // Row removal committed but some content files did not remove.
        // The retry pass finishes them; say so rather than claiming a
        // complete release.
        println!(
            "Incomplete: {} file(s) still pending removal ({}). Re-run \
             `fluzo history purge --all --yes` to finish them.",
            outcome.files_pending.len(),
            outcome.files_pending.join(", ")
        );
    }
    ExitCode::SUCCESS
}

enum Prompt {
    Confirmed,
    Declined,
    Unavailable,
}

fn prompt(plan: &PurgePlan) -> Prompt {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Prompt::Unavailable;
    }
    print_plan(plan, "About to delete:");
    print!(
        "Purge {} session(s) (~{})? [y/N] ",
        plan.sessions.len(),
        human(plan.total_bytes_released)
    );
    let _ = io::stdout().flush();
    let mut line = String::new();
    match io::stdin().read_line(&mut line) {
        Ok(0) => Prompt::Declined,
        Ok(_) => {
            let answer = line.trim().to_ascii_lowercase();
            if answer == "y" || answer == "yes" {
                Prompt::Confirmed
            } else {
                Prompt::Declined
            }
        }
        Err(_) => Prompt::Unavailable,
    }
}

fn print_plan(plan: &PurgePlan, heading: &str) {
    println!("{heading}");
    println!("  Repository: {}", plan.repository);
    println!("  Drive:      {}", plan.drive_path);
    for session in &plan.sessions {
        println!(
            "  Session {} ({}) - {} events, {} artifact(s) to remove, {} to preserve",
            session.id,
            session.kind,
            session.event_count,
            session.exclusive.len(),
            session.preserved.len()
        );
        for artifact in &session.exclusive {
            println!(
                "    remove   {:<24} {:<12} {}",
                artifact.id,
                artifact.kind,
                human(artifact.bytes)
            );
        }
        for artifact in &session.preserved {
            println!(
                "    preserve {:<24} {:<12} still held by {}",
                artifact.id,
                artifact.kind,
                artifact.held_by.join(", ")
            );
        }
    }
    println!(
        "  Total release: {} ({} artifact(s) preserved)",
        human(plan.total_bytes_released),
        plan.preserved_count()
    );
}

fn parse(arguments: &[OsString]) -> Result<Request, String> {
    let mut request = Request {
        purge: false,
        ids: Vec::new(),
        all: false,
        dry_run: false,
        yes: false,
    };
    let mut index = 0;
    // An optional leading `history` lets the caller pass the whole tail.
    if arguments.first().is_some_and(|value| value == "history") {
        index = 1;
    }
    let Some(raw_verb) = arguments.get(index) else {
        return Err(format!("history requires list or purge.\n{USAGE}"));
    };
    let Some(verb) = raw_verb.to_str() else {
        return Err("history arguments must be valid UTF-8.".to_string());
    };
    index += 1;
    match verb {
        "list" => {}
        "purge" => request.purge = true,
        other => return Err(format!("unknown history command '{other}'.\n{USAGE}")),
    }
    while index < arguments.len() {
        let Some(flag) = arguments[index].to_str() else {
            return Err("history arguments must be valid UTF-8.".to_string());
        };
        match flag {
            "--session" => {
                index += 1;
                let Some(value) = arguments.get(index).and_then(|v| v.to_str()) else {
                    return Err("--session requires an id.".to_string());
                };
                if value.trim().is_empty() {
                    return Err("--session requires a non-empty id.".to_string());
                }
                request.ids.push(value.to_string());
            }
            "--all" => request.all = true,
            "--dry-run" => request.dry_run = true,
            "--yes" => request.yes = true,
            "--help" | "-h" => return Err(format!("HELP\n{USAGE}")),
            other => {
                return Err(format!("unknown history option '{other}'.\n{USAGE}"));
            }
        }
        index += 1;
    }
    if !request.purge {
        if request.all || !request.ids.is_empty() || request.dry_run || request.yes {
            return Err("options apply to purge, not list.".to_string());
        }
        return Ok(request);
    }
    if request.all && !request.ids.is_empty() {
        return Err("--all and --session cannot be combined.".to_string());
    }
    if !request.all && request.ids.is_empty() {
        return Err("history purge needs --session <id> or --all.".to_string());
    }
    Ok(request)
}

/// Structured single-line failure on stderr, so a script can read the
/// code without parsing prose.
fn failure(code: &str, message: &str) -> ExitCode {
    let escaped: String = message
        .chars()
        .flat_map(|character| match character {
            '"' => vec!['\\', '"'],
            '\\' => vec!['\\', '\\'],
            '\n' => vec!['\\', 'n'],
            other => vec![other],
        })
        .collect();
    eprintln!("{{\"code\":\"{code}\",\"message\":\"{escaped}\"}}");
    ExitCode::FAILURE
}

fn human(bytes: i64) -> String {
    const KIB: i64 = 1024;
    const MIB: i64 = KIB * 1024;
    if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}
