use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
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
            println!("Fluzo development bootstrap\nUsage: fluzo --help | --version | demo");
            println!(
                "{}",
                fluzo_tui::availability_text(fluzo_runtime::availability())
            );
            return ExitCode::SUCCESS;
        }
    }
    eprintln!(
        "{}",
        fluzo_tui::availability_text(fluzo_runtime::availability())
    );
    ExitCode::FAILURE
}
