use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() == 1 {
        if arguments[0] == "--version" {
            println!("fluzo {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        if arguments[0] == "--help" {
            println!("Fluzo development bootstrap\nUsage: fluzo --help | --version");
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
