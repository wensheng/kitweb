use base64::{prelude::BASE64_STANDARD, Engine};
use std::io::{self, Write};
use std::thread;
use std::time::Duration;

/// Robustly write all bytes to stdout, retrying on EAGAIN / WouldBlock errors.
pub fn write_all_robust<W: Write>(mut writer: W, mut buf: &[u8]) -> io::Result<()> {
    while !buf.is_empty() {
        match writer.write(buf) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "failed to write whole buffer")),
            Ok(n) => buf = &buf[n..],
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(ref e) if e.raw_os_error() == Some(35) => { // EAGAIN on mac
                thread::sleep(Duration::from_millis(1));
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Robustly flush stdout, retrying on EAGAIN / WouldBlock errors.
pub fn flush_robust<W: Write>(mut writer: W) -> io::Result<()> {
    loop {
        match writer.flush() {
            Ok(()) => return Ok(()),
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(ref e) if e.raw_os_error() == Some(35) => { // EAGAIN on mac
                thread::sleep(Duration::from_millis(1));
            }
            Err(e) => return Err(e),
        }
    }
}

#[allow(dead_code)]
pub fn move_up_robust(rows: u16) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    let mut buf = Vec::new();
    crossterm::queue!(buf, crossterm::cursor::MoveUp(rows), crossterm::cursor::MoveToColumn(0))?;
    write_all_robust(&mut stdout, &buf)?;
    flush_robust(&mut stdout)?;
    Ok(())
}

#[allow(dead_code)]
pub fn write_rgba_frame(
    pixels: &[u8],
    width_px: u32,
    height_px: u32,
    cols: u32,
    rows: u32,
    prevent_cursor_move: bool,
) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    write_rgba_frame_to(
        &mut stdout,
        pixels,
        width_px,
        height_px,
        cols,
        rows,
        prevent_cursor_move,
    )
}

pub fn write_rgba_frame_to<W: Write>(
    writer: &mut W,
    pixels: &[u8],
    width_px: u32,
    height_px: u32,
    cols: u32,
    rows: u32,
    prevent_cursor_move: bool,
) -> io::Result<()> {
    write_rgba_frame_impl(
        writer,
        pixels,
        width_px,
        height_px,
        Some((cols, rows)),
        prevent_cursor_move,
        false,
    )
}

/// Transmit a frame. When `compressed` is true, `payload` is the zlib (RFC 1950)
/// deflated RGBA buffer and `o=z` is added so the terminal inflates it; the
/// `width_px`/`height_px` remain the uncompressed pixel dimensions.
pub fn write_rgba_frame_native_to<W: Write>(
    writer: &mut W,
    payload: &[u8],
    width_px: u32,
    height_px: u32,
    prevent_cursor_move: bool,
    compressed: bool,
) -> io::Result<()> {
    write_rgba_frame_impl(
        writer,
        payload,
        width_px,
        height_px,
        None,
        prevent_cursor_move,
        compressed,
    )
}

fn write_rgba_frame_impl<W: Write>(
    writer: &mut W,
    payload: &[u8],
    width_px: u32,
    height_px: u32,
    cells: Option<(u32, u32)>,
    prevent_cursor_move: bool,
    compressed: bool,
) -> io::Result<()> {
    let base64_str = BASE64_STANDARD.encode(payload);
    let bytes = base64_str.as_bytes();
    let chunk_size = 4096;
    let mut offset = 0;

    while offset < bytes.len() {
        let is_last = offset + chunk_size >= bytes.len();
        let chunk = &bytes[offset..std::cmp::min(offset + chunk_size, bytes.len())];
        let m_param = if is_last { 0 } else { 1 };

        let mut packet = Vec::new();
        if offset == 0 {
            let c_policy = if prevent_cursor_move { ",C=1" } else { "" };
            let o_policy = if compressed { ",o=z" } else { "" };
            if let Some((cols, rows)) = cells {
                write!(
                    packet,
                    "\x1b_Ga=T,f=32,s={},v={},c={},r={}{}{},q=2,m={};",
                    width_px, height_px, cols, rows, c_policy, o_policy, m_param
                )?;
            } else {
                write!(
                    packet,
                    "\x1b_Ga=T,f=32,s={},v={}{}{},q=2,m={};",
                    width_px, height_px, c_policy, o_policy, m_param
                )?;
            }
        } else {
            write!(packet, "\x1b_Gq=2,m={};", m_param)?;
        }

        packet.write_all(chunk)?;
        packet.write_all(b"\x1b\\")?;

        write_all_robust(&mut *writer, &packet)?;
        offset += chunk_size;
    }

    flush_robust(&mut *writer)?;
    Ok(())
}
