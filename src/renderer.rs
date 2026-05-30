use crate::capture::CaptureMsg;
use crate::kitty;
use crossterm::{
    cursor::MoveTo,
    execute,
    terminal::{self, Clear, ClearType},
};
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// zlib-deflate an RGBA buffer for transmission with the Kitty `o=z` option.
/// Level 1 (fastest): screen content is dominated by runs of identical pixels,
/// so the marginal ratio from higher levels isn't worth the extra CPU per frame.
fn compress_frame(rgba: &[u8]) -> Option<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(1));
    encoder.write_all(rgba).ok()?;
    encoder.finish().ok()
}

/// A `Write` wrapper that tallies every byte handed to the underlying writer.
/// Used (behind `KITWEB_STATS=1`) to measure the real on-the-wire byte rate of
/// the renderer's stdout output — image payload, status bar, and cursor moves.
struct StatWriter<'a, W: Write> {
    inner: W,
    counter: &'a mut u64,
}

impl<W: Write> Write for StatWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        *self.counter += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

pub fn run_renderer(
    rx: Receiver<CaptureMsg>,
    running: Arc<AtomicBool>,
    fps: u32,
    status_msg: Arc<Mutex<String>>,
    stdout: Arc<Mutex<io::Stdout>>,
    prompt_active: Arc<AtomicBool>,
) {
    let frame_duration = Duration::from_micros(1_000_000 / fps.max(1) as u64);
    let mut last_frame = Instant::now();
    let mut prev_rgba: Option<Vec<u8>> = None;
    let mut last_status = String::new();

    // Wire-byte instrumentation: count bytes written to stdout and, once per
    // second, surface the rate on the status bar. Off unless KITWEB_STATS is set.
    let stats_enabled = std::env::var_os("KITWEB_STATS").is_some();
    let mut byte_window: u64 = 0;
    let mut window_start = Instant::now();
    let mut rate_str = String::new();

    while running.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(CaptureMsg::Frame { rgba, width, height }) => {
                let now = Instant::now();
                let elapsed = now.duration_since(last_frame);
                if elapsed < frame_duration {
                    std::thread::sleep(frame_duration - elapsed);
                }
                last_frame = Instant::now();

                let (cols, rows) = terminal::size().unwrap_or((80, 24));

                let status = {
                    let lock = status_msg.lock().unwrap();
                    lock.clone()
                };

                // Refresh the byte-rate label about once per second. Forces a
                // status-bar redraw even on an idle (unchanged) page so the rate
                // keeps ticking down to 0 when nothing is moving.
                let mut rate_changed = false;
                if stats_enabled {
                    let el = window_start.elapsed();
                    if el >= Duration::from_secs(1) {
                        let mbps = byte_window as f64 / el.as_secs_f64() / 1_000_000.0;
                        let new_rate = format!("[{:.2} MB/s] ", mbps);
                        if new_rate != rate_str {
                            rate_str = new_rate;
                            rate_changed = true;
                        }
                        byte_window = 0;
                        window_start = Instant::now();
                    }
                }

                // Only retransmit the image when the pixels actually changed.
                // Browsing is mostly static, so this drops idle bandwidth to ~0.
                let frame_changed = prev_rgba.as_ref().map_or(true, |p| p != &rgba);
                let payload = if frame_changed {
                    compress_frame(&rgba)
                } else {
                    None
                };
                let status_changed = status != last_status;

                // While an input prompt is up, the input thread owns the bottom
                // row. Keep streaming the image so video doesn't freeze, but skip
                // the status bar so we don't clobber the prompt.
                let draw_status = !prompt_active.load(Ordering::SeqCst);

                if payload.is_some() || (draw_status && (status_changed || rate_changed)) {
                    let mut guard = stdout.lock().unwrap();
                    let mut w = StatWriter {
                        inner: &mut *guard,
                        counter: &mut byte_window,
                    };
                    if let Some(data) = &payload {
                        let _ = execute!(&mut w, MoveTo(0, 0));
                        let _ = kitty::write_rgba_frame_native_to(
                            &mut w,
                            data,
                            width,
                            height,
                            false,
                            true,
                        );
                    }
                    if draw_status {
                        let bar = format!(
                            " {}{:<width$}",
                            rate_str,
                            status,
                            width = cols.saturating_sub(1) as usize
                        );
                        let _ = execute!(&mut w, MoveTo(0, rows.saturating_sub(1)));
                        let _ = write!(
                            &mut w,
                            "\x1b[7m{}\x1b[0m",
                            &bar[..bar.len().min(cols as usize)]
                        );
                    }
                    let _ = w.flush();
                }

                if payload.is_some() {
                    prev_rgba = Some(rgba);
                }
                last_status = status;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        }
    }

    // Clear screen on exit
    let mut guard = stdout.lock().unwrap();
    let _ = execute!(&mut *guard, Clear(ClearType::All), MoveTo(0, 0));
    let _ = guard.flush();
}
