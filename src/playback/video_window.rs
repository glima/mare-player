// SPDX-License-Identifier: MIT

//! Handle to the out-of-process music-video window (`mare-video-window`).
//!
//! A COSMIC panel applet cannot spawn a real toplevel window — the libcosmic
//! applet runtime parents every surface it creates back into the panel. So to
//! "pop out" a music video we launch a tiny companion process that re-plays the
//! same HLS stream in its own window (the window GStreamer's video sink
//! creates). No decoded frames cross the process boundary: the child decodes
//! the stream itself.
//!
//! This type owns the child process and the pipe to its stdin. A background
//! thread reads the child's stdout (one event per line) and forwards each line
//! over an [`tokio::sync::mpsc::UnboundedSender`] that the app drains via a
//! subscription. See `docs/video-popout-plan.md` and the companion crate
//! `mare-video-window` for the line protocol.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

use tokio::sync::mpsc::UnboundedSender;

/// A running `mare-video-window` child process and the writer half of its
/// stdin pipe. Dropping (or [`kill`](Self::kill)ing) it terminates the child.
pub struct VideoWindowChild {
    child: Child,
    stdin: ChildStdin,
}

impl VideoWindowChild {
    /// Launch the companion window playing `url`, resuming at `position`
    /// seconds with the given perceptual `volume` and video `preamp_db`.
    ///
    /// Each line the child writes to stdout is forwarded over `events`. Returns
    /// `None` if the binary could not be located or spawned (the caller then
    /// falls back to inline playback).
    pub fn spawn(
        url: &str,
        position: f64,
        volume: f32,
        preamp_db: f32,
        events: UnboundedSender<String>,
    ) -> Option<Self> {
        let exe = locate_binary();
        let mut child = match Command::new(&exe)
            .arg(url)
            .arg(format!("{position}"))
            .arg(format!("{volume}"))
            .arg(format!("{preamp_db}"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Inherit stderr so the child's error logs surface in the applet's
            // terminal / journal.
            .stderr(Stdio::inherit())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("failed to spawn {}: {e}", exe.display());
                return None;
            }
        };

        let stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;

        // Reader thread: forward each stdout line to the app. Ends on EOF
        // (child exited) or when the receiver is dropped.
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if events.send(line).is_err() {
                    break;
                }
            }
        });

        Some(Self { child, stdin })
    }

    /// Send one command line to the child (newline-terminated, flushed).
    /// Errors (e.g. the child already exited) are ignored.
    pub fn send(&mut self, line: &str) {
        let _ = writeln!(self.stdin, "{line}");
        let _ = self.stdin.flush();
    }

    /// Terminate the child (best-effort) and reap it so it never lingers.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for VideoWindowChild {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Locate the `mare-video-window` binary: prefer a sibling of the running
/// executable (covers dev `target/{debug,release}/` and installed `/usr/bin/`),
/// then fall back to bare name resolution via `PATH`.
fn locate_binary() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("mare-video-window");
        if sibling.exists() {
            return sibling;
        }
    }
    std::path::PathBuf::from("mare-video-window")
}
