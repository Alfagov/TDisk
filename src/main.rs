mod app;
mod filesystem;
#[cfg(target_os = "macos")]
mod fs_mac;
mod ui;

use filesystem::{Progress, ScanEvent};
use ratatui::crossterm::{
    self,
    event::{self, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    env, io,
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
    }
}

fn main() -> io::Result<()> {
    let mut args = env::args_os().skip(1);
    let first = args.next();
    let benchmark = first.as_deref() == Some(std::ffi::OsStr::new("--benchmark"));
    let path = if benchmark { args.next() } else { first }
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    if benchmark {
        return benchmark_scan(path);
    }

    let (tx, rx) = mpsc::channel();
    let progress = Arc::new(Progress::default());
    let worker_progress = Arc::clone(&progress);
    let worker_path = path.clone();
    // Start I/O before terminal setup, and immediately publish root entries once enumerated.
    thread::spawn(move || filesystem::scan(worker_path, &tx, &worker_progress));
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut app = app::App::new(path);
    let mut dirty = true;
    loop {
        // Drain queued results before drawing to avoid an extra frame of latency.
        loop {
            match rx.try_recv() {
                Ok(event) => {
                    app.apply(event);
                    dirty = true;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if app.scanning {
                        app.apply(ScanEvent::Failed(
                            "Scanner stopped before completion".into(),
                        ));
                        dirty = true;
                    }
                    break;
                }
            }
        }
        if dirty || app.scanning {
            terminal.draw(|frame| ui::draw(frame, &mut app, &progress))?;
            dirty = false;
        }
        // No idle redraws after scanning; poll quickly only while results are arriving.
        if event::poll(Duration::from_millis(if app.scanning { 33 } else { 250 }))? {
            match event::read()? {
                Event::Key(key) => {
                    if app.handle_key(key) {
                        break;
                    }
                    dirty = true;
                }
                Event::Resize(_, _) => dirty = true,
                _ => {}
            }
        }
    }
    Ok(())
}

fn benchmark_scan(path: PathBuf) -> io::Result<()> {
    let start = Instant::now();
    let (tx, rx) = mpsc::channel();
    let mut app = app::App::new(path.clone());
    let worker = thread::spawn(move || filesystem::scan(path, &tx, &Progress::default()));
    let mut first = None;
    let mut first_directory = None;
    let mut finished = false;
    for event in rx {
        match &event {
            ScanEvent::Started(_) => first = Some(start.elapsed()),
            ScanEvent::Directory { .. } => {
                first_directory.get_or_insert_with(|| start.elapsed());
            }
            ScanEvent::Finished => finished = true,
            ScanEvent::Failed(message) => return Err(io::Error::other(message.clone())),
        }
        app.apply(event);
        if finished {
            break;
        }
    }
    worker
        .join()
        .map_err(|_| io::Error::other("scanner panicked"))?;
    if !finished {
        return Err(io::Error::other("scanner stopped before completion"));
    }
    let scan = start.elapsed();
    let refresh_start = Instant::now();
    app.refresh();
    let refresh = refresh_start.elapsed();
    let navigation_start = Instant::now();
    app.drill_down();
    app.go_up();
    let navigation = navigation_start.elapsed();
    let root = app.root.as_ref().unwrap();
    println!(
        "bytes={} errors={} first_entries_ms={:.3} first_directory_ms={} total_ms={:.3} load_ms={:.3} navigation_ms={:.3}",
        root.size,
        root.errors,
        first.unwrap_or_default().as_secs_f64() * 1000.0,
        first_directory
            .map(|time| format!("{:.3}", time.as_secs_f64() * 1000.0))
            .unwrap_or_else(|| "n/a".into()),
        scan.as_secs_f64() * 1000.0,
        refresh.as_secs_f64() * 1000.0,
        navigation.as_secs_f64() * 1000.0
    );
    Ok(())
}
