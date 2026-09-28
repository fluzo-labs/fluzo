use std::io::{self, IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use fluzo_core::application::{
    ApplicationPort, PROTOCOL_VERSION, Query, QueryRequest, QueryResponse,
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::shell::Shell;

struct TerminalGuard {
    raw: bool,
    screen: bool,
    paste: bool,
    cursor: bool,
}

impl TerminalGuard {
    fn enter(output: &mut impl Write) -> io::Result<Self> {
        let mut guard = Self {
            raw: false,
            screen: false,
            paste: false,
            cursor: false,
        };
        let result = (|| {
            enable_raw_mode()?;
            guard.raw = true;
            guard.screen = true;
            execute!(output, EnterAlternateScreen)?;
            guard.paste = true;
            execute!(output, EnableBracketedPaste)?;
            guard.cursor = true;
            execute!(output, Hide)
        })();
        if let Err(error) = result {
            let cleanup = guard.restore(output);
            return Err(combine(error, cleanup));
        }
        Ok(guard)
    }

    fn restore(&mut self, output: &mut impl Write) -> io::Result<()> {
        let mut errors = Vec::new();
        if self.paste {
            match execute!(output, DisableBracketedPaste) {
                Ok(()) => self.paste = false,
                Err(error) => errors.push(error),
            }
        }
        if self.cursor {
            match execute!(output, Show) {
                Ok(()) => self.cursor = false,
                Err(error) => errors.push(error),
            }
        }
        if self.screen {
            match execute!(output, LeaveAlternateScreen) {
                Ok(()) => self.screen = false,
                Err(error) => errors.push(error),
            }
        }
        if self.raw {
            match disable_raw_mode() {
                Ok(()) => self.raw = false,
                Err(error) => errors.push(error),
            }
        }
        if let Err(error) = output.flush() {
            errors.push(error);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "Terminal restoration failed ({} errors).",
                errors.len()
            )))
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore(&mut io::stdout());
    }
}

fn combine(error: io::Error, cleanup: io::Result<()>) -> io::Error {
    match cleanup {
        Ok(()) => error,
        Err(cleanup) => io::Error::other(format!("{error}; {cleanup}")),
    }
}

struct Signals(Vec<signal_hook::SigId>);
impl Drop for Signals {
    fn drop(&mut self) {
        for identity in self.0.drain(..) {
            signal_hook::low_level::unregister(identity);
        }
    }
}

pub fn run_demo(port: &dyn ApplicationPort) -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other(
            "Interactive demo requires terminal stdin and stdout; use 'fluzo demo' for plain output.",
        ));
    }
    let snapshot = match port.query(QueryRequest {
        protocol_version: PROTOCOL_VERSION,
        query: Query::Tasks {
            after: None,
            limit: 64,
        },
    }) {
        Ok(QueryResponse::Snapshot(snapshot)) => snapshot,
        _ => {
            return Err(io::Error::other(
                "Synthetic snapshot unavailable; no work was started.",
            ));
        }
    };
    let mut shell = Shell::new(snapshot).map_err(io::Error::other)?;
    let stop = Arc::new(AtomicBool::new(false));
    let mut signals = Signals(Vec::new());
    for signal in [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
        signal_hook::consts::SIGINT,
    ] {
        signals
            .0
            .push(signal_hook::flag::register(signal, stop.clone())?);
    }
    let color = std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty());
    let mut output = io::stdout();
    let mut guard = TerminalGuard::enter(&mut output)?;
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> io::Result<()> {
        let mut terminal = Terminal::new(CrosstermBackend::new(&mut output))?;
        let mut dirty = true;
        let mut next = Instant::now();
        let mut emitted = 0;
        while !shell.quit && !stop.load(Ordering::Relaxed) {
            if dirty {
                terminal.draw(|frame| shell.render(frame, color))?;
                dirty = false;
            }
            if event::poll(Duration::from_millis(50))? {
                match event::read()? {
                    Event::Key(key) => shell.key(key),
                    Event::Paste(text) => shell.paste(&text),
                    Event::Resize(_, _) => {}
                    _ => continue,
                }
                dirty = true;
            }
            if shell.playing && Instant::now() >= next {
                shell.stream(&format!(
                    "Synthetic fragment {}: input remains yours. No tool has run.",
                    emitted + 1
                ));
                emitted += 1;
                next = Instant::now() + Duration::from_millis(100);
                dirty = true;
                if emitted == 80 {
                    shell.playing = false;
                    emitted = 0;
                    shell.status =
                        "Synthetic playback ended; no runtime completion implied.".into();
                }
            }
        }
        Ok(())
    }));
    let cleanup = guard.restore(&mut output);
    std::panic::set_hook(previous_hook);
    match result {
        Ok(Ok(())) => cleanup,
        Ok(Err(error)) => Err(combine(error, cleanup)),
        Err(_) => Err(combine(
            io::Error::other("Interactive preview stopped after an internal error."),
            cleanup,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailingWriter {
        attempts: usize,
    }
    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            self.attempts += 1;
            Err(io::Error::other("synthetic write failure"))
        }
        fn flush(&mut self) -> io::Result<()> {
            self.attempts += 1;
            Err(io::Error::other("synthetic flush failure"))
        }
    }

    #[test]
    fn restoration_attempts_all_modes_despite_write_failure() {
        let mut guard = TerminalGuard {
            raw: false,
            screen: true,
            paste: true,
            cursor: true,
        };
        let mut writer = FailingWriter { attempts: 0 };
        let error = guard.restore(&mut writer).unwrap_err();
        assert!(writer.attempts >= 4);
        assert!(
            combine(io::Error::other("original error"), Err(error))
                .to_string()
                .contains("original error; Terminal restoration failed")
        );
        guard.screen = false;
        guard.paste = false;
        guard.cursor = false;
    }
}
