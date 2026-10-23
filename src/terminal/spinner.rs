// SPDX-License-Identifier: MIT
// SPDX-FileCopyrightText: 2026 Andrew C.E. Dent <https://github.com/ace-dent>

//! Stderr spinner with a timeout countdown, shared by the CLI and the verbose
//! reporter.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::terminal;
use crate::terminal::formatting::{
    clear_spinner_line, countdown_seconds, write_spinner_line, SpinnerStyle,
};

const TICK: Duration = Duration::from_secs(1);
const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// ASCII frames for legacy consoles that print escapes literally; their fonts
/// usually lack Braille patterns.
const PLAIN_FRAMES: [&str; 4] = ["|", "/", "-", "\\"];

pub(crate) struct Spinner {
    running: Option<Arc<AtomicBool>>,
    worker: Option<JoinHandle<()>>,
}

impl Spinner {
    pub(crate) fn start(enabled: bool, deadline: Instant) -> Self {
        if !enabled || !terminal::stderr_interactive() {
            return Self {
                running: None,
                worker: None,
            };
        }

        let running = Arc::new(AtomicBool::new(true));
        let worker_running = Arc::clone(&running);
        let (style, frames): (_, &[&str]) = if terminal::stderr_escapes_enabled() {
            let color = terminal::stderr_color_enabled();
            (SpinnerStyle::Ansi { color }, &FRAMES)
        } else {
            (SpinnerStyle::Plain, &PLAIN_FRAMES)
        };
        let worker = thread::Builder::new()
            .spawn(move || {
                let mut frame = 0;
                let mut drawn = None;
                thread::park_timeout(TICK);
                while worker_running.load(Ordering::Relaxed) {
                    let seconds =
                        countdown_seconds(deadline.saturating_duration_since(Instant::now()));
                    {
                        let stderr = io::stderr();
                        let mut output = stderr.lock();
                        let previous = drawn.unwrap_or(0);
                        let width = write_spinner_line(
                            &mut output,
                            frames[frame],
                            seconds,
                            style,
                            previous,
                        );
                        let _ = output.flush();
                        // Pad and clear to the widest frame drawn so far.
                        drawn = Some(width.unwrap_or(previous).max(previous));
                    }
                    frame = (frame + 1) % frames.len();
                    thread::park_timeout(TICK);
                }
                if let Some(width) = drawn {
                    let stderr = io::stderr();
                    let mut output = stderr.lock();
                    let _ = clear_spinner_line(&mut output, style, width);
                    let _ = output.flush();
                }
            })
            .ok();
        Self {
            running: worker.as_ref().map(|_| running),
            worker,
        }
    }

    pub(crate) fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            running.store(false, Ordering::Relaxed);
        }
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop();
    }
}
