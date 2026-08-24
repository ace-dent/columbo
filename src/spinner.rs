// SPDX-License-Identifier: MIT

//! Shared terminal pulse for CLI and detailed progress modes.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::presentation::{countdown_seconds, write_spinner_line};
use crate::terminal;

const TICK: Duration = Duration::from_secs(1);
const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

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
        let color = terminal::stderr_color_enabled();
        let worker = thread::Builder::new()
            .spawn(move || {
                let mut frame = 0;
                let mut drawn = false;
                thread::park_timeout(TICK);
                while worker_running.load(Ordering::Relaxed) {
                    let seconds =
                        countdown_seconds(deadline.saturating_duration_since(Instant::now()));
                    {
                        let stderr = io::stderr();
                        let mut output = stderr.lock();
                        let _ = write_spinner_line(&mut output, FRAMES[frame], seconds, color);
                        let _ = output.flush();
                    }
                    drawn = true;
                    frame = (frame + 1) % FRAMES.len();
                    thread::park_timeout(TICK);
                }
                if drawn {
                    eprint!("\r\x1b[K");
                    let _ = io::stderr().flush();
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
