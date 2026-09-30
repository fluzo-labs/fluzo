use std::io::{self, IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use fluzo_core::application::{
    ApplicationPort, PROTOCOL_VERSION, Query, QueryRequest, QueryResponse, Snapshot,
};
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::shell::Shell;
use crate::visual::{AnimationClock, Preferences, VisualOptions};

struct TerminalGuard {
    raw: bool,
    screen: bool,
    paste: bool,
    cursor: bool,
    mouse: bool,
    keyboard: bool,
    focus: bool,
}

impl TerminalGuard {
    fn enter(output: &mut impl Write) -> io::Result<Self> {
        let mut guard = Self {
            raw: false,
            screen: false,
            paste: false,
            cursor: false,
            mouse: false,
            keyboard: false,
            focus: false,
        };
        let result = (|| {
            enable_raw_mode()?;
            guard.raw = true;
            guard.screen = true;
            execute!(output, EnterAlternateScreen)?;
            guard.paste = true;
            execute!(output, EnableBracketedPaste)?;
            guard.mouse = true;
            execute!(output, EnableMouseCapture)?;
            guard.cursor = true;
            execute!(output, Hide)?;
            guard.focus = true;
            execute!(output, event::EnableFocusChange)?;
            if std::env::var("TERM")
                .is_ok_and(|term| matches!(term.as_str(), "xterm-ghostty" | "xterm-kitty"))
            {
                guard.keyboard = true;
                execute!(
                    output,
                    event::PushKeyboardEnhancementFlags(
                        event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    )
                )?;
            }
            Ok::<(), io::Error>(())
        })();
        if let Err(error) = result {
            let cleanup = guard.restore(output);
            return Err(combine(error, cleanup));
        }
        Ok(guard)
    }

    fn restore(&mut self, output: &mut impl Write) -> io::Result<()> {
        let mut errors = Vec::new();
        if self.focus {
            match execute!(output, event::DisableFocusChange) {
                Ok(()) => self.focus = false,
                Err(error) => errors.push(error),
            }
        }
        if self.keyboard {
            match execute!(output, event::PopKeyboardEnhancementFlags) {
                Ok(()) => self.keyboard = false,
                Err(error) => errors.push(error),
            }
        }
        if self.mouse {
            match execute!(output, DisableMouseCapture) {
                Ok(()) => self.mouse = false,
                Err(error) => errors.push(error),
            }
        }
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

pub fn run_demo(
    port: &dyn ApplicationPort,
    options: VisualOptions,
    scenes: &[Snapshot],
) -> io::Result<()> {
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
    shell.set_repository(&options.repository);
    shell.preferences = Preferences::new(options).map_err(io::Error::other)?;
    if scenes.len() > 16 {
        return Err(io::Error::other("Too many synthetic scenes."));
    }
    for scene in scenes {
        Shell::new(scene.clone()).map_err(io::Error::other)?;
    }
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
    let limited = std::env::var("TERM").is_ok_and(|value| value == "dumb" || value == "linux");
    let color = !limited && std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty());
    shell.preferences.ascii |= limited;
    shell.truecolor = !limited
        && (std::env::var("COLORTERM")
            .is_ok_and(|value| matches!(value.as_str(), "truecolor" | "24bit"))
            || std::env::var("TERM").is_ok_and(|value| value == "xterm-ghostty"));
    let synchronized = std::env::var("TERM").is_ok_and(|value| value == "xterm-ghostty");
    shell.notification_transport_supported = synchronized;
    let mut notifications = crate::notification::Notifications::new(
        shell.preferences.effective().notifications.desktop_enabled,
        &std::env::var("TERM").unwrap_or_default(),
    );
    let mut output = io::stdout();
    let mut guard = TerminalGuard::enter(&mut output)?;
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> io::Result<()> {
        let mut terminal = Terminal::new(CrosstermBackend::new(&mut output))?;
        let mut dirty = true;
        let mut next = Instant::now();
        let mut emitted = 0;
        let started = Instant::now();
        let mut animation = AnimationClock::default();
        let mut cached: Option<ratatui::buffer::Buffer> = None;
        let mut geometry = crate::shell::RenderGeometry::default();
        let mut scene_index = 0;
        let mut last_size = terminal.size()?;
        let mut last_timer_second = None;
        let mut last_scrollbar_visible = false;
        while !shell.quit && !stop.load(Ordering::Relaxed) {
            let size = terminal.size()?;
            if size != last_size {
                terminal.resize(ratatui::layout::Rect::new(0, 0, size.width, size.height))?;
                cached = None;
                dirty = true;
                last_size = size;
            }
            shell.resize(ratatui::layout::Rect::new(0, 0, size.width, size.height));
            shell.elapsed = started.elapsed();
            dirty |= shell.advance_notifications();
            let scrollbar_visible = shell.scrollbar_visible();
            if scrollbar_visible != last_scrollbar_visible {
                dirty = true;
                last_scrollbar_visible = scrollbar_visible;
            }
            let timer_second = shell.loading_timer_second();
            if timer_second != last_timer_second {
                dirty = true;
                last_timer_second = timer_second;
            }
            let animate = animation.tick(
                shell.elapsed,
                shell.presentation_state(),
                shell.state_age(),
                shell.preferences.effective(),
            );
            shell.skipped = animation.skipped;
            if dirty || animate {
                let drawing = Instant::now();
                if synchronized {
                    execute!(
                        terminal.backend_mut(),
                        crossterm::terminal::BeginSynchronizedUpdate
                    )?;
                }
                let draw_result = terminal
                    .draw(|frame| {
                        if !dirty
                            && let Some(buffer) = &cached
                            && buffer.area == frame.area()
                        {
                            frame.buffer_mut().clone_from(buffer);
                            shell.render_identity(frame, color, geometry);
                        } else {
                            geometry = shell.render_frame(frame, color);
                        }
                        cached = Some(frame.buffer_mut().clone());
                    })
                    .map(|_| ());
                let sync_result = if synchronized {
                    execute!(
                        terminal.backend_mut(),
                        crossterm::terminal::EndSynchronizedUpdate
                    )
                } else {
                    Ok(())
                };
                draw_result?;
                sync_result?;
                shell.redraws = shell.redraws.saturating_add(1);
                shell.frame_micros = drawing.elapsed().as_micros();
                dirty = false;
            }
            if event::poll(animation.wait(started.elapsed()))? {
                let mut changed = true;
                match event::read()? {
                    Event::Key(key) => shell.key(key),
                    Event::Paste(text) => shell.paste(&text),
                    Event::FocusGained => notifications.focus(true),
                    Event::FocusLost => notifications.focus(false),
                    Event::Resize(_, _) => {
                        cached = None;
                    }
                    Event::Mouse(mouse) => {
                        let size = terminal.size()?;
                        changed = shell.mouse(
                            mouse,
                            ratatui::layout::Rect::new(0, 0, size.width, size.height),
                        );
                    }
                }
                dirty |= changed;
            }
            notifications.set_enabled(shell.preferences.applied().notifications.desktop_enabled);
            if shell.notification_test {
                shell.notification_test = false;
                shell.status = notifications
                    .schedule_message(started.elapsed(), shell.notification_text())
                    .into();
                dirty = true;
            }
            if let Some(result) = notifications.tick(started.elapsed(), terminal.backend_mut()) {
                shell.status = match result {
                    Ok(true) => {
                        "Notification test sent to terminal; desktop delivery is not confirmed."
                    }
                    Ok(false) => notifications
                        .suppression_reason()
                        .unwrap_or("Notification suppressed."),
                    Err(_) => "Notification delivery failed; no automatic retry.",
                }
                .into();
                dirty = true;
            }
            if shell.next_scene {
                shell.next_scene = false;
                if let Some(scene) = scenes.get(scene_index) {
                    shell
                        .set_snapshot(scene.clone())
                        .map_err(io::Error::other)?;
                    scene_index = (scene_index + 1) % scenes.len();
                    dirty = true;
                }
            }
            if shell.playing && Instant::now() >= next {
                shell.synthetic_fragment_tick(emitted);
                emitted += 1;
                next = Instant::now() + Duration::from_millis(700);
                dirty = true;
                if emitted == 80 {
                    shell.finish_playback();
                    emitted = 0;
                    shell.status = match notifications.completed(terminal.backend_mut()) {
                        Ok(true) => "Playback ended. Notification sent to terminal; desktop display unconfirmed.",
                        Ok(false) => notifications.suppression_reason().unwrap_or("Notification suppressed."),
                        Err(_) => "Playback ended. Notification delivery failed; no automatic retry.",
                    }.into();
                }
            }
        }
        Ok(())
    }));
    if synchronized {
        let _ = execute!(output, crossterm::terminal::EndSynchronizedUpdate);
    }
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
            mouse: true,
            keyboard: true,
            focus: true,
        };
        let mut writer = FailingWriter { attempts: 0 };
        let error = guard.restore(&mut writer).unwrap_err();
        assert!(writer.attempts >= 7);
        assert!(
            combine(io::Error::other("original error"), Err(error))
                .to_string()
                .contains("original error; Terminal restoration failed")
        );
        guard.screen = false;
        guard.paste = false;
        guard.cursor = false;
        guard.mouse = false;
        guard.keyboard = false;
        guard.focus = false;
    }
}
