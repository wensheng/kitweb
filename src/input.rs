use crate::browser::BrowserSession;
use crossterm::{
    cursor::MoveTo,
    event::{
        self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
    },
    execute,
    terminal::{self, enable_raw_mode, Clear, ClearType},
};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub enum ControlCmd {
    Navigate(String),
    #[allow(dead_code)]
    Reload,
    ToggleMute,
    VolumeBy(i32),
    Quit,
    #[allow(dead_code)]
    Resize(u16, u16),
}

const STATUS_HELP: &str =
    "o/l=url i=input r=reload m=mute +/-=vol click=mouse arrows/tab/home/end=browser q=quit";
const FALLBACK_CELL_WIDTH_PX: f64 = 10.0;
const FALLBACK_CELL_HEIGHT_PX: f64 = 20.0;

pub fn run_input(
    control_tx: std::sync::mpsc::SyncSender<ControlCmd>,
    running: Arc<AtomicBool>,
    browser: Arc<Mutex<BrowserSession>>,
    status_msg: Arc<Mutex<String>>,
    stdout: Arc<Mutex<io::Stdout>>,
    prompt_active: Arc<AtomicBool>,
) {
    {
        let mut s = status_msg.lock().unwrap();
        *s = STATUS_HELP.to_string();
    }

    while running.load(Ordering::SeqCst) {
        if !event::poll(Duration::from_millis(50)).unwrap_or(false) {
            continue;
        }

        let ev = match event::read() {
            Ok(e) => e,
            Err(_) => break,
        };

        match ev {
            Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers: KeyModifiers::CONTROL,
                kind: KeyEventKind::Press,
                ..
            })
            | Event::Key(KeyEvent {
                code: KeyCode::Char('q'),
                kind: KeyEventKind::Press,
                ..
            }) => {
                running.store(false, Ordering::SeqCst);
                let _ = control_tx.try_send(ControlCmd::Quit);
                break;
            }

            Event::Key(KeyEvent {
                code: KeyCode::Char('r'),
                kind: KeyEventKind::Press,
                ..
            }) => {
                let b = browser.lock().unwrap();
                b.reload();
                set_status(&status_msg, "Reloading...");
            }

            Event::Key(KeyEvent {
                code: KeyCode::Char('o') | KeyCode::Char('l'),
                kind: KeyEventKind::Press,
                ..
            }) => {
                if let Some(url) = prompt_url(&status_msg, &stdout, &prompt_active) {
                    let _ = control_tx.try_send(ControlCmd::Navigate(url));
                }
                set_status(&status_msg, STATUS_HELP);
            }

            Event::Key(KeyEvent {
                code: KeyCode::Char('i'),
                kind: KeyEventKind::Press,
                ..
            }) => {
                if let Some(text) = prompt_input(&status_msg, &stdout, &prompt_active) {
                    let b = browser.lock().unwrap();
                    if let Err(e) = b.type_text_and_enter(&text) {
                        set_status(&status_msg, &format!("Input error: {}", e));
                    }
                }
            }

            Event::Key(KeyEvent {
                code: KeyCode::Char('m'),
                kind: KeyEventKind::Press,
                ..
            }) => {
                let _ = control_tx.try_send(ControlCmd::ToggleMute);
            }

            Event::Key(KeyEvent {
                code: KeyCode::Char('+') | KeyCode::Char('='),
                kind: KeyEventKind::Press,
                ..
            }) => {
                let _ = control_tx.try_send(ControlCmd::VolumeBy(5));
            }

            Event::Key(KeyEvent {
                code: KeyCode::Char('-'),
                kind: KeyEventKind::Press,
                ..
            }) => {
                let _ = control_tx.try_send(ControlCmd::VolumeBy(-5));
            }

            // Browser key forwarding
            Event::Key(KeyEvent {
                code: KeyCode::Up,
                kind: KeyEventKind::Press,
                ..
            }) => {
                let b = browser.lock().unwrap();
                b.send_key("Up");
            }
            Event::Key(KeyEvent {
                code: KeyCode::Down,
                kind: KeyEventKind::Press,
                ..
            }) => {
                let b = browser.lock().unwrap();
                b.send_key("Down");
            }
            Event::Key(KeyEvent {
                code: KeyCode::Left,
                kind: KeyEventKind::Press,
                ..
            }) => {
                let b = browser.lock().unwrap();
                b.send_key("Left");
            }
            Event::Key(KeyEvent {
                code: KeyCode::Right,
                kind: KeyEventKind::Press,
                ..
            }) => {
                let b = browser.lock().unwrap();
                b.send_key("Right");
            }
            Event::Key(KeyEvent {
                code: KeyCode::PageUp,
                kind: KeyEventKind::Press,
                ..
            }) => {
                let b = browser.lock().unwrap();
                b.send_key("Page_Up");
            }
            Event::Key(KeyEvent {
                code: KeyCode::PageDown,
                kind: KeyEventKind::Press,
                ..
            }) => {
                let b = browser.lock().unwrap();
                b.send_key("Page_Down");
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char(' '),
                kind: KeyEventKind::Press,
                ..
            }) => {
                let b = browser.lock().unwrap();
                b.send_key("space");
            }
            Event::Key(KeyEvent {
                code: KeyCode::Home,
                kind: KeyEventKind::Press,
                ..
            }) => {
                let b = browser.lock().unwrap();
                b.send_key("Home");
            }
            Event::Key(KeyEvent {
                code: KeyCode::End,
                kind: KeyEventKind::Press,
                ..
            }) => {
                let b = browser.lock().unwrap();
                b.send_key("End");
            }
            Event::Key(KeyEvent {
                code: KeyCode::Tab,
                kind: KeyEventKind::Press,
                ..
            }) => {
                let b = browser.lock().unwrap();
                b.send_key("Tab");
            }
            Event::Key(KeyEvent {
                code: KeyCode::BackTab,
                kind: KeyEventKind::Press,
                ..
            }) => {
                let b = browser.lock().unwrap();
                b.send_key("shift+Tab");
            }
            Event::Key(KeyEvent {
                code: KeyCode::Esc,
                kind: KeyEventKind::Press,
                ..
            }) => {
                let b = browser.lock().unwrap();
                b.send_key("Escape");
            }

            // Mouse click / scroll forwarding
            Event::Mouse(m) => match m.kind {
                MouseEventKind::Down(button) => {
                    let button = mouse_button_number(button);
                    let b = browser.lock().unwrap();
                    if let Some((x, y)) = click_position(&b, m.column, m.row) {
                        if let Err(e) = b.click_at(button, x, y) {
                            set_status(&status_msg, &format!("Click error: {}", e));
                        }
                    }
                }
                MouseEventKind::ScrollUp => {
                    let b = browser.lock().unwrap();
                    b.scroll(1);
                }
                MouseEventKind::ScrollDown => {
                    let b = browser.lock().unwrap();
                    b.scroll(-1);
                }
                MouseEventKind::ScrollLeft => {
                    let b = browser.lock().unwrap();
                    b.scroll_horizontal(1);
                }
                MouseEventKind::ScrollRight => {
                    let b = browser.lock().unwrap();
                    b.scroll_horizontal(-1);
                }
                _ => {}
            },

            Event::Resize(cols, rows) => {
                let _ = control_tx.try_send(ControlCmd::Resize(cols, rows));
            }

            _ => {}
        }
    }
}

fn set_status(status_msg: &Arc<Mutex<String>>, msg: &str) {
    let mut s = status_msg.lock().unwrap();
    *s = msg.to_string();
}

fn prompt_url(
    status_msg: &Arc<Mutex<String>>,
    stdout: &Arc<Mutex<io::Stdout>>,
    prompt_active: &Arc<AtomicBool>,
) -> Option<String> {
    prompt_line(status_msg, stdout, prompt_active, "Open URL:").map(|input| {
        if input.contains("://") {
            input
        } else {
            format!("https://{}", input)
        }
    })
}

fn prompt_input(
    status_msg: &Arc<Mutex<String>>,
    stdout: &Arc<Mutex<io::Stdout>>,
    prompt_active: &Arc<AtomicBool>,
) -> Option<String> {
    prompt_line(status_msg, stdout, prompt_active, "Input:")
}

fn prompt_line(
    status_msg: &Arc<Mutex<String>>,
    stdout: &Arc<Mutex<io::Stdout>>,
    prompt_active: &Arc<AtomicBool>,
    label: &str,
) -> Option<String> {
    let _ = enable_raw_mode();
    let (cols, rows) = terminal::size().unwrap_or((80, 24));

    // Tell the render thread to stop drawing the bottom row so it won't clobber
    // our prompt. It keeps streaming the image, so video keeps playing while we
    // type. We only grab the stdout lock briefly per redraw (not for the whole
    // session), leaving the render thread free to draw frames between keystrokes.
    prompt_active.store(true, Ordering::SeqCst);

    let mut input = String::new();
    set_prompt_status(status_msg, label, &input);
    redraw_prompt(stdout, rows, cols, label, &input);

    let mut submitted = false;
    loop {
        if !event::poll(Duration::from_millis(200)).unwrap_or(false) {
            continue;
        }
        if let Ok(Event::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            ..
        })) = event::read()
        {
            match code {
                KeyCode::Enter => {
                    submitted = true;
                    break;
                }
                KeyCode::Esc => {
                    input.clear();
                    break;
                }
                KeyCode::Backspace => {
                    input.pop();
                    set_prompt_status(status_msg, label, &input);
                    redraw_prompt(stdout, rows, cols, label, &input);
                }
                KeyCode::Char(c) => {
                    if modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
                        continue;
                    }
                    input.push(c);
                    set_prompt_status(status_msg, label, &input);
                    redraw_prompt(stdout, rows, cols, label, &input);
                }
                _ => {}
            }
        }
    }

    prompt_active.store(false, Ordering::SeqCst);
    let _ = enable_raw_mode();

    set_status(status_msg, STATUS_HELP);
    if submitted && !input.is_empty() {
        Some(input)
    } else {
        None
    }
}

fn set_prompt_status(status_msg: &Arc<Mutex<String>>, label: &str, input: &str) {
    let escaped_input: String = input.chars().flat_map(|c| c.escape_default()).collect();
    set_status(status_msg, &format!("{} {}", label, escaped_input));
}

fn redraw_prompt(
    stdout: &Arc<Mutex<io::Stdout>>,
    rows: u16,
    cols: u16,
    label: &str,
    input: &str,
) {
    let mut guard = stdout.lock().unwrap();
    let out = &mut *guard;
    let _ = execute!(out, MoveTo(0, rows.saturating_sub(1)));
    let _ = execute!(out, Clear(ClearType::CurrentLine));
    let _ = write!(out, "\x1b[7m {} \x1b[0m ", label);

    let label_width = label.chars().count() + 3;
    let available = (cols as usize).saturating_sub(label_width + 1);
    let visible_input: String = input.chars().take(available).collect();
    let _ = write!(out, "{}", visible_input);
    let _ = out.flush();
}

fn mouse_button_number(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Middle => 2,
        MouseButton::Right => 3,
    }
}

fn click_position(browser: &BrowserSession, column: u16, row: u16) -> Option<(u32, u32)> {
    let window = terminal::window_size().ok();
    let rows = window
        .as_ref()
        .map(|size| size.rows)
        .or_else(|| terminal::size().ok().map(|(_, rows)| rows))?;
    let render_rows = rows.checked_sub(1)?;

    if render_rows == 0 || row >= render_rows {
        return None;
    }

    if browser.width == 0 || browser.height == 0 {
        return None;
    }

    let (cell_width_px, cell_height_px) = terminal_cell_size_px(window.as_ref());
    let x = ((column as f64 + 0.5) * cell_width_px).floor() as u32;
    let y = ((row as f64 + 0.5) * cell_height_px).floor() as u32;

    if x >= browser.width || y >= browser.height {
        return None;
    }

    Some((x, y))
}

fn terminal_cell_size_px(window: Option<&terminal::WindowSize>) -> (f64, f64) {
    let Some(window) = window else {
        return (FALLBACK_CELL_WIDTH_PX, FALLBACK_CELL_HEIGHT_PX);
    };

    let cell_width = if window.columns > 0 && window.width > 0 {
        window.width as f64 / window.columns as f64
    } else {
        FALLBACK_CELL_WIDTH_PX
    };
    let cell_height = if window.rows > 0 && window.height > 0 {
        window.height as f64 / window.rows as f64
    } else {
        FALLBACK_CELL_HEIGHT_PX
    };

    (cell_width, cell_height)
}
