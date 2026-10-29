use super::output_history::OutputHistory;
use crate::application::{Context, Error, Event, Progress, Result};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event as TerminalEvent, KeyCode,
        KeyEventKind, KeyModifiers, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    text::{Line, Text},
    widgets::{Block, Borders, Gauge, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use std::{
    io,
    path::PathBuf,
    sync::{
        atomic::Ordering,
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

pub struct EventChannel(pub SyncSender<Event>);
impl Progress for EventChannel {
    fn emit(&self, e: Event) {
        match e {
            Event::Stage(_) | Event::Log(_) | Event::Warning(_) => {
                let _ = self.0.send(e);
            }
            _ => {
                let _ = self.0.try_send(e);
            }
        }
    }
}
pub fn channel() -> (std::sync::Arc<dyn Progress>, Receiver<Event>) {
    let (tx, rx) = mpsc::sync_channel(128);
    (std::sync::Arc::new(EventChannel(tx)), rx)
}
struct Screen;
impl Drop for Screen {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stderr(),
            DisableMouseCapture,
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
    }
}

pub fn monitor<T: Send + 'static>(
    tui: bool,
    input: String,
    ctx: Context,
    events: Receiver<Event>,
    job: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    let mut terminal = if tui {
        enable_raw_mode()?;
        let screen = Screen;
        execute!(io::stderr(), EnterAlternateScreen, EnableMouseCapture)?;
        let terminal = Terminal::new(CrosstermBackend::new(io::stderr()))?;
        Some((screen, terminal))
    } else {
        None
    };
    let worker = thread::spawn(job);
    let started = Instant::now();
    let mut stage = "Preparing";
    let mut percent = None;
    let mut logs = OutputHistory::default();
    let mut output_area = Rect::default();
    let mut last_log_percent = -10.0;
    let display_result: Result<()> = (|| {
        loop {
            // A final pass after the worker stops drains every remaining output event.
            let finished = worker.is_finished();
            for e in events.try_iter().take(512) {
                match e {
                    Event::Stage(s) => {
                        stage = s;
                        percent = None;
                        last_log_percent = -10.0;
                        if !tui {
                            eprintln!("[{stage}]");
                        }
                    }
                    Event::Progress(p) => {
                        percent = Some(p);
                        if !tui && p - last_log_percent >= 10.0 {
                            eprintln!("  {p:.0}%");
                            last_log_percent = p;
                        }
                    }
                    Event::Log(s) => {
                        if tui {
                            logs.push(s);
                        }
                    }
                    Event::Warning(s) => {
                        if tui {
                            logs.push(format!("WARNING: {s}"));
                        } else {
                            eprintln!("autosrt: warning: {s}");
                        }
                    }
                }
            }
            if let Some((_, term)) = terminal.as_mut() {
                term.draw(|f| {
                    let areas = Layout::vertical([
                        Constraint::Length(3),
                        Constraint::Length(3),
                        Constraint::Min(3),
                        Constraint::Length(2),
                    ])
                    .split(f.area());
                    f.render_widget(
                        Paragraph::new(format!(
                            "autosrt — {input}\n{stage} · {}s",
                            started.elapsed().as_secs()
                        ))
                        .block(Block::default().borders(Borders::BOTTOM)),
                        areas[0],
                    );
                    let label = if ctx.cancel.load(Ordering::Relaxed) {
                        "Cancelling…".into()
                    } else {
                        percent.map_or_else(|| "Working…".into(), |p| format!("{p:.0}%"))
                    };
                    f.render_widget(
                        Gauge::default()
                            .block(Block::bordered().title(stage))
                            .ratio(percent.unwrap_or(0.0) / 100.0)
                            .label(label),
                        areas[1],
                    );
                    output_area = areas[2];
                    render_history(f, output_area, &mut logs);
                    f.render_widget(
                        Paragraph::new(
                            "↑/↓ j/k · PgUp/PgDn · wheel: scroll · Home: first · End: follow
q / Ctrl+C: cancel · SRT path is printed on completion",
                        ),
                        areas[3],
                    );
                })?;
                if event::poll(Duration::from_millis(50))? {
                    match event::read()? {
                        TerminalEvent::Key(key) if key.kind != KeyEventKind::Release => {
                            if key.code == KeyCode::Char('q')
                                || (key.code == KeyCode::Char('c')
                                    && key.modifiers.contains(KeyModifiers::CONTROL))
                            {
                                ctx.cancel.store(true, Ordering::Relaxed);
                            } else {
                                match key.code {
                                    KeyCode::Up | KeyCode::Char('k') => logs.up(1),
                                    KeyCode::Down | KeyCode::Char('j') => logs.down(1),
                                    KeyCode::PageUp => logs.up(logs.height().max(1)),
                                    KeyCode::PageDown => logs.down(logs.height().max(1)),
                                    KeyCode::Home => logs.home(),
                                    KeyCode::End => logs.follow(),
                                    _ => {}
                                }
                            }
                        }
                        TerminalEvent::Mouse(mouse)
                            if mouse.column >= output_area.x
                                && mouse.column < output_area.right()
                                && mouse.row >= output_area.y
                                && mouse.row < output_area.bottom() =>
                        {
                            match mouse.kind {
                                MouseEventKind::ScrollUp => logs.up(3),
                                MouseEventKind::ScrollDown => logs.down(3),
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                }
            } else {
                thread::sleep(Duration::from_millis(50));
            }
            if finished {
                break;
            }
        }
        Ok(())
    })();
    if display_result.is_err() {
        ctx.cancel.store(true, Ordering::Relaxed);
    }
    // Keep draining if the terminal failed, so a stage event cannot block shutdown.
    while !worker.is_finished() {
        let _ = events.recv_timeout(Duration::from_millis(50));
    }
    let result = worker
        .join()
        .map_err(|_| Error::Message("Pipeline worker panicked".into()));
    drop(terminal.take());
    display_result?;
    result?
}

pub fn output_path(path: PathBuf) {
    println!("{}", path.display());
}

fn render_history(frame: &mut ratatui::Frame, area: Rect, history: &mut OutputHistory) {
    let block = Block::bordered();
    let mut inner = block.inner(area);
    inner.width = inner.width.saturating_sub(1);
    history.resize(inner.width as usize, inner.height as usize);
    let mode = if history.following() {
        "FOLLOW"
    } else {
        "SCROLL · End to follow"
    };
    let title = format!("Output history · {mode} · {} rows", history.len());
    frame.render_widget(block.title(title), area);
    let lines: Vec<_> = history
        .visible()
        .iter()
        .map(|s| Line::raw(s.as_str()))
        .collect();
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
    let mut state = ScrollbarState::new(history.len())
        .position(history.offset())
        .viewport_content_length(history.height());
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight),
        area,
        &mut state,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn log_events_are_not_dropped_when_queue_is_full() {
        let (sender, receiver) = channel();
        let producer = thread::spawn(move || {
            for i in 0..1024 {
                sender.emit(Event::Log(format!("caption {i}")));
            }
        });
        let lines: Vec<_> = receiver
            .iter()
            .map(|event| match event {
                Event::Log(s) => s,
                _ => panic!("unexpected event"),
            })
            .collect();
        producer.join().unwrap();
        assert_eq!(lines.len(), 1024);
        assert_eq!(lines[0], "caption 0");
        assert_eq!(lines[1023], "caption 1023");
    }
    #[test]
    fn rendering_shows_tail_then_earlier_captions_after_scroll() {
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(60, 8)).unwrap();
        let mut history = OutputHistory::default();
        for i in 0..20 {
            history.push(format!("caption {i:02}"));
        }
        let draw = |terminal: &mut Terminal<ratatui::backend::TestBackend>,
                    history: &mut OutputHistory| {
            terminal
                .draw(|f| render_history(f, f.area(), history))
                .unwrap();
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>()
        };
        let text = draw(&mut terminal, &mut history);
        assert!(text.contains("caption 19"));
        assert!(!text.contains("caption 00"));
        history.home();
        assert!(draw(&mut terminal, &mut history).contains("caption 00"));
        history.push("caption 20".into());
        let text = draw(&mut terminal, &mut history);
        assert!(text.contains("caption 00"));
        assert!(!text.contains("caption 20"));
        history.follow();
        assert!(draw(&mut terminal, &mut history).contains("caption 20"));
    }
}
