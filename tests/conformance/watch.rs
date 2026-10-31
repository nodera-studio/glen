//! W5 `glen watch` (AC-501…AC-526), derived from `.claude/plans/v010-wave-5-watch.md` (acceptance
//! criteria, Design spec, orchestrator decisions and the TUI reference-study amendments),
//! `docs/design.md` §A3, §A9–§A11, §A15 and `docs/json.md`, without reading the implementation
//! (`src/tui/`, `src/commands/watch.rs`) or the Implementer's `tests/watch.rs` and snapshots.
//!
//! Every case drives the real `glen` binary. Full-screen cases run it on a pseudo-terminal
//! (rustix `openpt`), always as a new session leader (`setsid`) so it never sees the test
//! runner's terminal; `Ctty::Yes` also makes the pty its controlling terminal. A small VT
//! emulator (`Vt`) replays the output into a cell grid with foreground colour, bold and dim, so
//! layout, colour and restore checks read what a terminal would show. Fixtures come from
//! `tests/fixtures/` through the W1 `OmpBox` (inside the W2 `RpcBox`, inside the W3 `LiveBox`);
//! the `fake_omp` and `fake_ext` examples are the only peers. Nothing reads a live `~/.omp`, and
//! the process environment is never mutated.
//!
//! Timing ACs are asserted at their bound plus `SLACK` for pty, debug-build and parallel-test
//! scheduling (the plan's Step 10 allows the same); the terminal restore keeps a tighter
//! allowance (`RESTORE_SLACK`).
//!
//! Readings this suite takes where the plan leaves room:
//! - R1 The terminal gate (AC-501) runs before session resolution: `watch nosuch1 | cat` is `usage`.
//! - R2 AC-504 "render_items" is observed as `glen show --human`'s body after its header block
//!   (title, meta line, blank line), at `COLUMNS=W`, W = pane width − 2 (list 34 or 24 + `│`).
//!   Styled-run equality compares non-blank cells only (style on spaces is invisible).
//! - R3 AC-521 "a help overlay that lists every binding available in this build" is read
//!   literally: one overlay lists the whole keymap, wherever it is opened.
//! - R4 Amendment 6 (ASCII hint register) applies to the status-bar hint row; ⇥ ⏎ ⌥ and the
//!   ⌃/⇧ modifier glyphs are rejected there, arrows are not asserted. Empty-state texts that carry
//!   ⇥/⏎ in the Design table (placeholders, "↑↓ choose · ⏎ write") accept `tab`/`enter` too.
//! - R5 Amendment 5 compares bar segments by identity; a `…` truncation is the same segment.
//! - R6 In narrow mode the visible pane follows focus; focus is read from the glyphs: the
//!   terminal cursor shows only while the prompt has focus, else `▍` marks the focused tab.
//! - R7 AC-505's not_found text is `tail -f`'s prose before the file path.
//! - R8 A second SIGINT while quitting (orchestrator decision 2) exits 130 within 850 ms.
//! - R9 AC-526 is checked as its own source rule, ignoring `//` comments and code after the
//!   first `#[cfg(test)]`; `eprint!(` is rejected with the listed print macros.
//! - R10 Repeat and Release key events are sent as kitty CSI-u sequences (`ESC[13;1:2u`), which
//!   crossterm parses without the keyboard enhancement flags.
//! - R11 B (`f13a5b6`) continues in C, so selecting it lands on C (AC-505); draft and selection
//!   walks use G.
//! - R12 Typing after a refusal may clear the notice; Esc dismissal is checked before typing.
//!
//! Not covered offline: AC-509's probe policy (not observable from outside; the extension-row
//! state is checked), AC-507 (the unmodified W1–W3 modules and `tests/site_demo.rs` cover it),
//! AC-511's "same frame", AC-513's 60 s expiry, the transient "list loading" and "opening" states,
//! AC-510(c), AC-516 with W3b (the build constant is off), and plan Step 12 (real omp 18.14.0).

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use rustix::process::Signal;
use serde_json::{Value, json};

use crate::live_support::*;
use crate::reader::*;
use crate::rpc_support::*;
use crate::support::*;

/// Allowance on top of an AC timing bound (pty, debug build, parallel tests).
const SLACK: Duration = Duration::from_millis(1000);
/// Allowance on top of the 250 ms restore bound (AC-522, AC-523).
const RESTORE_SLACK: Duration = Duration::from_millis(350);
/// Bound for expectations without an AC timing bound (first frame, a key's effect).
const UI: Duration = Duration::from_secs(10);

const SPINNER: &str = "⠁⠉⠙⠚⠒⠂⠲⠴⠤⠄⠠⠦⠖⠐⠓⠋⠈";
const ALT_ON: &[u8] = b"\x1b[?1049h";
const ALT_OFF: &[u8] = b"\x1b[?1049l";
const PASTE_ON: &[u8] = b"\x1b[?2004h";
const PASTE_OFF: &[u8] = b"\x1b[?2004l";
const CURSOR_ON: &[u8] = b"\x1b[?25h";

const NOT_IN_BUILD: &str = "session controls are not in this build";
const TERMINAL_MSG: &str = "glen watch needs an interactive terminal";

// Keys (raw bytes a terminal sends).
const TAB: &[u8] = b"\t";
const BACKTAB: &[u8] = b"\x1b[Z";
const ENTER: &[u8] = b"\r";
const ESC: &[u8] = b"\x1b";
const UP: &[u8] = b"\x1b[A";
const DOWN: &[u8] = b"\x1b[B";
const RIGHT: &[u8] = b"\x1b[C";
const LEFT: &[u8] = b"\x1b[D";
const HOME: &[u8] = b"\x1b[H";
const END: &[u8] = b"\x1b[F";
const PGUP: &[u8] = b"\x1b[5~";
const PGDN: &[u8] = b"\x1b[6~";
const DELETE: &[u8] = b"\x1b[3~";
const BACKSPACE: &[u8] = b"\x7f";
const F1: &[u8] = b"\x1bOP";
const ALT_ENTER: &[u8] = b"\x1b\r";
const CTRL_A: &[u8] = b"\x01";
const CTRL_C: &[u8] = b"\x03";
const CTRL_D: &[u8] = b"\x04";
const CTRL_E: &[u8] = b"\x05";
const CTRL_J: &[u8] = b"\n";
const CTRL_K: &[u8] = b"\x0b";
const CTRL_T: &[u8] = b"\x14";
const CTRL_U: &[u8] = b"\x15";
const CTRL_W: &[u8] = b"\x17";
const CTRL_X: &[u8] = b"\x18";

// =============================================================================================
// The VT emulator.
// =============================================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fg {
    Default,
    Idx(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pen {
    fg: Fg,
    bold: bool,
    dim: bool,
    /// italic, underline, blink, reverse, hidden, strike.
    other: bool,
    /// A non-default background is in effect.
    bg: bool,
}

impl Default for Pen {
    fn default() -> Self {
        Pen {
            fg: Fg::Default,
            bold: false,
            dim: false,
            other: false,
            bg: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    /// The grapheme; empty for the right half of a wide character.
    pub ch: String,
    pub fg: Fg,
    pub bold: bool,
    pub dim: bool,
    pub other: bool,
    pub bg: bool,
}

impl Cell {
    fn blank() -> Cell {
        Cell::with(" ", Pen::default())
    }

    fn with(ch: &str, p: Pen) -> Cell {
        Cell {
            ch: ch.to_owned(),
            fg: p.fg,
            bold: p.bold,
            dim: p.dim,
            other: p.other,
            bg: p.bg,
        }
    }

    pub fn styled(&self) -> bool {
        self.fg != Fg::Default || self.bold || self.dim || self.other || self.bg
    }
}

/// A minimal xterm-like screen: CUP/ED/EL/SGR/DEC private modes, UTF-8 and wide characters.
#[derive(Clone)]
pub struct Vt {
    pub cols: usize,
    pub rows: usize,
    pub grid: Vec<Vec<Cell>>,
    pub cx: usize,
    pub cy: usize,
    pen: Pen,
    pub cursor_visible: bool,
    pub alt: bool,
    /// Printable characters written past the right or bottom edge.
    pub oob: Vec<String>,
    /// Ignore out-of-bounds writes until the next full clear (after a resize).
    pub resizing: bool,
    /// Full clears (`ESC[2J`) seen.
    pub clears: usize,
    /// SGR sequences that set a background colour.
    pub bg_sgr: Vec<String>,
    buf: Vec<u8>,
}

enum Esc {
    Csi { params: String, fin: u8 },
    Skip,
}

fn parse_esc(b: &[u8]) -> Option<(usize, Esc)> {
    if b.len() < 2 {
        return None;
    }
    match b[1] {
        b'[' => {
            let mut i = 2;
            while i < b.len() && (0x30..=0x3f).contains(&b[i]) {
                i += 1;
            }
            let pend = i;
            while i < b.len() && (0x20..=0x2f).contains(&b[i]) {
                i += 1;
            }
            if i >= b.len() {
                return None;
            }
            Some((
                i + 1,
                Esc::Csi {
                    params: String::from_utf8_lossy(&b[2..pend]).into_owned(),
                    fin: b[i],
                },
            ))
        }
        b']' | b'P' | b'X' | b'^' | b'_' => {
            let mut i = 2;
            while i < b.len() {
                if b[i] == 0x07 {
                    return Some((i + 1, Esc::Skip));
                }
                if b[i] == 0x1b {
                    if i + 1 >= b.len() {
                        return None;
                    }
                    if b[i + 1] == b'\\' {
                        return Some((i + 2, Esc::Skip));
                    }
                }
                i += 1;
            }
            None
        }
        b'(' | b')' | b'*' | b'+' | b'#' | b'%' => {
            if b.len() < 3 {
                None
            } else {
                Some((3, Esc::Skip))
            }
        }
        _ => Some((2, Esc::Skip)),
    }
}

impl Vt {
    pub fn new(cols: usize, rows: usize) -> Vt {
        Vt {
            cols,
            rows,
            grid: vec![vec![Cell::blank(); cols]; rows],
            cx: 0,
            cy: 0,
            pen: Pen::default(),
            cursor_visible: true,
            alt: false,
            oob: Vec::new(),
            resizing: false,
            clears: 0,
            bg_sgr: Vec::new(),
            buf: Vec::new(),
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.cols = cols;
        self.rows = rows;
        self.grid = vec![vec![Cell::blank(); cols]; rows];
        self.cx = 0;
        self.cy = 0;
        self.resizing = true;
    }

    fn clear_all(&mut self) {
        for r in &mut self.grid {
            for c in r.iter_mut() {
                *c = Cell::blank();
            }
        }
    }

    pub fn feed(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
        let b = std::mem::take(&mut self.buf);
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            match c {
                0x1b => match parse_esc(&b[i..]) {
                    Some((n, e)) => {
                        if let Esc::Csi { params, fin } = e {
                            self.csi(&params, fin);
                        }
                        i += n;
                    }
                    None => break,
                },
                b'\r' => {
                    self.cx = 0;
                    i += 1;
                }
                b'\n' | 0x0b | 0x0c => {
                    if self.cy + 1 < self.rows {
                        self.cy += 1;
                    }
                    i += 1;
                }
                0x08 => {
                    self.cx = self.cx.saturating_sub(1);
                    i += 1;
                }
                0x00..=0x1f | 0x7f => i += 1,
                _ => {
                    let len = match c {
                        0xc0..=0xdf => 2,
                        0xe0..=0xef => 3,
                        0xf0..=0xf7 => 4,
                        0x80..=0xbf | 0xf8..=0xff => {
                            i += 1;
                            continue;
                        }
                        _ => 1,
                    };
                    if i + len > b.len() {
                        break;
                    }
                    let s = String::from_utf8_lossy(&b[i..i + len]).into_owned();
                    i += len;
                    for ch in s.chars() {
                        self.put(ch);
                    }
                }
            }
        }
        self.buf = b[i..].to_vec();
    }

    fn put(&mut self, ch: char) {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w == 0 {
            if self.cx > 0 && self.cy < self.rows && self.cx <= self.cols {
                self.grid[self.cy][self.cx - 1].ch.push(ch);
            }
            return;
        }
        if self.cy >= self.rows || self.cx + w > self.cols {
            if !self.resizing {
                self.oob
                    .push(format!("{ch:?} at row {} col {}", self.cy, self.cx));
            }
            self.cx += w;
            return;
        }
        self.grid[self.cy][self.cx] = Cell::with(&ch.to_string(), self.pen);
        if w == 2 {
            self.grid[self.cy][self.cx + 1] = Cell::with("", self.pen);
        }
        self.cx += w;
    }

    fn nums(params: &str) -> Vec<usize> {
        params
            .split(';')
            .map(|p| p.parse::<usize>().unwrap_or(0))
            .collect()
    }

    fn csi(&mut self, params: &str, fin: u8) {
        if let Some(private) = params.strip_prefix('?') {
            let on = fin == b'h';
            if fin == b'h' || fin == b'l' {
                for m in private.split(';') {
                    match m {
                        "1049" | "47" | "1047" => {
                            self.alt = on;
                            self.clear_all();
                        }
                        "25" => self.cursor_visible = on,
                        _ => {}
                    }
                }
            }
            return;
        }
        let n = Self::nums(params);
        let n1 = |i: usize| n.get(i).copied().filter(|v| *v > 0).unwrap_or(1);
        match fin {
            b'H' | b'f' => {
                self.cy = (n1(0) - 1).min(self.rows.saturating_sub(1));
                self.cx = n1(1) - 1;
            }
            b'A' => self.cy = self.cy.saturating_sub(n1(0)),
            b'B' => self.cy = (self.cy + n1(0)).min(self.rows.saturating_sub(1)),
            b'C' => self.cx += n1(0),
            b'D' => self.cx = self.cx.saturating_sub(n1(0)),
            b'E' => {
                self.cy = (self.cy + n1(0)).min(self.rows.saturating_sub(1));
                self.cx = 0;
            }
            b'F' => {
                self.cy = self.cy.saturating_sub(n1(0));
                self.cx = 0;
            }
            b'G' => self.cx = n1(0) - 1,
            b'd' => self.cy = (n1(0) - 1).min(self.rows.saturating_sub(1)),
            b'J' => match n.first().copied().unwrap_or(0) {
                2 | 3 => {
                    self.clear_all();
                    self.clears += 1;
                    self.resizing = false;
                }
                0 => {
                    for y in self.cy..self.rows {
                        let from = if y == self.cy { self.cx } else { 0 };
                        for x in from..self.cols {
                            self.grid[y][x] = Cell::blank();
                        }
                    }
                }
                1 => {
                    for y in 0..=self.cy.min(self.rows.saturating_sub(1)) {
                        let to = if y == self.cy { self.cx + 1 } else { self.cols };
                        for x in 0..to.min(self.cols) {
                            self.grid[y][x] = Cell::blank();
                        }
                    }
                }
                _ => {}
            },
            b'K' if self.cy < self.rows => {
                let (from, to) = match n.first().copied().unwrap_or(0) {
                    0 => (self.cx, self.cols),
                    1 => (0, self.cx + 1),
                    _ => (0, self.cols),
                };
                for x in from.min(self.cols)..to.min(self.cols) {
                    self.grid[self.cy][x] = Cell::blank();
                }
            }
            b'X' if self.cy < self.rows => {
                for x in self.cx..(self.cx + n1(0)).min(self.cols) {
                    self.grid[self.cy][x] = Cell::blank();
                }
            }
            b'm' => self.sgr(params),
            _ => {}
        }
    }

    fn sgr(&mut self, params: &str) {
        let ps: Vec<u32> = if params.is_empty() {
            vec![0]
        } else {
            params
                .split([';', ':'])
                .map(|p| p.parse().unwrap_or(0))
                .collect()
        };
        let mut i = 0;
        while i < ps.len() {
            let p = ps[i];
            match p {
                0 => self.pen = Pen::default(),
                1 => self.pen.bold = true,
                2 => self.pen.dim = true,
                3..=9 | 21 | 53 => self.pen.other = true,
                22 => {
                    self.pen.bold = false;
                    self.pen.dim = false;
                }
                23..=29 | 55 => self.pen.other = false,
                30..=37 => self.pen.fg = Fg::Idx((p - 30) as u8),
                90..=97 => self.pen.fg = Fg::Idx((p - 90 + 8) as u8),
                39 => self.pen.fg = Fg::Default,
                38 | 48 | 58 => {
                    let (val, used) = match ps.get(i + 1) {
                        Some(5) => (Some(Fg::Idx(ps.get(i + 2).copied().unwrap_or(0) as u8)), 2),
                        Some(2) => {
                            let c = |k: usize| ps.get(i + k).copied().unwrap_or(0) as u8;
                            (Some(Fg::Rgb(c(2), c(3), c(4))), 4)
                        }
                        _ => (None, 0),
                    };
                    if p == 38 {
                        if let Some(v) = val {
                            self.pen.fg = v;
                        }
                    } else if p == 48 {
                        self.pen.bg = true;
                        self.bg_sgr.push(params.to_owned());
                    }
                    i += used;
                }
                40..=47 | 100..=107 => {
                    self.pen.bg = true;
                    self.bg_sgr.push(params.to_owned());
                }
                49 => self.pen.bg = false,
                _ => {}
            }
            i += 1;
        }
    }

    // -- reading the grid ---------------------------------------------------------------------

    /// The non-continuation cells of row `y` with their columns.
    pub fn cells(&self, y: usize) -> Vec<(usize, &Cell)> {
        self.grid[y]
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.ch.is_empty())
            .collect()
    }

    /// The text of columns `[x0, x1)` of row `y`.
    pub fn text(&self, y: usize, x0: usize, x1: usize) -> String {
        if y >= self.rows {
            return String::new();
        }
        self.cells(y)
            .into_iter()
            .filter(|(x, _)| *x >= x0 && *x < x1)
            .map(|(_, c)| c.ch.as_str())
            .collect()
    }

    pub fn row(&self, y: usize) -> String {
        self.text(y, 0, self.cols)
    }

    pub fn lines(&self) -> Vec<String> {
        (0..self.rows)
            .map(|y| self.row(y).trim_end().to_owned())
            .collect()
    }

    pub fn dump(&self) -> String {
        let mut s = String::new();
        for (y, l) in self.lines().iter().enumerate() {
            s.push_str(&format!("{y:>3}|{l}\n"));
        }
        s
    }

    pub fn contains(&self, needle: &str) -> bool {
        self.lines().iter().any(|l| l.contains(needle))
    }

    /// (row, column) of every occurrence of `needle`.
    pub fn find_all(&self, needle: &str) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for y in 0..self.rows {
            let cells = self.cells(y);
            let text: String = cells.iter().map(|(_, c)| c.ch.as_str()).collect();
            // Map byte offsets of `text` to columns.
            let mut cols = Vec::new();
            for (x, c) in &cells {
                for _ in 0..c.ch.len() {
                    cols.push(*x);
                }
            }
            let mut from = 0;
            while let Some(p) = text[from..].find(needle) {
                out.push((y, cols[from + p]));
                from += p + needle.len().max(1);
            }
        }
        out
    }

    pub fn find(&self, needle: &str) -> Option<(usize, usize)> {
        self.find_all(needle).into_iter().next()
    }

    /// The cells (with columns) covered by the first occurrence of `needle` in row `y`.
    pub fn cells_of(&self, y: usize, x: usize, needle: &str) -> Vec<&Cell> {
        let w = unicode_width::UnicodeWidthStr::width(needle);
        self.grid[y][x..(x + w).min(self.cols)]
            .iter()
            .filter(|c| !c.ch.is_empty())
            .collect()
    }

    /// Every cell of the grid.
    pub fn all_cells(&self) -> impl Iterator<Item = &Cell> {
        self.grid.iter().flatten().filter(|c| !c.ch.is_empty())
    }
}

/// Parse a line-oriented ANSI document (e.g. `glen show --color always`) into styled lines.
fn styled_lines(ansi: &str, width: usize) -> Vec<Vec<Cell>> {
    let n = ansi.split('\n').count();
    let mut vt = Vt::new(width + 8, n + 1);
    for (i, l) in ansi.split('\n').enumerate() {
        vt.cx = 0;
        vt.cy = i;
        vt.feed(l.as_bytes());
    }
    (0..n)
        .map(|y| {
            vt.grid[y]
                .iter()
                .filter(|c| !c.ch.is_empty())
                .cloned()
                .collect()
        })
        .collect()
}

// =============================================================================================
// Palette (docs/design.md §A10) and depth/theme expectations.
// =============================================================================================

#[derive(Clone, Copy, Debug, PartialEq)]
enum Depth {
    True,
    X256,
    X16,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Theme {
    Dark,
    Light,
}

/// (token, light hex, dark hex, xterm light, xterm dark, ANSI-16 index or None for default+dim).
type PaletteRow = (&'static str, u32, u32, u8, u8, Option<u8>);
const PALETTE: [PaletteRow; 8] = [
    ("muted", 0x505050, 0xB4B5BD, 239, 250, None),
    ("border", 0xDADADA, 0x45474D, 253, 238, Some(8)),
    ("accent", 0x5E6AD2, 0x7170FF, 62, 63, Some(4)),
    ("accent_2", 0x12A594, 0x12A594, 36, 36, Some(6)),
    ("success", 0x007F3D, 0x00CA50, 29, 41, Some(2)),
    ("warning", 0xA66300, 0xFF9300, 130, 208, Some(3)),
    ("danger", 0xDC2626, 0xFF565F, 160, 203, Some(1)),
    ("focus", 0x4069FD, 0x94ADFF, 12, 111, Some(12)),
];

fn rgb(hex: u32) -> Fg {
    Fg::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// The foreground a token paints at a depth and theme (`Fg::Default` for ANSI-16 muted).
fn token_fg(token: &str, depth: Depth, theme: Theme) -> Fg {
    let p = PALETTE
        .iter()
        .find(|p| p.0 == token)
        .unwrap_or_else(|| panic!("no token {token}"));
    match (depth, theme) {
        (Depth::True, Theme::Light) => rgb(p.1),
        (Depth::True, Theme::Dark) => rgb(p.2),
        (Depth::X256, Theme::Light) => Fg::Idx(p.3),
        (Depth::X256, Theme::Dark) => Fg::Idx(p.4),
        (Depth::X16, _) => p.5.map(Fg::Idx).unwrap_or(Fg::Default),
    }
}

/// The token a foreground colour belongs to at this depth and theme.
fn token_of(fg: Fg, depth: Depth, theme: Theme) -> Option<&'static str> {
    PALETTE
        .iter()
        .map(|p| p.0)
        .find(|t| fg != Fg::Default && token_fg(t, depth, theme) == fg)
}

// =============================================================================================
// Pseudo-terminals.
// =============================================================================================

/// Held around opening a pty master (until it is close-on-exec) and around this harness's spawns.
static FORK_LOCK: Mutex<()> = Mutex::new(());

pub struct Pty {
    master: fs::File,
    path: PathBuf,
    /// A slave fd the test keeps, to read termios before and after the child.
    slave: fs::File,
}

impl Pty {
    pub fn open(cols: u16, rows: u16) -> Pty {
        use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
        // The master must not leak into any child (glen, or another test's processes): a leaked
        // master keeps the terminal alive after the test closes it.
        let m: OwnedFd = {
            let _fork = FORK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            #[cfg(target_os = "linux")]
            let m = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)
                .expect("openpt");
            #[cfg(not(target_os = "linux"))]
            let m = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).expect("openpt");
            rustix::io::fcntl_setfd(&m, rustix::io::FdFlags::CLOEXEC).expect("FD_CLOEXEC");
            m
        };
        grantpt(&m).expect("grantpt");
        unlockpt(&m).expect("unlockpt");
        let name = ptsname(&m, Vec::new()).expect("ptsname");
        let path = PathBuf::from(name.to_str().unwrap());
        let master = fs::File::from(m);
        let slave = Self::open_path(&path);
        let p = Pty {
            master,
            path,
            slave,
        };
        p.set_size(cols, rows);
        p
    }

    fn open_path(path: &Path) -> fs::File {
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY)
            .open(path)
            .unwrap_or_else(|e| panic!("open {}: {e}", path.display()))
    }

    pub fn open_slave(&self) -> fs::File {
        Self::open_path(&self.path)
    }

    pub fn set_size(&self, cols: u16, rows: u16) {
        rustix::termios::tcsetwinsize(
            &self.master,
            rustix::termios::Winsize {
                ws_row: rows,
                ws_col: cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )
        .expect("tcsetwinsize");
    }

    /// (ICANON, ECHO) of the slave, or None when the slave is no longer usable.
    pub fn cooked(&self) -> Option<(bool, bool)> {
        let t = rustix::termios::tcgetattr(&self.slave).ok()?;
        Some((
            t.local_modes.contains(rustix::termios::LocalModes::ICANON),
            t.local_modes.contains(rustix::termios::LocalModes::ECHO),
        ))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ctty {
    /// New session leader without a controlling terminal: termios stays readable after exit on
    /// every platform (design §A15), SIGWINCH is sent by the test.
    No,
    /// The pty is the child's controlling terminal (the way a shell runs it).
    Yes,
}

/// Which of the child's standard streams are the pty.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wire {
    All,
    /// stdin is `/dev/null`, stdout and stderr the pty.
    NullStdin,
}

/// `glen` running on a pty, with its output replayed into a `Vt`.
pub struct Tui {
    child: Option<Child>,
    pub pid: u32,
    pub pty: Pty,
    ctty: Ctty,
    out: Arc<Mutex<Vec<u8>>>,
    stop: Arc<AtomicBool>,
    vt: Vt,
    fed: usize,
    pub desc: String,
    status: Option<ExitStatus>,
    exited_at: Option<Instant>,
}

/// What to run, built from a box environment.
pub struct Spec {
    cmd: Command,
    desc: String,
}

impl Spec {
    /// `glen <args>` with the `RpcBox` environment (fixtures, temp HOME, GLEN_HOME, fake omp),
    /// `TERM=xterm-256color`, from `cwd`.
    pub fn new(rb: &RpcBox, cwd: &Path, args: &[&str]) -> Spec {
        let a: Vec<OsString> = args.iter().map(OsString::from).collect();
        let mut cmd = rb.command_in(cwd, &a);
        // A real HOME is canonical; the temp sandbox may sit behind a symlink (macOS /var).
        cmd.env("HOME", fs::canonicalize(&rb.bx.home).unwrap())
            .current_dir(fs::canonicalize(cwd).unwrap())
            .env("TERM", "xterm-256color");
        Spec {
            cmd,
            desc: args.join(" "),
        }
    }

    pub fn env(mut self, k: &str, v: impl AsRef<std::ffi::OsStr>) -> Spec {
        self.cmd.env(k, v);
        self
    }
}

impl Tui {
    pub fn start(spec: Spec, cols: u16, rows: u16, ctty: Ctty) -> Tui {
        Self::start_wired(spec, cols, rows, ctty, Wire::All)
    }

    fn start_wired(spec: Spec, cols: u16, rows: u16, ctty: Ctty, wire: Wire) -> Tui {
        let pty = Pty::open(cols, rows);
        let Spec { mut cmd, desc } = spec;
        let slave = pty.open_slave();
        match wire {
            Wire::All => {
                cmd.stdin(Stdio::from(slave.try_clone().unwrap()));
            }
            Wire::NullStdin => {
                cmd.stdin(Stdio::null());
            }
        }
        cmd.stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave));
        let want_ctty = ctty == Ctty::Yes;
        unsafe {
            cmd.pre_exec(move || {
                rustix::process::setsid().map_err(std::io::Error::from)?;
                if want_ctty {
                    rustix::process::ioctl_tiocsctty(BorrowedFd::borrow_raw(1))
                        .map_err(std::io::Error::from)?;
                }
                Ok(())
            });
        }
        let child = {
            let _fork = FORK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            cmd.spawn()
        }
        .unwrap_or_else(|e| panic!("spawn glen {desc}: {e}"));
        let pid = child.id();
        let out = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let mut reader = pty.master.try_clone().expect("dup master");
        let (sink, st) = (Arc::clone(&out), Arc::clone(&stop));
        std::thread::spawn(move || {
            let mut buf = [0u8; 65536];
            while !st.load(Ordering::Relaxed) {
                let mut pfd = libc::pollfd {
                    fd: reader.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                let r = unsafe { libc::poll(&mut pfd, 1, 50) };
                if r <= 0 {
                    continue;
                }
                if pfd.revents & libc::POLLIN == 0 {
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => std::thread::sleep(Duration::from_millis(20)),
                    Ok(n) => sink.lock().unwrap().extend_from_slice(&buf[..n]),
                }
            }
        });
        Tui {
            child: Some(child),
            pid,
            vt: Vt::new(cols as usize, rows as usize),
            pty,
            ctty,
            out,
            stop,
            fed: 0,
            desc,
            status: None,
            exited_at: None,
        }
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.out.lock().unwrap().clone()
    }

    pub fn raw(&self) -> String {
        String::from_utf8_lossy(&self.bytes()).into_owned()
    }

    /// Replay new output and return the screen.
    pub fn screen(&mut self) -> &Vt {
        let b = self.bytes();
        if b.len() > self.fed {
            self.vt.feed(&b[self.fed..]);
            self.fed = b.len();
        }
        &self.vt
    }

    pub fn ctx(&mut self) -> String {
        let dump = self.screen().dump();
        let raw = self.bytes();
        let tail = &raw[raw.len().saturating_sub(300)..];
        format!(
            "`glen {}` (status {:?})\n--- screen {}x{} ---\n{dump}--- last output bytes ---\n{:?}",
            self.desc,
            self.status,
            self.vt.cols,
            self.vt.rows,
            String::from_utf8_lossy(tail)
        )
    }

    pub fn send(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(4096) {
            self.pty.master.write_all(chunk).expect("write to the pty");
        }
        self.pty.master.flush().unwrap();
    }

    /// Send a key and give the UI a moment (lone ESC needs its own read).
    pub fn key(&mut self, bytes: &[u8]) {
        self.send(bytes);
        std::thread::sleep(Duration::from_millis(if bytes == ESC { 150 } else { 40 }));
    }

    pub fn typ(&mut self, text: &str) {
        self.send(text.as_bytes());
        std::thread::sleep(Duration::from_millis(40));
    }

    /// A bracketed paste.
    pub fn paste(&mut self, text: &[u8]) {
        let mut b = b"\x1b[200~".to_vec();
        b.extend_from_slice(text);
        b.extend_from_slice(b"\x1b[201~");
        let mut master = self.pty.master.try_clone().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for chunk in b.chunks(4096) {
                if master.write_all(chunk).is_err() {
                    break;
                }
            }
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(60))
            .expect("the paste was not consumed within 60 s");
        std::thread::sleep(Duration::from_millis(60));
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.screen();
        if (cols as usize, rows as usize) == (self.vt.cols, self.vt.rows) {
            return;
        }
        self.pty.set_size(cols, rows);
        self.vt.resize(cols as usize, rows as usize);
        if self.ctty == Ctty::No {
            let _ = rustix::process::kill_process(
                rustix::process::Pid::from_raw(self.pid as i32).unwrap(),
                Signal::WINCH,
            );
        }
    }

    pub fn signal(&self, sig: Signal) {
        rustix::process::kill_process(
            rustix::process::Pid::from_raw(self.pid as i32).unwrap(),
            sig,
        )
        .expect("signal glen");
    }

    pub fn try_status(&mut self) -> Option<ExitStatus> {
        if self.status.is_none()
            && let Some(c) = self.child.as_mut()
            && let Ok(Some(st)) = c.try_wait()
        {
            self.status = Some(st);
            self.exited_at = Some(Instant::now());
        }
        self.status
    }

    pub fn alive(&mut self) -> bool {
        self.try_status().is_none()
    }

    /// Wait for exit; returns the exit code (panics on a signal death).
    pub fn wait_exit(&mut self, timeout: Duration) -> i32 {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(st) = self.try_status() {
                std::thread::sleep(Duration::from_millis(150));
                self.screen();
                return st.code().unwrap_or_else(|| {
                    let ctx = self.ctx();
                    panic!("glen was killed by a signal\n{ctx}")
                });
            }
            if Instant::now() >= deadline {
                let ctx = self.ctx();
                panic!("glen did not exit within {timeout:?}\n{ctx}");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Poll the screen until `pred` holds.
    pub fn wait_for(&mut self, what: &str, timeout: Duration, pred: impl Fn(&Vt) -> bool) {
        let deadline = Instant::now() + timeout;
        loop {
            if pred(self.screen()) {
                return;
            }
            if Instant::now() >= deadline {
                let ctx = self.ctx();
                panic!("timed out after {timeout:?} waiting for {what}\n{ctx}");
            }
            if self.status.is_none() && self.try_status().is_some() {
                std::thread::sleep(Duration::from_millis(150));
                if pred(self.screen()) {
                    return;
                }
                let ctx = self.ctx();
                panic!("glen exited while waiting for {what}\n{ctx}");
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    /// Time until `pred` holds (bounded by `timeout`).
    pub fn time_until(
        &mut self,
        what: &str,
        timeout: Duration,
        pred: impl Fn(&Vt) -> bool,
    ) -> Duration {
        let t0 = Instant::now();
        self.wait_for(what, timeout, pred);
        t0.elapsed()
    }

    pub fn wait_text(&mut self, needle: &str, timeout: Duration) {
        let n = needle.to_owned();
        self.wait_for(&format!("{needle:?} on screen"), timeout, move |v| {
            v.contains(&n)
        });
    }

    /// `pred` stays false for `dur`.
    pub fn never(&mut self, what: &str, dur: Duration, pred: impl Fn(&Vt) -> bool) {
        let deadline = Instant::now() + dur;
        while Instant::now() < deadline {
            if pred(self.screen()) {
                let ctx = self.ctx();
                panic!("{what}\n{ctx}");
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Wait until the screen has not changed for `quiet` (glen redraws on every tick, so the
    /// byte stream itself never goes quiet).
    pub fn settle(&mut self, quiet: Duration) {
        let norm = |v: &Vt| -> Vec<String> {
            v.lines()
                .into_iter()
                .map(|l| {
                    l.chars()
                        .map(|c| if SPINNER.contains(c) { '*' } else { c })
                        .collect()
                })
                .collect()
        };
        let deadline = Instant::now() + UI;
        let mut last = norm(self.screen());
        let mut since = Instant::now();
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
            let now = norm(self.screen());
            if now != last {
                last = now;
                since = Instant::now();
            } else if since.elapsed() >= quiet {
                break;
            }
        }
    }

    /// The first frame: the list or an empty-state text is drawn.
    pub fn ready(&mut self) {
        self.wait_for("the first frame", UI, |v| v.alt && rule_row(v).is_some());
        self.settle(Duration::from_millis(150));
    }

    /// Quit with `q` from a pane and wait for exit 0.
    pub fn quit(&mut self) {
        // Esc closes an overlay, dismisses a notice or leaves the prompt; q then quits from a pane.
        for _ in 0..3 {
            if focus(self.screen()) != Some(Focus::Prompt) && !overlay_open(self.screen()) {
                break;
            }
            self.key(ESC);
            self.settle(Duration::from_millis(60));
        }
        self.key(b"q");
        let code = self.wait_exit(UI);
        if code != 0 {
            let ctx = self.ctx();
            panic!("quit exited {code}\n{ctx}");
        }
    }

    /// Press Tab until `want` has focus.
    pub fn focus_to(&mut self, want: Focus) {
        for _ in 0..4 {
            if focus(self.screen()) == Some(want) {
                return;
            }
            self.key(TAB);
            self.settle(Duration::from_millis(80));
        }
        let ctx = self.ctx();
        panic!("could not move focus to {want:?}\n{ctx}");
    }

    /// The restore sequences after the last alternate-screen entry, and (when readable) cooked
    /// termios on the slave (AC-522).
    pub fn assert_restored(&mut self) {
        let b = self.bytes();
        let last_on = rfind(&b, ALT_ON)
            .unwrap_or_else(|| panic!("the alternate screen was never entered\n{}", self.ctx()));
        let after = &b[last_on..];
        assert!(
            find(after, ALT_OFF).is_some(),
            "the alternate screen was not left\n{}",
            self.ctx()
        );
        assert!(
            find(after, CURSOR_ON).is_some(),
            "the cursor was not shown again\n{}",
            self.ctx()
        );
        assert!(
            find(&b, PASTE_OFF).is_some(),
            "bracketed paste was not disabled\n{}",
            self.ctx()
        );
        if let Some((icanon, echo)) = self.pty.cooked() {
            assert!(
                icanon && echo,
                "raw mode was not disabled (ICANON {icanon}, ECHO {echo})\n{}",
                self.ctx()
            );
        } else {
            assert!(
                self.ctty == Ctty::Yes && cfg!(target_os = "macos"),
                "the slave termios is unreadable after exit\n{}",
                self.ctx()
            );
        }
    }

    /// Raw mode while running: ICANON and ECHO are off on the slave.
    pub fn assert_raw(&mut self) {
        let (icanon, echo) = self.pty.cooked().expect("termios while running");
        assert!(
            !icanon && !echo,
            "raw mode is not on while watch runs (ICANON {icanon}, ECHO {echo})\n{}",
            self.ctx()
        );
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take()
            && self.status.is_none()
        {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

pub fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

// =============================================================================================
// Reading glen watch's screen (Design spec regions).
// =============================================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    TooSmall,
    Narrow,
    Medium,
    Wide,
}

/// AC-517 breakpoints.
pub fn mode_of(cols: usize, rows: usize) -> Mode {
    if cols < 40 || rows < 10 {
        Mode::TooSmall
    } else if cols < 72 {
        Mode::Narrow
    } else if cols < 100 {
        Mode::Medium
    } else {
        Mode::Wide
    }
}

/// The list width (Design spec: 34 wide, 24 medium) or None in narrow mode.
pub fn list_width(v: &Vt) -> Option<usize> {
    match mode_of(v.cols, v.rows) {
        Mode::Wide => Some(34),
        Mode::Medium => Some(24),
        _ => None,
    }
}

/// The transcript pane's inner columns `[x0, x1)` (1-column padding on each side).
pub fn pane_cols(v: &Vt) -> (usize, usize) {
    match list_width(v) {
        Some(w) => (w + 2, v.cols - 1),
        None => (1, v.cols - 1),
    }
}

/// W, the transcript's inner width.
pub fn pane_width(cols: usize, rows: usize) -> usize {
    match mode_of(cols, rows) {
        Mode::Wide => cols - 35 - 2,
        Mode::Medium => cols - 25 - 2,
        _ => cols - 2,
    }
}

const MODE_LABELS: [&str; 4] = ["auto", "steer", "follow-up", "sending…"];

/// The prompt rule: `─` across the width with the mode label inset two columns from the right.
pub fn rule_row(v: &Vt) -> Option<usize> {
    (0..v.rows).rev().find(|&y| {
        let r = v.row(y);
        let t = r.trim_end();
        t.starts_with('─')
            && t.ends_with(" ──")
            && MODE_LABELS
                .iter()
                .any(|l| t.contains(&format!(" {l} ")) || t.contains(l))
    })
}

/// The mode label on the prompt rule (`auto`, `steer`, `follow-up`, or `… sending…`).
pub fn mode_label(v: &Vt) -> String {
    let y = rule_row(v).expect("prompt rule");
    v.row(y)
        .trim_matches(|c| c == '─' || c == ' ')
        .trim()
        .to_owned()
}

/// The first input row (the one with `❯`) below the rule.
pub fn input_row(v: &Vt) -> Option<usize> {
    let r = rule_row(v)?;
    (r + 1..v.rows.saturating_sub(1)).find(|&y| v.row(y).contains('❯'))
}

/// The prompt's text: the rows after `<handle> ❯ `, continuation rows aligned after the prefix.
pub fn input_text(v: &Vt) -> String {
    let Some(y0) = input_row(v) else {
        return String::new();
    };
    let (_, px) = v.find_all("❯").into_iter().find(|(y, _)| *y == y0).unwrap();
    let start = px + 2;
    let mut lines = Vec::new();
    for y in y0..v.rows - 1 {
        lines.push(v.text(y, start, v.cols).trim_end().to_owned());
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Notice rows between the rule and the input.
pub fn notice_rows(v: &Vt) -> Vec<String> {
    let (Some(r), Some(i)) = (rule_row(v), input_row(v)) else {
        return Vec::new();
    };
    (r + 1..i).map(|y| v.row(y).trim_end().to_owned()).collect()
}

pub fn status_bar(v: &Vt) -> String {
    v.row(v.rows - 1).trim_end().to_owned()
}

pub fn title_bar(v: &Vt) -> String {
    v.row(0).trim_end().to_owned()
}

pub fn tab_strip(v: &Vt) -> String {
    v.row(1).trim_end().to_owned()
}

/// Body rows (between the tab strip and the prompt rule).
pub fn body(v: &Vt) -> std::ops::Range<usize> {
    2..rule_row(v).unwrap_or(v.rows)
}

fn all_rule(s: &str) -> bool {
    let t = s.trim();
    t.chars().count() >= 3 && t.chars().all(|c| c == '─')
}

/// The transcript pane's rows above the activity strip, trimmed on the right.
pub fn pane_lines(v: &Vt) -> Vec<String> {
    let (x0, x1) = pane_cols(v);
    let mut out = Vec::new();
    for y in body(v) {
        let t = v.text(y, x0, x1);
        if all_rule(&v.text(y, x0 - 1, v.cols)) {
            break;
        }
        out.push(t.trim_end().to_owned());
    }
    out
}

/// The activity strip rows (after its `─` rule), trimmed.
pub fn strip_lines(v: &Vt) -> Vec<String> {
    let (x0, _) = pane_cols(v);
    let rows: Vec<usize> = body(v).collect();
    match rows
        .iter()
        .position(|&y| all_rule(&v.text(y, x0 - 1, v.cols)))
    {
        Some(i) => rows[i + 1..]
            .iter()
            .map(|&y| v.text(y, x0 - 1, v.cols).trim().to_owned())
            .collect(),
        None => Vec::new(),
    }
}

/// The strip row containing `needle`, with its screen row and column.
pub fn strip_find(v: &Vt, needle: &str) -> Option<(usize, usize)> {
    let r = rule_row(v)?;
    let (x0, _) = pane_cols(v);
    v.find_all(needle)
        .into_iter()
        .find(|(y, x)| *y >= 2 && *y < r && *x + 1 >= x0)
}

/// A list row's `(selected, glyph, handle)`: `▌` or a space, then the state glyph, a space and a
/// 7-hex handle (one column further right in narrow mode, which pads the pane by one column).
pub fn list_entry(v: &Vt, y: usize) -> Option<(bool, char, String)> {
    let w = list_width(v).unwrap_or(v.cols);
    let cs: Vec<char> = v.text(y, 0, w).chars().collect();
    for start in 0..2 {
        let (Some(bar), Some(glyph)) = (cs.get(start), cs.get(start + 1)) else {
            continue;
        };
        if !matches!(*bar, '▌' | ' ') || !matches!(*glyph, '●' | '◐' | '○' | '?') {
            continue;
        }
        if cs.get(start + 2) != Some(&' ') || cs.len() < start + 10 {
            continue;
        }
        let h: String = cs[start + 3..start + 10].iter().collect();
        let tail_ok = cs.get(start + 10).is_none_or(|c| !c.is_ascii_hexdigit());
        if tail_ok
            && h.chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        {
            return Some((*bar == '▌', *glyph, h));
        }
    }
    None
}

/// Handles in list order.
pub fn list_handles(v: &Vt) -> Vec<String> {
    body(v)
        .filter_map(|y| list_entry(v, y))
        .map(|e| e.2)
        .collect()
}

/// (row, text of the list columns) of the list row holding `handle`.
pub fn list_row_of(v: &Vt, handle: &str) -> Option<(usize, String)> {
    let w = list_width(v).unwrap_or(v.cols);
    body(v).find_map(|y| {
        list_entry(v, y)
            .filter(|e| e.2 == handle)
            .map(|_| (y, v.text(y, 0, w)))
    })
}

/// The handle on the row marked selected (`▌`).
pub fn selected_handle(v: &Vt) -> Option<String> {
    body(v).find_map(|y| list_entry(v, y).filter(|e| e.0).map(|e| e.2))
}

/// The help or inbox overlay is open (its footer or controls row is on screen).
pub fn overlay_open(v: &Vt) -> bool {
    v.contains("peek only · glen inbox marks these read") || v.contains("(not in this build)")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Sessions,
    Transcript,
    Prompt,
}

/// Focus from the glyphs: the terminal cursor is shown only while the prompt has focus; else the
/// `▍` marks the focused pane's tab.
pub fn focus(v: &Vt) -> Option<Focus> {
    if v.cursor_visible {
        return Some(Focus::Prompt);
    }
    let tabs = v.row(1);
    let (_, x) = v.find_all("▍").into_iter().find(|(y, _)| *y == 1)?;
    let after: String = tabs.chars().skip_while(|c| *c != '▍').skip(1).collect();
    match list_width(v) {
        Some(w) => Some(if x < w {
            Focus::Sessions
        } else {
            Focus::Transcript
        }),
        None => Some(if after.starts_with("sessions") {
            Focus::Sessions
        } else {
            Focus::Transcript
        }),
    }
}

// =============================================================================================
// Session builders and CLI references.
// =============================================================================================

/// A new top-level session in `bucket` with a 256-byte title slot (as omp writes it) when
/// `title` is given, created now (newest mtime).
fn add_session(
    bx: &OmpBox,
    bucket: &str,
    id: &str,
    title: Option<&str>,
    cwd: &str,
    body: &[Value],
) -> PathBuf {
    let created = "2026-10-06T10:00:00.000Z";
    let path = bx.bucket(bucket).join(session_name(created, id));
    let mut text = String::new();
    if let Some(t) = title {
        let mut slot = title_slot(t).to_string();
        // Pad to 255 bytes + newline, the fixture shape.
        if slot.len() < 255 {
            let pad = 255 - slot.len() - ",\"pad\":\"\"".len();
            slot.pop();
            slot.push_str(&format!(",\"pad\":\"{}\"}}", " ".repeat(pad)));
        }
        text.push_str(&slot);
        text.push('\n');
    }
    text.push_str(&format!("{}\n", header(id, Some(created), Some(cwd))));
    text.push_str(&jsonl(body));
    fs::write(&path, text).unwrap();
    path
}

/// `n` user messages chained after each other, each with a distinct text.
fn chain(
    n: usize,
    from: usize,
    text: impl Fn(usize) -> String,
    parent: Option<String>,
) -> Vec<Value> {
    let mut out = Vec::new();
    let mut p = parent;
    for i in from..from + n {
        let id = format!("{:08x}", 0x7000_0000 + i);
        out.push(user(
            &id,
            p.as_deref(),
            "2026-10-06T10:00:01.000Z",
            &text(i),
        ));
        p = Some(id);
    }
    out
}

fn long_text(i: usize) -> String {
    format!(
        "msg {i:04} alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima \
         mike november oscar papa quebec romeo sierra tango uniform victor whiskey xray"
    )
}

/// `glen show <handle> --human` at `COLUMNS=width` with `extra` flags and `env`: the transcript
/// lines after `show`'s header block (title and meta line, then a blank line).
fn show_body(
    rb: &RpcBox,
    handle: &str,
    width: usize,
    extra: &[&str],
    env: &[(&str, &str)],
) -> String {
    let mut args = vec!["show", handle, "--human"];
    args.extend_from_slice(extra);
    let mut run = rb.glen(args).env("COLUMNS", width.to_string());
    for (k, v) in env {
        run = run.env(k, v);
    }
    let out = run.run();
    out.assert_exit(0);
    let text = out.out();
    let plain = strip_ansi(&text);
    let mut lines: Vec<&str> = text.split('\n').collect();
    let plain_lines: Vec<&str> = plain.split('\n').collect();
    let skip = plain_lines
        .iter()
        .position(|l| l.trim().is_empty())
        .map(|i| i + 1)
        .unwrap_or(0);
    lines.drain(..skip);
    while lines
        .last()
        .is_some_and(|l| strip_ansi(l).trim().is_empty())
    {
        lines.pop();
    }
    lines.join("\n")
}

fn show_lines(rb: &RpcBox, handle: &str, width: usize, extra: &[&str]) -> Vec<String> {
    strip_ansi(&show_body(rb, handle, width, extra, &[("NO_COLOR", "1")]))
        .split('\n')
        .map(|l| l.trim_end().to_owned())
        .collect()
}

fn trim_blank(mut v: Vec<String>) -> Vec<String> {
    while v.last().is_some_and(|l| l.is_empty()) {
        v.pop();
    }
    let lead = v.iter().take_while(|l| l.is_empty()).count();
    v.drain(..lead);
    v
}

/// The pane shows `want` (or its last page when it is longer than the pane).
fn pane_matches(v: &Vt, want: &[String]) -> bool {
    let pane = pane_lines(v);
    let h = pane.len();
    if want.len() <= h {
        trim_blank(pane) == want
    } else {
        pane == want[want.len() - h..]
    }
}

/// Where the pane's rows start inside `want` (a unique window match), ignoring the AC-506
/// indicator row when present.
fn window_offset(v: &Vt, want: &[String]) -> Option<usize> {
    let mut pane = pane_lines(v);
    if pane.last().is_some_and(|l| l.contains("G follows")) {
        pane.pop();
    }
    let h = pane.len();
    if h == 0 || want.len() < h {
        return None;
    }
    let hits: Vec<usize> = (0..=want.len() - h)
        .filter(|&k| want[k..k + h] == pane[..])
        .collect();
    (hits.len() == 1).then(|| hits[0])
}

/// The `ls --json` rows of a scope: (handle, state, title).
fn ls_rows(rb: &RpcBox, cwd: &Path, extra: &[&str]) -> Vec<(String, String, Option<String>)> {
    let mut args = vec!["ls", "-n", "0", "--json"];
    args.extend_from_slice(extra);
    let d = rb.bx.data_in(cwd, args);
    arr(&d["sessions"])
        .iter()
        .map(|s| {
            (
                s["handle"].as_str().unwrap().to_owned(),
                s["state"].as_str().unwrap().to_owned(),
                s["title"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

fn state_glyph(state: &str) -> &'static str {
    match state {
        "live" => "●",
        "busy" => "◐",
        "idle" => "○",
        _ => "?",
    }
}

/// A truncated rendering of `full` that fits: equal, or a prefix ending in `…`.
fn fits(shown: &str, full: &str) -> bool {
    let s = shown.trim();
    s == full
        || s.strip_suffix('…')
            .is_some_and(|p| full.starts_with(p.trim_end()))
}

fn start(rb: &RpcBox, args: &[&str], cols: u16, rows: u16) -> Tui {
    let mut t = Tui::start(Spec::new(rb, &rb.bx.alpha, args), cols, rows, Ctty::No);
    t.ready();
    t
}

fn start_env(rb: &RpcBox, args: &[&str], env: &[(&str, &str)], cols: u16, rows: u16) -> Tui {
    let mut spec = Spec::new(rb, &rb.bx.alpha, args);
    for (k, v) in env {
        spec = spec.env(k, v);
    }
    let mut t = Tui::start(spec, cols, rows, Ctty::No);
    t.ready();
    t
}

/// Wait for the list to show `handle` and the transcript to have content.
fn wait_list(t: &mut Tui, handle: &str) {
    let h = handle.to_owned();
    t.wait_for(&format!("{handle} in the list"), UI, move |v| {
        list_handles(v).contains(&h)
    });
}

fn wait_pane(t: &mut Tui, needle: &str) {
    let n = needle.to_owned();
    t.wait_for(
        &format!("{needle:?} in the transcript pane"),
        UI,
        move |v| pane_lines(v).iter().any(|l| l.contains(&n)),
    );
}

// =============================================================================================
// More helpers.
// =============================================================================================

/// A user entry appended to fixture A's copy (its leaf is 10000009).
fn a_user(id: &str, parent: &str, text: &str) -> Value {
    user(id, Some(parent), "2026-10-01T09:10:00.000Z", text)
}

/// Publish a continuation of A in A's bucket the way omp does (stage a dotfile, rename): A's
/// entries plus `next`, header `continuationOf` A.
fn continue_a(bx: &OmpBox, new_id: &str, next: &Value) -> PathBuf {
    let created = "2026-10-01T10:00:00.000Z";
    let path = bx.sessions.join(ALPHA).join(session_name(created, new_id));
    let mut h = header(new_id, Some(created), Some(A.cwd));
    h["parentSession"] = Value::from(A.id);
    h["continuationOf"] = Value::from(A.id);
    let mut text = format!("{h}\n");
    for l in entry_lines(&bx.path(&A)) {
        text.push_str(&l);
        text.push('\n');
    }
    text.push_str(&format!("{next}\n"));
    let stage = path.with_file_name(format!(
        ".{}.staged.tmp",
        path.file_name().unwrap().to_str().unwrap()
    ));
    fs::write(&stage, text).unwrap();
    fs::rename(&stage, &path).unwrap();
    path
}

/// A new session with `n` one-line user items (`item NNN …`); returns (id, handle).
fn many_items(bx: &OmpBox, tag: u64, n: usize, text: impl Fn(usize) -> String) -> (String, String) {
    let id = uuid(tag);
    add_session(
        bx,
        ALPHA,
        &id,
        Some("Many items"),
        A.cwd,
        &chain(n, 1, text, None),
    );
    let h = tail7(&id);
    (id, h)
}

fn short_item(i: usize) -> String {
    format!("item {i:04} short line")
}

/// Time from now until the alternate screen has been left after its last entry.
fn time_to_restore(t: &Tui, t0: Instant, timeout: Duration) -> Option<Duration> {
    loop {
        let b = t.bytes();
        if let Some(on) = rfind(&b, ALT_ON)
            && find(&b[on..], ALT_OFF).is_some()
        {
            return Some(t0.elapsed());
        }
        if t0.elapsed() > timeout {
            return None;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Time from now until the child has exited.
fn time_to_exit(t: &mut Tui, t0: Instant, timeout: Duration) -> Option<Duration> {
    loop {
        if t.try_status().is_some() {
            return Some(t0.elapsed());
        }
        if t0.elapsed() > timeout {
            return None;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn inbox_path(rb: &RpcBox, id: &str) -> PathBuf {
    rb.bx.glen_home.join("inbox").join(format!("{id}.ndjson"))
}

fn record(session: &str, text: &str, fin: bool, at: jiff::Timestamp) -> Value {
    let mut r = inbox_record(session, None, text, &at.to_string());
    r["final"] = Value::Bool(fin);
    r
}

/// Every coloured cell maps to a palette token of this depth and theme, and nothing sets a
/// background. Returns the tokens seen.
fn palette_tokens(v: &Vt, depth: Depth, theme: Theme, ctx: &str) -> BTreeSet<&'static str> {
    assert!(
        v.bg_sgr.is_empty(),
        "a background colour was set: {:?}\n{ctx}",
        v.bg_sgr
    );
    let mut seen = BTreeSet::new();
    for c in v.all_cells() {
        assert!(!c.bg, "cell {:?} has a background\n{ctx}", c.ch);
        if c.fg == Fg::Default {
            continue;
        }
        match token_of(c.fg, depth, theme) {
            Some(t) => {
                seen.insert(t);
            }
            None => panic!(
                "cell {:?} has colour {:?}, which is no {depth:?}/{theme:?} palette token\n{ctx}",
                c.ch, c.fg
            ),
        }
    }
    seen
}

/// `job.json` once it is readable (the job directory appears first).
fn job_of(rb: &RpcBox, id: &str) -> Value {
    rb.wait_file(&rb.job_dir(id).join("job.json"), LONG)
}

fn job_ids_now(rb: &RpcBox) -> Vec<String> {
    rb.job_ids()
}

/// Wait for a job id not in `before`.
fn new_job(rb: &RpcBox, before: &[String]) -> String {
    poll_until("a new job directory", LONG, || {
        rb.job_ids().into_iter().find(|i| !before.contains(i))
    })
}

fn wait_file_exists(p: &Path, timeout: Duration) {
    poll_until(&format!("{}", p.display()), timeout, || {
        p.exists().then_some(())
    });
}

fn ready_path(gate: &Path) -> PathBuf {
    PathBuf::from(format!("{}.ready", gate.display()))
}

// =============================================================================================
// AC-501: refusal off a terminal.
// =============================================================================================

#[test]
fn ac501_piped_watch_is_a_json_usage_error_that_touches_nothing() {
    let rb = RpcBox::new();
    let before = snapshot(&rb.bx.root);
    let beta = s(&rb.bx.beta);
    for args in [
        vec!["watch"],
        vec!["watch", "e27c0de", "--all"],
        vec!["watch", "nosuch1"],
        vec!["watch", "--cwd", beta.as_str()],
    ] {
        let out = rb.glen(&args).run();
        let err = out.error_envelope("usage");
        out.assert_stderr_empty().assert_no_ansi();
        assert_eq!(err["message"], TERMINAL_MSG, "{}", out.ctx());
        assert_eq!(
            plain(&err["hint"]),
            "agents: glen tail -f <session> and glen inbox -f",
            "{}",
            out.ctx()
        );
    }
    assert!(!rb.bx.glen_home.exists(), "GLEN_HOME was created");
    assert_eq!(snapshot(&rb.bx.root), before, "watch wrote a file");
}

#[test]
fn ac501_human_mode_reports_the_refusal_on_stderr() {
    let rb = RpcBox::new();
    let out = rb.glen(["watch", "--human"]).run();
    out.assert_exit(2).assert_no_ansi();
    assert!(out.stdout.is_empty(), "{}", out.ctx());
    let err = out.err();
    assert_eq!(
        err.lines().next(),
        Some(format!("error: {TERMINAL_MSG}").as_str()),
        "{}",
        out.ctx()
    );
    assert!(
        err.contains("glen tail -f <session>") && err.contains("glen inbox -f"),
        "{}",
        out.ctx()
    );
    assert!(!rb.bx.glen_home.exists());
}

fn assert_refused_before_any_mode(t: &mut Tui) {
    let code = t.wait_exit(UI);
    assert_eq!(code, 2, "{}", t.ctx());
    let b = t.bytes();
    for (seq, what) in [
        (ALT_ON, "alternate screen"),
        (PASTE_ON, "bracketed paste"),
        (b"\x1b[?25l".as_slice(), "cursor hide"),
    ] {
        assert!(
            find(&b, seq).is_none(),
            "{what} was enabled before the refusal\n{}",
            t.ctx()
        );
    }
    assert_eq!(
        t.pty.cooked(),
        Some((true, true)),
        "termios changed\n{}",
        t.ctx()
    );
    assert!(t.raw().contains(TERMINAL_MSG), "{}", t.ctx());
}

#[test]
fn ac501_json_mode_on_a_terminal_is_refused_before_any_mode_change() {
    let rb = RpcBox::new();
    let mut t = Tui::start(
        Spec::new(&rb, &rb.bx.alpha, &["watch", "--json"]),
        120,
        40,
        Ctty::No,
    );
    assert_refused_before_any_mode(&mut t);
    let text = t.raw().replace('\r', "");
    let env: Value = serde_json::from_str(text.trim())
        .unwrap_or_else(|e| panic!("JSON mode prints one envelope ({e})\n{}", t.ctx()));
    assert_eq!(env["ok"], false);
    assert_eq!(env["error"]["code"], "usage");
    assert!(!rb.bx.glen_home.exists());
}

#[test]
fn ac501_term_dumb_is_refused_before_any_mode_change() {
    let rb = RpcBox::new();
    let spec = Spec::new(&rb, &rb.bx.alpha, &["watch"]).env("TERM", "dumb");
    let mut t = Tui::start(spec, 120, 40, Ctty::No);
    assert_refused_before_any_mode(&mut t);
    assert!(t.raw().contains("error:"), "human error line\n{}", t.ctx());
    assert!(!rb.bx.glen_home.exists());
}

#[test]
fn ac501_a_non_terminal_stdin_is_refused() {
    let rb = RpcBox::new();
    let mut t = Tui::start_wired(
        Spec::new(&rb, &rb.bx.alpha, &["watch"]),
        120,
        40,
        Ctty::No,
        Wire::NullStdin,
    );
    assert_refused_before_any_mode(&mut t);
    assert!(!rb.bx.glen_home.exists());
}

// =============================================================================================
// AC-502, AC-503: start, scope, selection, CLI surface.
// =============================================================================================

#[test]
fn ac502_start_enters_the_alternate_screen_raw_mode_and_bracketed_paste() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch"], 120, 40);
    let b = t.bytes();
    assert!(find(&b, ALT_ON).is_some(), "{}", t.ctx());
    assert!(find(&b, PASTE_ON).is_some(), "bracketed paste\n{}", t.ctx());
    t.assert_raw();
    t.quit();
    t.assert_restored();
    rb.bx.assert_no_state();
}

/// The list shows the scope's rows in `ls` order with `ls`'s state glyph and title.
fn assert_list_is_ls(t: &mut Tui, want: &[(String, String, Option<String>)]) {
    let handles: Vec<String> = want.iter().map(|r| r.0.clone()).collect();
    let h2 = handles.clone();
    t.wait_for("the list in ls order", UI, move |v| list_handles(v) == h2);
    let v = t.screen().clone();
    for (h, state, title) in want {
        let (y, _) = list_row_of(&v, h).unwrap();
        let (_, glyph, _) = list_entry(&v, y).unwrap();
        assert_eq!(
            glyph.to_string(),
            state_glyph(state),
            "glyph of {h}\n{}",
            t.ctx()
        );
        // Wide rows put the title on the second line; medium and narrow after the handle.
        let w = list_width(&v).unwrap_or(v.cols);
        let line = if mode_of(v.cols, v.rows) == Mode::Wide {
            v.text(y + 1, 0, w)
        } else {
            v.text(y, 0, w).chars().skip(11).collect()
        };
        let mut shown = line.trim_start_matches(['▌', ' ']).trim().to_owned();
        for turn in [" pending", " interrupted", " aborted", " error"] {
            if let Some(p) = shown.strip_suffix(turn) {
                shown = p.trim_end().to_owned();
            }
        }
        let title = title.clone().unwrap();
        if mode_of(v.cols, v.rows) == Mode::Wide {
            assert!(
                fits(&shown, &title),
                "title of {h}: {shown:?} vs {title:?}\n{}",
                t.ctx()
            );
        }
    }
}

#[test]
fn ac502_the_default_scope_is_the_cwd_bucket_as_ls_lists_it() {
    let rb = RpcBox::new();
    let want = ls_rows(&rb, &rb.bx.alpha, &[]);
    let mut t = start(&rb, &["watch"], 120, 40);
    assert_list_is_ls(&mut t, &want);
    let v = t.screen().clone();
    assert_eq!(
        selected_handle(&v).as_deref(),
        Some(want[0].0.as_str()),
        "newest row selected\n{}",
        t.ctx()
    );
    assert!(
        tab_strip(&v).contains(&format!(
            "{} · {}",
            want[0].0,
            want[0].2.as_deref().unwrap()
        )),
        "the transcript tab names the selection\n{}",
        t.ctx()
    );
    assert!(
        tab_strip(&v).contains("sessions · ~/work/alpha"),
        "list tab\n{}",
        t.ctx()
    );
    t.quit();
}

#[test]
fn ac502_all_lists_every_bucket() {
    let rb = RpcBox::new();
    let want = ls_rows(&rb, &rb.bx.alpha, &["--all"]);
    assert_eq!(want.len(), 7);
    let mut t = start(&rb, &["watch", "--all"], 120, 40);
    assert_list_is_ls(&mut t, &want);
    assert!(
        tab_strip(t.screen()).contains("sessions · all projects"),
        "{}",
        t.ctx()
    );
    t.quit();
}

#[test]
fn ac502_cwd_selects_that_project() {
    let rb = RpcBox::new();
    let beta = s(&fs::canonicalize(&rb.bx.beta).unwrap());
    let want = ls_rows(&rb, &rb.bx.alpha, &["--cwd", &beta]);
    let mut t = start(&rb, &["watch", "--cwd", &beta], 120, 40);
    assert_list_is_ls(&mut t, &want);
    let v = t.screen().clone();
    assert!(
        tab_strip(&v).contains("sessions · ~/work/beta"),
        "{}",
        t.ctx()
    );
    assert_eq!(
        selected_handle(&v).as_deref(),
        Some(want[0].0.as_str()),
        "{}",
        t.ctx()
    );
    t.quit();
}

#[test]
fn ac502_a_session_argument_selects_it_in_its_own_bucket() {
    let rb = RpcBox::new();
    let beta = s(&rb.bx.beta);
    let want = ls_rows(&rb, &rb.bx.alpha, &["--cwd", &beta]);
    let mut t = start(&rb, &["watch", D.handle], 120, 40);
    assert_list_is_ls(&mut t, &want);
    let v = t.screen().clone();
    assert_eq!(
        selected_handle(&v).as_deref(),
        Some(D.handle),
        "{}",
        t.ctx()
    );
    assert!(
        tab_strip(&v).contains(&format!("{} · {}", D.handle, D.title)),
        "{}",
        t.ctx()
    );
    t.quit();
}

#[test]
fn ac502_an_unresolvable_session_prints_shows_error_before_the_alternate_screen() {
    let rb = RpcBox::new();
    for arg in ["1234567", "c0de", "@new", "j1234567"] {
        let show = rb.glen(["show", arg, "--json"]).run();
        let env = show.json();
        let code = env["error"]["code"].as_str().unwrap().to_owned();
        let msg = env["error"]["message"].as_str().unwrap().to_owned();
        let mut t = Tui::start(
            Spec::new(&rb, &rb.bx.alpha, &["watch", arg]),
            120,
            40,
            Ctty::No,
        );
        let exit = t.wait_exit(UI);
        assert_eq!(
            exit,
            exit_for(&code),
            "watch {arg} exits like show ({code})\n{}",
            t.ctx()
        );
        assert!(matches!(exit, 2..=4), "{}", t.ctx());
        assert!(
            t.raw().contains(&msg),
            "show's message {msg:?}\n{}",
            t.ctx()
        );
        assert!(
            find(&t.bytes(), ALT_ON).is_none(),
            "alternate screen entered\n{}",
            t.ctx()
        );
        assert_eq!(t.pty.cooked(), Some((true, true)), "{}", t.ctx());
    }
    assert!(!rb.bx.glen_home.exists());
}

#[test]
fn ac503_help_completions_and_arguments() {
    let rb = RpcBox::new();
    let help = rb.glen(["--help"]).run();
    help.assert_exit(0);
    assert!(
        help.out()
            .lines()
            .any(|l| l.trim_start().starts_with("watch ")),
        "--help lists watch\n{}",
        help.ctx()
    );
    for sh in ["bash", "zsh", "fish", "elvish", "powershell"] {
        let out = rb.glen(["completions", sh]).run();
        out.assert_exit(0);
        assert!(out.out().contains("watch"), "{sh} completions lack watch");
    }
    let wh = rb.glen(["watch", "--help"]).run();
    wh.assert_exit(0);
    for flag in ["--all", "--cwd"] {
        assert!(
            wh.out().contains(flag),
            "watch --help lacks {flag}\n{}",
            wh.ctx()
        );
    }
    let beta = s(&rb.bx.beta);
    let out = rb.glen(["watch", "--all", "--cwd", &beta, "--json"]).run();
    let err = out.error_envelope("usage");
    assert_ne!(
        err["message"],
        TERMINAL_MSG,
        "--all with --cwd is a conflict\n{}",
        out.ctx()
    );
    for args in [vec!["watch", "a", "b"], vec!["watch", "--bogus"]] {
        rb.glen(&args).run().error_envelope("usage");
    }
}

#[test]
fn ac503_all_conflicts_with_cwd_on_a_terminal() {
    let rb = RpcBox::new();
    let beta = s(&rb.bx.beta);
    let mut t = Tui::start(
        Spec::new(&rb, &rb.bx.alpha, &["watch", "--all", "--cwd", &beta]),
        120,
        40,
        Ctty::No,
    );
    assert_eq!(t.wait_exit(UI), 2, "{}", t.ctx());
    assert!(find(&t.bytes(), ALT_ON).is_none(), "{}", t.ctx());
}

#[test]
fn ac503_cwd_picks_the_project_for_at_latest() {
    let rb = RpcBox::new();
    let beta = s(&fs::canonicalize(&rb.bx.beta).unwrap());
    let want = ls_rows(&rb, &rb.bx.alpha, &["--cwd", &beta]);
    let mut t = start(&rb, &["watch", "@latest", "--cwd", &beta], 120, 40);
    assert_list_is_ls(&mut t, &want);
    assert_eq!(
        selected_handle(t.screen()).as_deref(),
        Some(want[0].0.as_str()),
        "{}",
        t.ctx()
    );
    t.quit();
}

// =============================================================================================
// AC-504: the transcript pane.
// =============================================================================================

#[test]
fn ac504_the_transcript_pane_equals_show_at_the_pane_width() {
    let rb = RpcBox::new();
    for (cols, rows) in [(120u16, 40u16), (80, 24), (60, 20)] {
        let want = show_lines(&rb, A.handle, pane_width(cols as usize, rows as usize), &[]);
        let mut t = start(&rb, &["watch", A.handle], cols, rows);
        if mode_of(cols as usize, rows as usize) == Mode::Narrow {
            t.focus_to(Focus::Transcript);
        }
        let w = want.clone();
        t.wait_for(
            &format!("the pane equals show at {cols}x{rows}"),
            UI,
            move |v| pane_matches(v, &w),
        );
        t.quit();
    }
}

#[test]
fn ac504_styled_runs_carry_the_painters_colour_bold_and_dim() {
    let rb = RpcBox::new();
    for env in [
        vec![("COLORTERM", "truecolor")],
        vec![("COLORTERM", "truecolor"), ("GLEN_THEME", "light")],
        vec![("TERM", "xterm-256color")],
        vec![("TERM", "xterm")],
    ] {
        let mut t = start_env(&rb, &["watch", A.handle], &env, 120, 40);
        let w = pane_width(120, 40);
        let want = show_lines(&rb, A.handle, w, &[]);
        let w2 = want.clone();
        t.wait_for("the pane equals show", UI, move |v| pane_matches(v, &w2));
        let mut show_env = vec![("TERM", "xterm-256color")];
        show_env.extend(env.iter().copied());
        let ansi = show_body(&rb, A.handle, w, &["--color", "always"], &show_env);
        let styled = styled_lines(&ansi, w);
        let v = t.screen().clone();
        let (x0, _) = pane_cols(&v);
        let y0 = body(&v)
            .find(|&y| !v.text(y, x0, v.cols).trim().is_empty())
            .unwrap();
        let mut checked = 0;
        for (i, line) in styled.iter().enumerate() {
            let mut col = 0;
            for c in line {
                let cw = unicode_width::UnicodeWidthStr::width(c.ch.as_str()).max(1);
                if !c.ch.trim().is_empty() {
                    let got = &v.grid[y0 + i][x0 + col];
                    assert_eq!(
                        (got.ch.as_str(), got.fg, got.bold, got.dim),
                        (c.ch.as_str(), c.fg, c.bold, c.dim),
                        "row {i} col {col} differs from show --color always under {env:?}\n{}",
                        t.ctx()
                    );
                    checked += 1;
                }
                col += cw;
            }
        }
        assert!(checked > 100, "compared {checked} cells");
        t.quit();
    }
}

#[test]
fn ac504_t_and_o_rebuild_the_pane_like_the_show_flags() {
    let rb = RpcBox::new();
    let w = pane_width(120, 50);
    let mut t = start(&rb, &["watch", A.handle], 120, 50);
    t.focus_to(Focus::Transcript);
    for (key, flags) in [
        (b"t", vec!["--thinking"]),
        (b"t", vec![]),
        (b"o", vec!["--tools", "full"]),
        (b"o", vec!["--tools", "none"]),
        (b"o", vec![]),
    ] {
        let want = show_lines(&rb, A.handle, w, &flags);
        t.key(key);
        let w2 = want.clone();
        t.wait_for(&format!("the pane equals show {flags:?}"), UI, move |v| {
            pane_matches(v, &w2)
        });
    }
    t.quit();
}

#[test]
fn ac504_items_appended_while_following_render_like_tail_f_human() {
    let rb = RpcBox::new();
    let a = rb.bx.path(&A);
    let w = pane_width(120, 40);
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    wait_pane(&mut t, "The test depends on map order. Fixed.");
    let cols = w.to_string();
    let mut st = rb.bx.follow_env(
        &["tail", A.handle, "-f", "--human"],
        &[("COLUMNS", cols.as_str()), ("NO_COLOR", "1")],
    );
    st.wait_for_text("The test depends on map order. Fixed.", T);
    // A tool call, then its result in a later write: the result streams as a standalone item.
    append_line(
        &a,
        &assistant(
            "10000010",
            Some("10000009"),
            "2026-10-01T09:10:00.000Z",
            json!([{"type": "text", "text": "Running the suite."},
                   tool_call("toolu_77", "bash", json!({"command": "cargo test"}))]),
            "toolUse",
        ),
    );
    std::thread::sleep(Duration::from_millis(700));
    append_line(
        &a,
        &tool_result(
            "10000011",
            Some("10000010"),
            "2026-10-01T09:10:01.000Z",
            "toolu_77",
            "bash",
            "test result: ok",
            false,
        ),
    );
    st.wait_for_text("test result: ok", T);
    while st.next_line(Duration::from_millis(400)).is_some() {}
    let tail_text: String = strip_ansi(&st.lines.concat());
    let after = |lines: Vec<String>| -> Vec<String> {
        let i = lines
            .iter()
            .position(|l| l.contains("The test depends on map order. Fixed."))
            .unwrap();
        trim_blank(lines[i + 1..].to_vec())
    };
    let want = after(tail_text.lines().map(|l| l.trim_end().to_owned()).collect());
    assert!(
        want.iter().any(|l| l.contains("Running the suite."))
            && want.iter().any(|l| l.contains("test result: ok")),
        "tail -f printed both appended items: {want:?}"
    );
    t.wait_for("the tool result", UI, |v| {
        pane_lines(v).iter().any(|l| l.contains("test result: ok"))
    });
    t.settle(Duration::from_millis(200));
    let got = after(pane_lines(t.screen()));
    assert_eq!(
        got,
        want,
        "appended items differ from tail -f's human text\n{}",
        t.ctx()
    );
    drop(st);
    t.quit();
}

#[test]
fn ac504_more_than_1000_items_show_the_last_1000_under_one_muted_line() {
    let rb = RpcBox::new();
    let (_, h) = many_items(&rb.bx, 0x504, 1003, short_item);
    let mut t = start(&rb, &["watch", &h], 120, 40);
    wait_pane(&mut t, "item 1003 short line");
    t.focus_to(Focus::Transcript);
    t.key(b"g");
    let marker = format!("── 3 earlier items · glen show {h} ──");
    let m2 = marker.clone();
    t.wait_for("the earlier-items line at the top", UI, move |v| {
        trim_blank(pane_lines(v)).first().map(String::as_str) == Some(m2.as_str())
    });
    let v = t.screen().clone();
    let lines = pane_lines(&v);
    assert!(
        lines.iter().any(|l| l.contains("item 0004 short line")),
        "{}",
        t.ctx()
    );
    assert!(
        !lines.iter().any(|l| l.contains("item 0003 short line")),
        "{}",
        t.ctx()
    );
    let (y, x) = v.find(&marker).unwrap();
    assert_eq!(
        v.grid[y][x].fg,
        token_fg("muted", Depth::X256, Theme::Dark),
        "muted\n{}",
        t.ctx()
    );
    t.quit();
}

#[test]
fn ac504_a_newly_selected_session_shows_within_1s() {
    let rb = RpcBox::new();
    let want = show_lines(&rb, C.handle, pane_width(120, 40), &[]);
    let mut t = start(&rb, &["watch"], 120, 40);
    t.wait_for("G selected", UI, |v| {
        selected_handle(v).as_deref() == Some(G.handle)
    });
    t.settle(Duration::from_millis(300));
    t.focus_to(Focus::Sessions);
    let t0 = Instant::now();
    t.send(b"j");
    let w = want.clone();
    t.wait_for("C's transcript", Duration::from_secs(1) + SLACK, move |v| {
        pane_matches(v, &w)
    });
    eprintln!("selection → transcript: {:?}", t0.elapsed());
    assert!(tab_strip(t.screen()).contains(C.handle), "{}", t.ctx());
    t.quit();
}

// =============================================================================================
// AC-505: following.
// =============================================================================================

#[test]
fn ac505_complete_lines_appear_within_1s_and_a_partial_line_waits() {
    let rb = RpcBox::new();
    let a = rb.bx.path(&A);
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    wait_pane(&mut t, "The test depends on map order. Fixed.");
    append_line(&a, &a_user("10000010", "10000009", "streamed line one"));
    let took = t.time_until("the appended item", Duration::from_secs(1) + SLACK, |v| {
        pane_lines(v)
            .iter()
            .any(|l| l.contains("streamed line one"))
    });
    eprintln!("append → pane: {took:?}");
    let partial = a_user("10000011", "10000010", "partial words here").to_string();
    let mut f = fs::OpenOptions::new().append(true).open(&a).unwrap();
    f.write_all(partial.as_bytes()).unwrap();
    f.flush().unwrap();
    t.never(
        "a partial line was rendered",
        Duration::from_millis(1500),
        |v| {
            pane_lines(v)
                .iter()
                .any(|l| l.contains("partial words here"))
        },
    );
    f.write_all(b"\n").unwrap();
    f.flush().unwrap();
    t.wait_for("the completed line", Duration::from_secs(1) + SLACK, |v| {
        pane_lines(v)
            .iter()
            .any(|l| l.contains("partial words here"))
    });
    t.quit();
}

#[test]
fn ac505_a_rewrite_inserts_the_divider_then_only_unseen_items() {
    let rb = RpcBox::new();
    let a = rb.bx.path(&A);
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    wait_pane(&mut t, "The test depends on map order. Fixed.");
    let mut text = fs::read_to_string(&a).unwrap();
    text.push_str(&jsonl(&[
        a_user("10000010", "10000009", "after the rewrite"),
        reply(
            "10000011",
            Some("10000010"),
            "2026-10-01T09:10:01.000Z",
            "second new",
        ),
    ]));
    let stage = a.with_file_name(format!(
        ".{}.4f2a.tmp",
        a.file_name().unwrap().to_str().unwrap()
    ));
    fs::write(&stage, text).unwrap();
    fs::rename(&stage, &a).unwrap();
    t.wait_for("the rewritten divider and new items", UI, |v| {
        let l = pane_lines(v);
        l.iter().any(|x| x.contains("second new"))
    });
    let lines = pane_lines(t.screen());
    let div = lines
        .iter()
        .position(|l| l.trim() == "── session file rewritten ──")
        .unwrap_or_else(|| panic!("no rewritten divider\n{}", t.ctx()));
    let new = lines
        .iter()
        .position(|l| l.contains("after the rewrite"))
        .unwrap();
    let old = lines
        .iter()
        .position(|l| l.contains("The test depends on map order"))
        .unwrap();
    assert!(
        old < div && div < new,
        "old items, divider, then new items\n{}",
        t.ctx()
    );
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.contains("Why does parser_test flake?"))
            .count(),
        1,
        "seen items are not repeated\n{}",
        t.ctx()
    );
    t.quit();
}

#[test]
fn ac505_a_continuation_is_followed_within_3s_and_moves_the_selection() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    wait_pane(&mut t, "The test depends on map order. Fixed.");
    let new_id = uuid(0x505);
    let h2 = tail7(&new_id);
    let t0 = Instant::now();
    continue_a(
        &rb.bx,
        &new_id,
        &user(
            "10000010",
            Some("10000009"),
            "2026-10-01T10:00:01.000Z",
            "continued here",
        ),
    );
    let div = format!("── continued in {h2} ──");
    let d2 = div.clone();
    t.wait_for(
        "the continued divider",
        Duration::from_secs(3) + SLACK,
        move |v| pane_lines(v).iter().any(|l| l.trim() == d2),
    );
    eprintln!("continuation → divider: {:?}", t0.elapsed());
    t.wait_for("the continuation's new item", UI, |v| {
        pane_lines(v).iter().any(|l| l.contains("continued here"))
    });
    let lines = pane_lines(t.screen());
    let d = lines.iter().position(|l| l.trim() == div).unwrap();
    let n = lines
        .iter()
        .position(|l| l.contains("continued here"))
        .unwrap();
    assert!(d < n, "{}", t.ctx());
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.contains("Why does parser_test flake?"))
            .count(),
        1,
        "only unseen items after the divider\n{}",
        t.ctx()
    );
    let h3 = h2.clone();
    t.wait_for(
        "the selection on the continuation",
        Duration::from_secs(3) + SLACK,
        move |v| selected_handle(v).as_deref() == Some(h3.as_str()) && tab_strip(v).contains(&h3),
    );
    t.quit();
}

#[test]
fn ac505_a_branch_switch_inserts_the_divider_then_the_new_branch() {
    let rb = RpcBox::new();
    let a = rb.bx.path(&A);
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    wait_pane(&mut t, "The test depends on map order. Fixed.");
    // A new leaf whose parent is the first user message: the old leaf leaves the branch.
    append_line(&a, &a_user("10000010", "10000003", "on another branch"));
    t.wait_for("the new branch's item", UI, |v| {
        pane_lines(v)
            .iter()
            .any(|l| l.contains("on another branch"))
    });
    let lines = pane_lines(t.screen());
    let d = lines
        .iter()
        .position(|l| l.trim() == "── switched branch ──")
        .unwrap_or_else(|| panic!("no branch divider\n{}", t.ctx()));
    let n = lines
        .iter()
        .position(|l| l.contains("on another branch"))
        .unwrap();
    assert!(d < n, "{}", t.ctx());
    t.quit();
}

#[test]
fn ac505_a_vanished_file_shows_tail_fs_not_found_in_danger_and_stops() {
    let rb = RpcBox::new();
    let a = rb.bx.path(&A);
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    wait_pane(&mut t, "The test depends on map order. Fixed.");
    // The reference: `tail -f` on the same file, human mode.
    let mut st = rb.bx.follow(&["tail", A.handle, "-f", "--human"]);
    st.wait_for_text("The test depends on map order. Fixed.", T);
    let saved = fs::read(&a).unwrap();
    fs::remove_file(&a).unwrap();
    let status = st.wait(Duration::from_secs(30));
    assert_eq!(status.code(), Some(3), "{}", st.ctx());
    let full = st
        .stderr()
        .lines()
        .find_map(|l| l.strip_prefix("error: ").map(str::to_owned))
        .unwrap_or_else(|| panic!("tail -f printed no error line\n{}", st.ctx()));
    // The prose before the path (the path may be shown as `~/…` or truncated to the pane).
    let msg = full.split(": /").next().unwrap().to_owned();
    let m2 = msg.clone();
    t.wait_for(
        "tail -f's not_found message in the pane",
        Duration::from_secs(5) + SLACK,
        move |v| pane_lines(v).iter().any(|l| l.contains(&m2)),
    );
    let v = t.screen().clone();
    let (y, x) = v.find(&msg).unwrap();
    let danger = token_fg("danger", Depth::X256, Theme::Dark);
    assert!(
        v.cells_of(y, x, &msg)
            .iter()
            .filter(|c| !c.ch.trim().is_empty())
            .all(|c| c.fg == danger),
        "the message is in the danger style\n{}",
        t.ctx()
    );
    // Following stopped: a reappearing file is not read.
    let mut text = String::from_utf8(saved).unwrap();
    text.push_str(&format!(
        "{}\n",
        a_user("10000010", "10000009", "after the stop")
    ));
    fs::write(&a, text).unwrap();
    t.never(
        "following continued after not_found",
        Duration::from_secs(2),
        |v| pane_lines(v).iter().any(|l| l.contains("after the stop")),
    );
    t.quit();
}

// =============================================================================================
// AC-506: scrolling and "N new".
// =============================================================================================

fn indicator(v: &Vt) -> Option<(usize, String)> {
    let (x0, x1) = pane_cols(v);
    let rows: Vec<usize> = body(v).collect();
    rows.iter()
        .rev()
        .find_map(|&y| {
            let t = v.text(y, x0 - 1, x1).trim().to_owned();
            (!t.is_empty()).then_some((y, t))
        })
        .filter(|(_, t)| t.contains("G follows"))
}

#[test]
fn ac506_scrolled_up_new_items_keep_the_viewport_and_count_as_n_new() {
    let rb = RpcBox::new();
    let (id, h) = many_items(&rb.bx, 0x506, 60, short_item);
    let path = rb
        .bx
        .sessions
        .join(ALPHA)
        .join(session_name("2026-10-06T10:00:00.000Z", &id));
    let mut t = start(&rb, &["watch", &h], 120, 40);
    wait_pane(&mut t, "item 0060 short line");
    t.focus_to(Focus::Transcript);
    for _ in 0..5 {
        t.key(b"k");
    }
    t.wait_for("the G follows row", UI, |v| {
        indicator(v).is_some_and(|(_, s)| s == "G follows")
    });
    let v = t.screen().clone();
    let (y, _) = indicator(&v).unwrap();
    let (gy, gx) = v
        .find_all("G follows")
        .into_iter()
        .find(|p| p.0 == y)
        .unwrap();
    assert_eq!(
        v.grid[gy][gx].fg,
        token_fg("muted", Depth::X256, Theme::Dark),
        "muted\n{}",
        t.ctx()
    );
    let mut before = pane_lines(&v);
    before.pop();

    for i in 61..=62 {
        let parent = format!("{:08x}", 0x7000_0000 + i - 1);
        append_line(
            &path,
            &user(
                &format!("{:08x}", 0x7000_0000 + i),
                Some(&parent),
                "2026-10-06T10:00:02.000Z",
                &short_item(i),
            ),
        );
    }
    t.wait_for("↓ 2 new", UI, |v| {
        indicator(v).is_some_and(|(_, s)| s == "↓ 2 new · G follows")
    });
    let v = t.screen().clone();
    let mut after = pane_lines(&v);
    after.pop();
    assert_eq!(
        after,
        before,
        "new items moved a scrolled viewport\n{}",
        t.ctx()
    );
    let (y, _) = indicator(&v).unwrap();
    let (ny, nx) = v
        .find_all("↓ 2 new")
        .into_iter()
        .find(|p| p.0 == y)
        .unwrap();
    assert_eq!(
        v.grid[ny][nx].fg,
        token_fg("focus", Depth::X256, Theme::Dark),
        "focus\n{}",
        t.ctx()
    );

    t.key(b"G");
    t.wait_for("following again", UI, |v| {
        indicator(v).is_none()
            && pane_lines(v)
                .iter()
                .any(|l| l.contains("item 0062 short line"))
    });
    append_line(
        &path,
        &user(
            &format!("{:08x}", 0x7000_0000 + 63),
            Some(&format!("{:08x}", 0x7000_0000 + 62)),
            "2026-10-06T10:00:03.000Z",
            &short_item(63),
        ),
    );
    wait_pane(&mut t, "item 0063 short line");
    // End resumes too.
    t.key(b"k");
    t.wait_for("scrolled", UI, |v| indicator(v).is_some());
    t.key(END);
    t.wait_for("End follows", UI, |v| indicator(v).is_none());
    t.quit();
}

// =============================================================================================
// AC-508, AC-509, AC-510: the list refresh, probing, and writing nothing.
// =============================================================================================

#[test]
fn ac508_a_new_session_appears_within_5s_newest_first() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch"], 120, 40);
    wait_list(&mut t, G.handle);
    let id = uuid(0x508);
    let h = tail7(&id);
    add_session(
        &rb.bx,
        ALPHA,
        &id,
        Some("Fresh session"),
        A.cwd,
        &chain(1, 1, short_item, None),
    );
    let h2 = h.clone();
    let took = t.time_until(
        "the new session at the top",
        Duration::from_secs(5) + SLACK,
        move |v| list_handles(v).first() == Some(&h2),
    );
    eprintln!("new session → list: {took:?}");
    let want: Vec<String> = ls_rows(&rb, &rb.bx.alpha, &[])
        .into_iter()
        .map(|r| r.0)
        .collect();
    t.wait_for("ls order", UI, move |v| list_handles(v) == want);
    t.quit();
}

#[test]
fn ac508_the_selection_follows_the_session_id_across_a_reorder() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch"], 120, 40);
    t.wait_for("G selected", UI, |v| {
        selected_handle(v).as_deref() == Some(G.handle)
    });
    t.focus_to(Focus::Sessions);
    t.key(b"j");
    t.wait_for("C selected", UI, |v| {
        selected_handle(v).as_deref() == Some(C.handle)
    });
    // A becomes the newest row; C moves down.
    set_mtime(&rb.bx.path(&A), SystemTime::now() + Duration::from_secs(1));
    t.key(b"r");
    t.wait_for("A on top", Duration::from_secs(5) + SLACK, |v| {
        list_handles(v).first().map(String::as_str) == Some(A.handle)
    });
    let v = t.screen().clone();
    assert_eq!(
        selected_handle(&v).as_deref(),
        Some(C.handle),
        "{}",
        t.ctx()
    );
    assert!(tab_strip(&v).contains(C.handle), "{}", t.ctx());
    t.quit();
}

#[test]
fn ac509_an_extension_row_shows_the_endpoint_state_like_ls() {
    let lb = LiveBox::new();
    let _ext = lb.start_ext(A.id, "busy", &[], &[]);
    let d = lb.rb.bx.data(["ls", "--json"]);
    let row = row(&d, A.id);
    assert_eq!(row["state"], "busy");
    assert_eq!(row["state_source"], "extension");
    let mut t = start(&lb.rb, &["watch", A.handle], 120, 40);
    t.wait_for("A busy", UI, |v| {
        list_row_of(v, A.handle).is_some_and(|(y, _)| list_entry(v, y).unwrap().1 == '◐')
    });
    assert!(tab_strip(t.screen()).contains("◐ busy"), "{}", t.ctx());
    t.quit();
}

#[test]
fn ac510_watching_writes_no_omp_file_and_no_inbox_cursor() {
    let rb = RpcBox::new();
    append_record(
        &inbox_path(&rb, A.id),
        &record(A.id, "already here", false, jiff::Timestamp::now()),
    );
    let omp_before = snapshot(&rb.bx.agent);
    let home_before = snapshot(&rb.bx.glen_home);
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    t.wait_for("the reply", UI, |v| strip_find(v, "already here").is_some());
    t.focus_to(Focus::Sessions);
    t.key(b"i");
    t.wait_text("peek only · glen inbox marks these read", UI);
    t.key(ESC);
    for k in [b"k", b"k", b"j"] {
        t.key(k);
    }
    std::thread::sleep(Duration::from_secs(3));
    t.quit();
    assert_eq!(snapshot(&rb.bx.agent), omp_before, "an omp file changed");
    assert_eq!(snapshot(&rb.bx.glen_home), home_before, "GLEN_HOME changed");
    assert!(!rb.bx.glen_home.join("inbox/.read").exists());
    assert!(
        !rb.bx.home.join(".omp").exists(),
        "{:?}",
        list_tree(&rb.bx.home.join(".omp"))
    );
}

// =============================================================================================
// AC-511, AC-512: sending and refusals.
// =============================================================================================

#[test]
fn ac511_enter_submits_through_the_rpc_route() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    t.focus_to(Focus::Prompt);
    t.typ("hello watch");
    t.key(ENTER);
    let id = rb.wait_one_job();
    let job = job_of(&rb, &id);
    assert_eq!(job["route"], "rpc", "{job}");
    assert_eq!(job["deliver_as"], "auto", "{job}");
    assert_eq!(job["text"], "hello watch", "{job}");
    assert_eq!(job["session_id"], A.id, "{job}");
    let jh = job_handle(&id);
    let done = format!("✓ completed · {jh}");
    t.wait_for("✓ completed and the reply", LONG, move |v| {
        strip_find(v, &done).is_some() && strip_find(v, "reply: hello watch").is_some()
    });
    assert!(
        !input_text(t.screen()).contains("hello watch"),
        "the prompt is cleared\n{}",
        t.ctx()
    );
    rb.wait_terminal(&id);
    t.quit();
    rb.assert_quiet();
}

#[test]
fn ac511_routing_shows_sending_locks_editing_and_keeps_keys_live() {
    if !cfg!(debug_assertions) {
        return; // the probe gate exists only in debug builds
    }
    let rb = RpcBox::new();
    let gate = rb.bx.root.join("gate");
    let mut t = start_env(
        &rb,
        &["watch", A.handle],
        &[("FAKE_OMP_LEASE_PROBE_GATE", &s(&gate))],
        120,
        40,
    );
    t.focus_to(Focus::Prompt);
    t.typ("held");
    t.key(ENTER);
    wait_file_exists(&ready_path(&gate), Duration::from_secs(5));
    t.wait_for("sending… on the prompt rule", UI, |v| {
        mode_label(v).contains("sending…")
    });
    t.typ("zz");
    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(
        input_text(t.screen()),
        "held",
        "editing is locked while routing\n{}",
        t.ctx()
    );
    let t0 = Instant::now();
    t.send(TAB);
    t.wait_for("Tab moves focus while routing", UI, |v| {
        focus(v) != Some(Focus::Prompt)
    });
    let took = t0.elapsed();
    eprintln!("Tab while routing: {took:?}");
    assert!(
        took <= Duration::from_millis(50) + RESTORE_SLACK,
        "key handling blocked: {took:?}"
    );
    fs::write(&gate, "").unwrap();
    let id = rb.wait_one_job();
    assert_eq!(job_of(&rb, &id)["text"], "held");
    t.wait_for("the prompt unlocked and cleared", UI, |v| {
        !mode_label(v).contains("sending…") && !input_text(v).contains("held")
    });
    rb.wait_terminal(&id);
    t.quit();
    rb.assert_quiet();
}

#[test]
fn ac511_a_live_endpoint_takes_the_message() {
    let lb = LiveBox::new();
    let _ext = lb.start_ext(A.id, "ok", &[], &[]);
    let mut t = start(&lb.rb, &["watch", A.handle], 120, 40);
    t.focus_to(Focus::Prompt);
    t.typ("via live");
    t.key(ENTER);
    let id = lb.wait_one_job();
    assert_eq!(job_of(&lb.rb, &id)["route"], "live");
    t.wait_for("the live reply", LONG, |v| {
        strip_find(v, "reply: via live").is_some()
    });
    lb.wait_terminal(&id);
    lb.assert_no_omp();
    t.quit();
    lb.assert_quiet();
}

/// The notice rows under the rule, from the `✗` row on (trimmed on the right, empty rows dropped).
fn notice(v: &Vt) -> Option<Vec<String>> {
    let rows = notice_rows(v);
    let first = rows.iter().position(|r| r.starts_with("✗ "))?;
    Some(
        rows[first..]
            .iter()
            .filter(|r| !r.trim().is_empty())
            .cloned()
            .collect(),
    )
}

/// Split notice rows into (message, hint) when a hint is expected: the hint is the last row.
fn split_notice(rows: &[String], with_hint: bool) -> (String, Option<String>) {
    let (msg_rows, hint) = if with_hint && rows.len() >= 2 {
        (&rows[..rows.len() - 1], Some(rows[rows.len() - 1].clone()))
    } else {
        (rows, None)
    };
    let msg = msg_rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            if i == 0 {
                r["✗ ".len()..].trim().to_owned()
            } else {
                r.trim().to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    (msg, hint)
}

fn words(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn ac512_a_held_lease_is_refused_inline_with_the_cli_message_and_hint() {
    let rb = RpcBox::new();
    let home = fs::canonicalize(&rb.bx.home).unwrap();
    let _held = hold_lease(&home.join(".omp/run/session-owners"), A.id);
    let cli = rb
        .glen(["send", A.handle, "hi", "--json"])
        .env("HOME", &home)
        .run();
    let err = failed(&cli, "no_route");
    let msg = err["message"].as_str().unwrap().to_owned();
    let hint = err["hint"].as_str().map(str::to_owned);
    assert!(rb.job_ids().is_empty());

    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    t.focus_to(Focus::Prompt);
    t.typ("hi");
    let t0 = Instant::now();
    t.send(ENTER);
    t.wait_for("the inline refusal", Duration::from_secs(3) + SLACK, |v| {
        notice(v).is_some()
    });
    eprintln!("Enter → refusal: {:?}", t0.elapsed());
    let v = t.screen().clone();
    let rows = notice(&v).unwrap();
    let (m, h) = split_notice(&rows, hint.is_some());
    assert!(
        words(&m) == words(&msg)
            || m.ends_with('…') && words(&msg).starts_with(words(m.trim_end_matches('…')).as_str()),
        "notice {m:?} is not the CLI message {msg:?}\n{}",
        t.ctx()
    );
    if let Some(hint) = &hint {
        let h = h.unwrap_or_else(|| panic!("no hint row\n{}", t.ctx()));
        assert!(
            h.starts_with("  ") && !h.starts_with("   "),
            "hint indented two columns: {h:?}\n{}",
            t.ctx()
        );
        assert!(fits(&h, hint), "hint row {h:?} vs {hint:?}\n{}", t.ctx());
        let (hy, hx) = v.find(h.trim()).unwrap();
        assert_eq!(
            v.grid[hy][hx].fg,
            token_fg("muted", Depth::X256, Theme::Dark),
            "muted hint\n{}",
            t.ctx()
        );
    }
    let r = rule_row(&v).unwrap();
    let (y, x) = v.find_all("✗ ").into_iter().find(|(y, _)| *y > r).unwrap();
    assert_eq!(
        v.grid[y][x].fg,
        token_fg("danger", Depth::X256, Theme::Dark),
        "danger ✗\n{}",
        t.ctx()
    );
    assert_eq!(input_text(&v), "hi", "the typed text stays\n{}", t.ctx());
    assert!(rb.job_ids().is_empty(), "a refusal creates no job");
    t.key(ESC);
    t.wait_for("Esc dismisses the notice", UI, |v| notice(v).is_none());
    assert_eq!(
        focus(t.screen()),
        Some(Focus::Prompt),
        "the first Esc only dismisses\n{}",
        t.ctx()
    );
    t.key(ESC);
    t.wait_for("the second Esc focuses the transcript", UI, |v| {
        focus(v) == Some(Focus::Transcript)
    });
    t.focus_to(Focus::Prompt);
    t.typ("!");
    t.wait_for("the prompt is editable again", UI, |v| {
        input_text(v) == "hi!"
    });
    t.quit();
}

#[test]
fn ac512_a_refusal_returns_the_text_to_the_draft_it_was_sent_from() {
    if !cfg!(debug_assertions) {
        return;
    }
    let rb = RpcBox::new();
    let home = fs::canonicalize(&rb.bx.home).unwrap();
    let _held = hold_lease(&home.join(".omp/run/session-owners"), A.id);
    let gate = rb.bx.root.join("gate");
    let mut t = start_env(
        &rb,
        &["watch", A.handle],
        &[("FAKE_OMP_LEASE_PROBE_GATE", &s(&gate))],
        120,
        40,
    );
    t.focus_to(Focus::Prompt);
    t.typ("for A");
    t.key(ENTER);
    wait_file_exists(&ready_path(&gate), Duration::from_secs(5));
    t.focus_to(Focus::Sessions);
    t.key(b"g");
    t.wait_for("G selected", UI, |v| {
        selected_handle(v).as_deref() == Some(G.handle)
    });
    fs::write(&gate, "").unwrap();
    std::thread::sleep(Duration::from_secs(3));
    let v = t.screen().clone();
    assert!(
        !input_text(&v).contains("for A"),
        "G's draft got A's text\n{}",
        t.ctx()
    );
    assert!(notice(&v).is_none(), "G shows A's refusal\n{}", t.ctx());
    t.key(b"G");
    t.wait_for("A's draft holds the refused text", UI, |v| {
        selected_handle(v).as_deref() == Some(A.handle) && input_text(v) == "for A"
    });
    t.wait_for("A's draft shows the refusal", UI, |v| notice(v).is_some());
    assert!(rb.job_ids().is_empty());
    t.quit();
}

// =============================================================================================
// AC-513: job activity.
// =============================================================================================

#[test]
fn ac513_a_running_job_shows_spinner_progress_and_elapsed_then_completed() {
    let rb = RpcBox::new();
    let release = rb.bx.root.join("release");
    let mut t = start_env(
        &rb,
        &["watch", A.handle],
        &[("FAKE_OMP_RUN_RELEASE", &s(&release))],
        120,
        40,
    );
    t.focus_to(Focus::Prompt);
    t.typ("slow one");
    t.key(ENTER);
    let id = rb.wait_one_job();
    let jh = job_handle(&id);
    rb.wait_status(&id, "accepted", LONG, |s| s["status"] == "accepted");
    let row = format!("{jh} · waiting for the reply · ");
    let r2 = row.clone();
    t.wait_for(
        "the running row",
        Duration::from_millis(500) + SLACK,
        move |v| strip_lines(v).iter().any(|l| l.contains(&r2)),
    );
    let v = t.screen().clone();
    let line = strip_lines(&v)
        .into_iter()
        .find(|l| l.contains(&row))
        .unwrap();
    let first = line.chars().next().unwrap();
    assert!(
        SPINNER.contains(first),
        "spinner glyph first: {line:?}\n{}",
        t.ctx()
    );
    let elapsed = line.split(&row).nth(1).unwrap().trim();
    assert!(
        !elapsed.is_empty() && elapsed.chars().any(|c| c.is_ascii_digit()),
        "elapsed: {line:?}"
    );
    fs::write(&release, "").unwrap();
    // Status → strip within 500 ms.
    let done = format!("✓ completed · {jh}");
    let deadline = Instant::now() + LONG;
    let mut t_status = None;
    loop {
        if t_status.is_none()
            && rb
                .try_status(&id)
                .is_some_and(|s| s["status"] == "completed")
        {
            t_status = Some(Instant::now());
        }
        if strip_find(t.screen(), &done).is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "no ✓ completed\n{}", t.ctx());
        std::thread::sleep(Duration::from_millis(5));
    }
    let lag = t_status.map(|s| s.elapsed()).unwrap_or_default();
    eprintln!("status.json → strip: {lag:?}");
    assert!(
        lag <= Duration::from_millis(500) + SLACK,
        "strip lagged {lag:?}"
    );
    rb.wait_terminal(&id);
    t.quit();
    rb.assert_quiet();
}

#[test]
fn ac513_a_final_reply_does_not_end_the_row() {
    let rb = RpcBox::new();
    let release = rb.bx.root.join("release");
    let mut t = start_env(
        &rb,
        &["watch", A.handle],
        &[("FAKE_OMP_RESULT_RELEASE", &s(&release))],
        120,
        40,
    );
    t.focus_to(Focus::Prompt);
    t.typ("fin");
    t.key(ENTER);
    let id = rb.wait_one_job();
    let jh = job_handle(&id);
    t.wait_for("the final reply", LONG, |v| {
        strip_find(v, "↩ reply: fin ✓").is_some()
    });
    assert_eq!(
        rb.status(&id)["status"],
        "accepted",
        "the fake holds before its result"
    );
    let j2 = jh.clone();
    t.never(
        "a final reply ended the job's row",
        Duration::from_millis(1500),
        move |v| {
            strip_find(v, "✓ completed").is_some()
                || !strip_lines(v)
                    .iter()
                    .any(|l| l.contains(&format!("{j2} · ")))
        },
    );
    fs::write(&release, "").unwrap();
    let done = format!("✓ completed · {jh}");
    t.wait_for("✓ completed after the terminal status", LONG, move |v| {
        strip_find(v, &done).is_some()
    });
    rb.wait_terminal(&id);
    t.quit();
    rb.assert_quiet();
}

#[test]
fn ac513_a_failed_job_shows_glen_waits_message_and_hint() {
    let rb = RpcBox::new();
    let mut t = start_env(
        &rb,
        &["watch", A.handle],
        &[("FAKE_OMP_SCENARIO", "error")],
        120,
        40,
    );
    t.focus_to(Focus::Prompt);
    t.typ("boom");
    t.key(ENTER);
    let id = rb.wait_one_job();
    let st = rb.wait_terminal(&id);
    assert_eq!(st["status"], "failed");
    let w = rb.glen(["wait", &job_handle(&id), "--json"]).run();
    let err = failed(&w, "job_failed");
    let msg = err["message"].as_str().unwrap().to_owned();
    let m2 = msg.clone();
    t.wait_for("✗ <wait's message>", UI, move |v| {
        strip_lines(v)
            .iter()
            .any(|l| l.strip_prefix("✗ ").is_some_and(|r| fits(r, &m2)))
    });
    let v = t.screen().clone();
    let (y, x) = strip_find(&v, "✗ ").unwrap();
    assert_eq!(
        v.grid[y][x].fg,
        token_fg("danger", Depth::X256, Theme::Dark),
        "{}",
        t.ctx()
    );
    if let Some(hint) = err["hint"].as_str() {
        assert!(
            strip_lines(&v).iter().any(|l| fits(l, hint)),
            "the hint row {hint:?}\n{}",
            t.ctx()
        );
    }
    t.quit();
    rb.assert_quiet();
}

#[test]
fn ac513_a_forked_job_shows_the_new_handle() {
    let rb = RpcBox::new();
    let mut t = start_env(
        &rb,
        &["watch", A.handle],
        &[("FAKE_OMP_SCENARIO", "fork")],
        120,
        40,
    );
    t.focus_to(Focus::Prompt);
    t.typ("fork me");
    t.key(ENTER);
    let id = rb.wait_one_job();
    rb.wait_terminal(&id);
    let want = format!("✓ forked → {}", tail7(&moved_id(A.id)));
    t.wait_for(&want.clone(), UI, move |v| strip_find(v, &want).is_some());
    t.quit();
    rb.assert_quiet();
}

#[test]
fn ac513_another_callers_job_appears_within_1_5s() {
    let rb = RpcBox::new();
    let release = rb.bx.root.join("release");
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    let out = rb
        .glen(["send", A.handle, "from an agent", "--no-wait", "--json"])
        .env("FAKE_OMP_RUN_RELEASE", &release)
        .run();
    let d = data_of(&out, 10);
    let id = job_id(&d);
    let jh = job_handle(&id);
    let took = t.time_until(
        "the agent's job in the strip",
        Duration::from_millis(1500) + SLACK,
        move |v| strip_lines(v).iter().any(|l| l.contains(&jh)),
    );
    eprintln!("other caller's job → strip: {took:?}");
    fs::write(&release, "").unwrap();
    rb.wait_terminal(&id);
    t.quit();
    rb.assert_quiet();
}

// =============================================================================================
// AC-514: prompt modes, limits and drafts.
// =============================================================================================

#[test]
fn ac514_ctrl_t_cycles_the_mode_and_a_submit_resets_it() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    t.focus_to(Focus::Prompt);
    assert_eq!(mode_label(t.screen()), "auto", "{}", t.ctx());
    for want in ["steer", "follow-up", "auto", "steer"] {
        t.key(CTRL_T);
        t.wait_for(want, UI, move |v| mode_label(v) == want);
    }
    for (text, presses, deliver) in [
        ("steered", 0, "steer"),
        ("followed", 2, "follow_up"),
        ("plain", 0, "auto"),
    ] {
        for _ in 0..presses {
            t.key(CTRL_T);
        }
        let before = job_ids_now(&rb);
        t.typ(text);
        t.key(ENTER);
        let id = new_job(&rb, &before);
        assert_eq!(job_of(&rb, &id)["deliver_as"], deliver, "{text}");
        t.wait_for("the mode back to auto", UI, |v| mode_label(v) == "auto");
        rb.wait_terminal(&id);
    }
    t.quit();
    rb.assert_quiet();
}

#[test]
fn ac514_empty_whitespace_and_oversized_prompts_submit_nothing() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    t.focus_to(Focus::Prompt);
    t.key(ENTER);
    t.typ("   ");
    t.key(ENTER);
    t.paste(&vec![b'a'; 524_289]);
    t.key(ENTER);
    t.wait_for("the size notice", UI, |v| {
        notice_rows(v)
            .iter()
            .any(|r| r.trim() == "✗ message is larger than 512 KiB")
    });
    std::thread::sleep(Duration::from_millis(500));
    assert!(rb.job_ids().is_empty(), "a job was created");
    assert!(!rb.bx.glen_home.exists(), "GLEN_HOME was created");
    t.key(CTRL_C);
    t.wait_for("the prompt cleared", UI, |v| {
        !input_text(v).contains("aaaa")
    });
    t.paste(&vec![b'b'; 524_288]);
    t.key(ENTER);
    let id = rb.wait_one_job();
    assert_eq!(job_of(&rb, &id)["text"].as_str().unwrap().len(), 524_288);
    rb.wait_terminal(&id);
    t.quit();
    rb.assert_quiet();
}

#[test]
fn ac514_drafts_stay_with_their_session() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    t.focus_to(Focus::Prompt);
    t.typ("draft for A");
    t.key(CTRL_T);
    t.key(LEFT);
    t.key(LEFT);
    t.settle(Duration::from_millis(150));
    let cursor_a = (t.screen().cx, t.screen().cy);
    t.focus_to(Focus::Sessions);
    t.key(b"g");
    t.wait_for("G selected", UI, |v| {
        selected_handle(v).as_deref() == Some(G.handle)
    });
    t.key(ENTER);
    t.wait_for("G's prompt focused", UI, |v| {
        focus(v) == Some(Focus::Prompt)
    });
    let v = t.screen().clone();
    assert!(!input_text(&v).contains("draft for A"), "{}", t.ctx());
    assert_eq!(mode_label(&v), "auto", "G's mode\n{}", t.ctx());
    t.typ("draft for G");
    t.focus_to(Focus::Sessions);
    t.key(b"G");
    t.wait_for("A selected", UI, |v| {
        selected_handle(v).as_deref() == Some(A.handle)
    });
    t.key(ENTER);
    t.wait_for("A's draft back", UI, |v| {
        focus(v) == Some(Focus::Prompt) && input_text(v) == "draft for A"
    });
    t.settle(Duration::from_millis(150));
    let v = t.screen().clone();
    assert_eq!(mode_label(&v), "steer", "A's mode kept\n{}", t.ctx());
    assert_eq!((v.cx, v.cy), cursor_a, "A's cursor kept\n{}", t.ctx());
    t.focus_to(Focus::Sessions);
    t.key(b"g");
    t.key(ENTER);
    t.wait_for("G's draft back", UI, |v| input_text(v) == "draft for G");
    t.quit();
}

#[test]
fn ac514_a_lone_dash_is_sent_as_the_text_dash() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    t.focus_to(Focus::Prompt);
    t.typ("-");
    t.key(ENTER);
    let id = rb.wait_one_job();
    assert_eq!(job_of(&rb, &id)["text"], "-");
    rb.wait_terminal(&id);
    t.quit();
    rb.assert_quiet();
}

#[test]
fn ac514_a_key_repeat_enter_submits_nothing() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    t.focus_to(Focus::Prompt);
    t.typ("x");
    t.send(b"\x1b[13;1:2u"); // Enter, kind Repeat (kitty keyboard protocol)
    t.send(b"\x1b[13;1:3u"); // Enter, kind Release
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        rb.job_ids().is_empty(),
        "a repeat or release Enter submitted"
    );
    assert_eq!(input_text(t.screen()), "x", "{}", t.ctx());
    t.key(ENTER);
    let id = rb.wait_one_job();
    assert_eq!(job_of(&rb, &id)["text"], "x");
    rb.wait_terminal(&id);
    t.quit();
    rb.assert_quiet();
}

// =============================================================================================
// AC-515: talk-back.
// =============================================================================================

fn overlay_rows(v: &Vt) -> Vec<String> {
    let (x0, x1) = pane_cols(v);
    body(v)
        .map(|y| v.text(y, x0 - 1, x1).trim_end().to_owned())
        .collect()
}

#[test]
fn ac515_an_inbox_record_shows_in_the_strip_badges_and_overlay_and_reads_clear_it() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    let now = jiff::Timestamp::now();
    append_record(
        &inbox_path(&rb, A.id),
        &record(A.id, "hello from omp", false, now),
    );
    let took = t.time_until("↩ hello from omp", Duration::from_secs(1) + SLACK, |v| {
        strip_find(v, "↩ hello from omp").is_some()
    });
    eprintln!("inbox append → strip: {took:?}");
    t.wait_for("unread badges", UI, |v| {
        title_bar(v).contains("↩ 1")
            && list_row_of(v, A.handle).is_some_and(|(_, r)| r.contains("↩1"))
    });
    append_record(
        &inbox_path(&rb, A.id),
        &record(
            A.id,
            "all done",
            true,
            now + jiff::SignedDuration::from_secs(1),
        ),
    );
    t.wait_for(
        "the final reply with ✓",
        Duration::from_secs(1) + SLACK,
        |v| strip_find(v, "↩ all done ✓").is_some() && title_bar(v).contains("↩ 2"),
    );
    t.focus_to(Focus::Sessions);
    t.key(b"i");
    t.wait_text("peek only · glen inbox marks these read", UI);
    let rows = overlay_rows(t.screen());
    let heads: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.contains(&format!("↩ {} · ", A.handle)))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(heads.len(), 2, "two records\n{}", t.ctx());
    for &i in &heads {
        assert!(
            rows[i].trim_start().starts_with('●'),
            "unread marker\n{}",
            t.ctx()
        );
        assert!(rows[i].contains("· tui"), "no job: tui\n{}", t.ctx());
    }
    assert!(
        rows[heads[1]].ends_with(" · ✓"),
        "final marker\n{}",
        t.ctx()
    );
    assert_eq!(
        rows[heads[0] + 1].trim_start(),
        "hello from omp",
        "{}",
        t.ctx()
    );
    assert!(
        rows[heads[0] + 1].starts_with("   ") || rows[heads[0] + 1].starts_with("  "),
        "indented\n{}",
        t.ctx()
    );
    assert_eq!(
        rows[heads[1] + 1].trim_start(),
        "all done",
        "chronological\n{}",
        t.ctx()
    );
    t.key(ESC);
    assert!(
        !rb.bx.glen_home.join("inbox/.read").exists(),
        "watch marked the inbox read"
    );
    let out = rb.glen(["inbox", "--json"]).run();
    out.ok_data();
    let took = t.time_until(
        "the unread count cleared",
        Duration::from_secs(2) + SLACK,
        |v| {
            !title_bar(v).contains('↩')
                && list_row_of(v, A.handle).is_some_and(|(_, r)| !r.contains('↩'))
        },
    );
    eprintln!("glen inbox → badges cleared: {took:?}");
    t.quit();
}

#[test]
fn ac515_a_reply_in_both_the_job_and_the_inbox_shows_once() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    t.focus_to(Focus::Prompt);
    t.typ("dup check");
    t.key(ENTER);
    let id = rb.wait_one_job();
    rb.wait_terminal(&id);
    let job_replies = ndjson(&rb.job_dir(&id).join("replies.ndjson"));
    let inbox = ndjson(&inbox_path(&rb, A.id));
    assert!(
        job_replies
            .iter()
            .any(|r| inbox.iter().any(|i| i["id"] == r["id"])),
        "precondition: the reply is in both places"
    );
    t.wait_for("the reply", UI, |v| {
        strip_find(v, "reply: dup check").is_some()
    });
    std::thread::sleep(Duration::from_millis(1500));
    let n = strip_lines(t.screen())
        .iter()
        .filter(|l| l.contains("reply: dup check"))
        .count();
    assert_eq!(n, 1, "the reply shows once\n{}", t.ctx());
    t.quit();
    rb.assert_quiet();
}

#[test]
fn ac515_the_overlay_lists_every_session_chronologically() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    let t0 = jiff::Timestamp::now() - jiff::SignedDuration::from_secs(60);
    append_record(
        &inbox_path(&rb, A.id),
        &record(
            A.id,
            "third",
            false,
            t0 + jiff::SignedDuration::from_secs(20),
        ),
    );
    append_record(
        &inbox_path(&rb, G.id),
        &record(
            G.id,
            "second",
            false,
            t0 + jiff::SignedDuration::from_secs(10),
        ),
    );
    append_record(&inbox_path(&rb, B.id), &record(B.id, "first", false, t0));
    t.wait_for("A's reply", Duration::from_secs(1) + SLACK, |v| {
        strip_find(v, "↩ third").is_some()
    });
    t.focus_to(Focus::Sessions);
    t.key(b"i");
    t.wait_text("peek only · glen inbox marks these read", UI);
    let rows = overlay_rows(t.screen());
    let order: Vec<&str> = rows
        .iter()
        .filter_map(|r| {
            [B.handle, G.handle, A.handle]
                .into_iter()
                .find(|h| r.contains(&format!("↩ {h} · ")))
        })
        .collect();
    assert_eq!(order, [B.handle, G.handle, A.handle], "{}", t.ctx());
    t.quit();
}

// =============================================================================================
// AC-516: session controls are not in this build.
// =============================================================================================

// W3b merged (44c9d5b): controls available; migrated at the W3b→W5 merge per W5 decision 4
#[test]
fn ac516_control_keys_dispatch_the_matching_control() {
    let lb = LiveBox::new();
    let ext = lb.start_ext(A.id, "ok", &[], &[]);
    let mut t = start(&lb.rb, &["watch", A.handle], 120, 40);
    t.key(CTRL_X);
    t.wait_for("the abort confirmation", UI, |v| {
        status_bar(v).contains("y stop  n cancel")
    });
    t.key(b"y");
    t.wait_for("abort reached the endpoint", UI, |_| {
        ext.frames("in").iter().any(|f| f["op"] == "abort")
    });
    t.wait_for("abort completed", UI, |v| !v.contains("y stop"));

    t.key(b"m");
    t.wait_for("the model picker", UI, |v| v.contains("model · filter:"));
    t.typ("other/fake-2");
    t.key(ENTER);
    t.wait_for("the chosen model reached the endpoint", UI, |_| {
        ext.frames("in").iter().any(|f| {
            f["op"] == "set_model"
                && f["args"]["provider"] == "other"
                && f["args"]["model_id"] == "fake-2"
        })
    });
    t.wait_for("model change completed", UI, |v| {
        status_bar(v).contains("thinking")
    });

    t.key(b"T");
    t.wait_for("the thinking picker", UI, |v| {
        v.contains("thinking level · filter:")
    });
    t.typ("high");
    t.key(ENTER);
    t.wait_for("the chosen thinking level reached the endpoint", UI, |_| {
        ext.frames("in")
            .iter()
            .any(|f| f["op"] == "set_thinking_level" && f["args"]["level"] == "high")
    });
    t.wait_for("thinking change completed", UI, |v| {
        status_bar(v).contains("thinking high")
    });

    t.key(b"C");
    t.wait_for("the compact confirmation", UI, |v| {
        status_bar(v).contains("y compact  n cancel")
    });
    t.key(b"y");
    t.wait_for("compact reached the endpoint", UI, |_| {
        ext.frames("in").iter().any(|f| f["op"] == "compact")
    });
    t.wait_for("compact completed", UI, |v| {
        status_bar(v).contains("compacted")
    });
    assert!(!status_bar(t.screen()).contains(NOT_IN_BUILD));
    t.focus_to(Focus::Prompt);
    t.key(CTRL_X);
    t.wait_for("abort is available from the prompt", UI, |v| {
        status_bar(v).contains("y stop  n cancel")
    });
    t.key(b"n");
    let control_ops = || {
        ext.ops_in()
            .into_iter()
            .filter(|op| !matches!(op.as_str(), "hello" | "state"))
            .collect::<Vec<_>>()
    };
    let before_typing = control_ops();
    t.typ("mTC");
    t.wait_for("m T C type in the prompt", UI, |v| input_text(v) == "mTC");
    t.settle(Duration::from_millis(500));
    assert_eq!(
        control_ops(),
        before_typing,
        "typing mTC in the prompt must start no control"
    );
    assert!(lb.job_ids().is_empty());
    lb.assert_no_omp();
    t.quit();
}

// =============================================================================================
// The help overlay (AC-516, AC-521).
// =============================================================================================

/// The key column of the overlay rows: the text before the first gap of two or more spaces.
fn help_keys(v: &Vt) -> Vec<(String, String)> {
    let (x0, x1) = pane_cols(v);
    body(v)
        .filter_map(|y| {
            let t = v.text(y, x0 - 1, x1);
            let t = t.trim();
            let gap = t.find("  ")?;
            Some((t[..gap].trim().to_owned(), t.to_owned()))
        })
        .collect()
}

fn tokens(key: &str) -> Vec<String> {
    key.split(|c: char| c.is_whitespace() || matches!(c, '/' | ',' | '|'))
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The Design spec keymap, each binding with the spellings a help row may use.
fn keymap() -> Vec<(&'static str, Vec<String>)> {
    let ctrl = |c: char| {
        ["^", "ctrl-", "ctrl+", "c-", "⌃", "control-"]
            .iter()
            .map(|p| format!("{p}{c}"))
            .collect::<Vec<_>>()
    };
    let alts = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let exact = |k: &str| vec![format!("={k}")];
    vec![
        ("Tab", alts(&["tab", "⇥"])),
        (
            "Shift-Tab",
            alts(&[
                "shift-tab",
                "s-tab",
                "shift+tab",
                "⇧⇥",
                "⇧tab",
                "backtab",
                "⇤",
            ]),
        ),
        ("↑", alts(&["↑", "up"])),
        ("↓", alts(&["↓", "down"])),
        ("k", exact("k")),
        ("j", exact("j")),
        ("PgUp", alts(&["pgup", "pageup", "pg-up", "⇞"])),
        ("PgDn", alts(&["pgdn", "pagedown", "pgdown", "pg-dn", "⇟"])),
        ("Home", alts(&["home"])),
        ("End", alts(&["end"])),
        ("g", exact("g")),
        ("G", exact("G")),
        ("Enter", alts(&["enter", "⏎", "return", "↵"])),
        ("Ctrl-U", ctrl('u')),
        ("Ctrl-D", ctrl('d')),
        ("t", exact("t")),
        ("o", exact("o")),
        ("a", exact("a")),
        ("r", exact("r")),
        ("i", exact("i")),
        ("?", exact("?")),
        ("F1", alts(&["f1"])),
        ("q", exact("q")),
        (
            "Alt-Enter",
            alts(&[
                "alt-enter",
                "alt+enter",
                "⌥⏎",
                "⌥enter",
                "⌥↵",
                "m-enter",
                "meta-enter",
                "alt-⏎",
                "alt-return",
            ]),
        ),
        ("Ctrl-J", ctrl('j')),
        ("Ctrl-T", ctrl('t')),
        ("←", alts(&["←", "left"])),
        ("→", alts(&["→", "right"])),
        ("Ctrl-A", ctrl('a')),
        ("Ctrl-E", ctrl('e')),
        ("Backspace", alts(&["backspace", "bksp", "⌫", "bs"])),
        ("Delete", alts(&["delete", "del", "⌦"])),
        ("Ctrl-W", ctrl('w')),
        ("Ctrl-K", ctrl('k')),
        ("Esc", alts(&["esc", "escape", "⎋"])),
        ("Ctrl-C", ctrl('c')),
        ("Ctrl-X", ctrl('x')),
        ("m", exact("m")),
        ("T", exact("T")),
        ("C", exact("C")),
    ]
}

fn has_binding(keys: &[String], spellings: &[String]) -> bool {
    keys.iter().flat_map(|k| tokens(k)).any(|tok| {
        spellings.iter().any(|sp| match sp.strip_prefix('=') {
            Some(exact) => tok == exact,
            None => tok.to_lowercase() == *sp,
        })
    })
}

fn open_help(t: &mut Tui, f: Focus) {
    t.focus_to(f);
    t.key(if f == Focus::Prompt { F1 } else { b"?" });
    t.wait_for("the help overlay", UI, |v| {
        !help_keys(v).is_empty() && v.contains("help")
    });
    t.settle(Duration::from_millis(100));
}

// W3b merged (44c9d5b): controls available; migrated at the W3b→W5 merge per W5 decision 4
#[test]
fn ac516_the_help_overlay_lists_the_control_bindings_as_available() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 140, 70);
    open_help(&mut t, Focus::Sessions);
    let rows = help_keys(t.screen());
    for (name, sp) in keymap()
        .into_iter()
        .filter(|(n, _)| ["Ctrl-X", "m", "T", "C"].contains(n))
    {
        let label = match name {
            "Ctrl-X" => "stop running turn",
            "m" => "choose model",
            "T" => "choose thinking level",
            "C" => "compact session",
            _ => unreachable!(),
        };
        assert!(
            rows.iter().any(|(k, row)| {
                has_binding(std::slice::from_ref(k), &sp) && row.contains(label)
            }),
            "{name} is not listed as available\n{}",
            t.ctx()
        );
    }
    assert!(!t.screen().contains("(not in this build)"), "{}", t.ctx());
    t.key(ESC);
    t.quit();
}

#[test]
fn ac521_the_help_overlay_lists_every_binding_and_esc_closes_it() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 140, 70);
    let mut missing = Vec::new();
    for f in [Focus::Sessions, Focus::Transcript, Focus::Prompt] {
        open_help(&mut t, f);
        let keys: Vec<String> = help_keys(t.screen()).into_iter().map(|r| r.0).collect();
        let gone: Vec<&str> = keymap()
            .into_iter()
            .filter(|(_, sp)| !has_binding(&keys, sp))
            .map(|(n, _)| n)
            .collect();
        if !gone.is_empty() {
            missing.push(format!("from {f:?}: {gone:?}"));
        }
        t.key(ESC);
        t.wait_for("Esc closes the overlay", UI, |v| {
            help_keys(v).is_empty() || !v.contains("(not in this build)")
        });
    }
    assert!(
        missing.is_empty(),
        "the help overlay does not list every binding:\n{}\n{}",
        missing.join("\n"),
        t.ctx()
    );
    t.quit();
}

// =============================================================================================
// AC-517, AC-525: layout and resize.
// =============================================================================================

#[test]
fn ac517_wide_layout_has_a_34_column_list_of_two_line_rows() {
    let rb = RpcBox::new();
    for (cols, rows) in [(120u16, 40u16), (100, 30)] {
        let mut t = start(&rb, &["watch"], cols, rows);
        wait_list(&mut t, A.handle);
        let v = t.screen().clone();
        assert_eq!(v.grid[1][34].ch, "│", "tab divider at {cols}\n{}", t.ctx());
        for y in body(&v) {
            assert_eq!(
                v.grid[y][34].ch,
                "│",
                "divider row {y} at {cols}\n{}",
                t.ctx()
            );
        }
        let ys: Vec<usize> = body(&v).filter(|&y| list_entry(&v, y).is_some()).collect();
        assert_eq!(ys.len(), 4, "{}", t.ctx());
        for w in ys.windows(2) {
            assert_eq!(w[1] - w[0], 2, "two-line rows\n{}", t.ctx());
        }
        for &y in &ys {
            let h = list_entry(&v, y).unwrap().2;
            let title = [&A, &B, &C, &G]
                .into_iter()
                .find(|f| f.handle == h)
                .unwrap()
                .title;
            let line2 = v.text(y + 1, 0, 34);
            assert!(
                line2.contains(&title[..12]),
                "title line for {h}\n{}",
                t.ctx()
            );
        }
        t.quit();
    }
}

#[test]
fn ac517_medium_layout_has_a_24_column_list_of_one_line_rows() {
    let rb = RpcBox::new();
    for (cols, rows) in [(80u16, 24u16), (72, 20), (99, 30)] {
        let mut t = start(&rb, &["watch"], cols, rows);
        wait_list(&mut t, A.handle);
        let v = t.screen().clone();
        for y in body(&v) {
            assert_eq!(
                v.grid[y][24].ch,
                "│",
                "divider row {y} at {cols}\n{}",
                t.ctx()
            );
        }
        let ys: Vec<usize> = body(&v).filter(|&y| list_entry(&v, y).is_some()).collect();
        assert_eq!(ys.len(), 4, "{}", t.ctx());
        for w in ys.windows(2) {
            assert_eq!(w[1] - w[0], 1, "one-line rows\n{}", t.ctx());
        }
        for &y in &ys {
            let h = list_entry(&v, y).unwrap().2;
            let title = [&A, &B, &C, &G]
                .into_iter()
                .find(|f| f.handle == h)
                .unwrap()
                .title;
            let rest: String = v.text(y, 11, 24).trim().to_owned();
            assert!(fits(&rest, title), "{rest:?} vs {title:?}\n{}", t.ctx());
        }
        t.quit();
    }
}

#[test]
fn ac517_narrow_layout_shows_one_pane_behind_two_tabs() {
    let rb = RpcBox::new();
    for (cols, rows) in [(71u16, 20u16), (60, 20), (40, 10)] {
        let mut t = start(&rb, &["watch", A.handle], cols, rows);
        let v = t.screen().clone();
        let tabs: String = tab_strip(&v)
            .replace('▍', " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            tabs.starts_with("sessions │ e27c0de · ") && tabs.contains("idle"),
            "narrow tabs {tabs:?}\n{}",
            t.ctx()
        );
        t.focus_to(Focus::Sessions);
        t.wait_for("the list pane", UI, |v| list_handles(v).len() == 4);
        assert!(
            !pane_lines(t.screen())
                .iter()
                .any(|l| l.contains("Why does parser_test flake?")),
            "one pane at a time\n{}",
            t.ctx()
        );
        t.key(TAB);
        t.wait_for("the transcript pane", UI, |v| {
            list_handles(v).is_empty() && pane_lines(v).iter().any(|l| l.contains("Fixed."))
        });
        t.quit();
    }
}

fn too_small_shown(v: &Vt, w: usize, h: usize) -> bool {
    let msg = format!("glen watch needs at least 40×10 · now {w}×{h}");
    let lines = v.lines();
    let text: Vec<&String> = lines.iter().filter(|l| !l.is_empty()).collect();
    if unicode_width::UnicodeWidthStr::width(msg.as_str()) <= w {
        text.len() == 1 && text[0].trim() == msg
    } else {
        let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        let shown = squash(&lines.concat());
        let want = squash(&msg);
        !shown.is_empty()
            && (want.starts_with(shown.trim_end_matches('…')))
            && shown.len() >= want.len().min(w * h / 2).min(8)
    }
}

#[test]
fn ac517_below_40x10_only_the_size_message_shows() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    for (w, h) in [
        (60usize, 9usize),
        (45, 5),
        (99, 3),
        (39, 10),
        (39, 40),
        (20, 20),
    ] {
        t.resize(w as u16, h as u16);
        t.wait_for(&format!("the size message at {w}x{h}"), UI, move |v| {
            too_small_shown(v, w, h)
        });
    }
    t.resize(40, 10);
    t.wait_for("the normal layout at 40x10", UI, |v| rule_row(v).is_some());
    t.resize(120, 40);
    t.wait_for("the normal layout again", UI, |v| {
        rule_row(v).is_some() && list_width(v) == Some(34)
    });
    t.quit();
}

/// Resize and wait for the redrawn frame.
fn resize_and_settle(t: &mut Tui, w: u16, h: u16) {
    let clears = t.screen().clears;
    t.resize(w, h);
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && t.screen().clears == clears {
        std::thread::sleep(Duration::from_millis(10));
    }
    t.settle(Duration::from_millis(120));
}

#[test]
fn ac517_every_size_draws_without_panic_or_overflow() {
    let rb = RpcBox::new();
    add_session(
        &rb.bx,
        ALPHA,
        &uuid(0x517),
        Some(
            "An extremely long session title that goes on and on well past any column budget glen has",
        ),
        A.cwd,
        &chain(3, 1, long_text, None),
    );
    append_record(
        &inbox_path(&rb, A.id),
        &record(
            A.id,
            "a reply for the badges",
            false,
            jiff::Timestamp::now(),
        ),
    );
    let mut t = start(&rb, &["watch"], 120, 40);
    t.focus_to(Focus::Prompt);
    t.typ("line one of a prompt");
    t.send(ALT_ENTER);
    t.typ("line two");
    let widths = [
        1u16, 2, 5, 10, 20, 39, 40, 41, 60, 71, 72, 73, 80, 99, 100, 101, 120, 200, 300,
    ];
    let heights = [1u16, 3, 9, 10, 11, 24, 50, 100];
    for &w in &widths {
        for &h in &heights {
            resize_and_settle(&mut t, w, h);
            let ctx = format!("at {w}x{h}");
            assert!(t.alive(), "glen exited {ctx}\n{}", t.ctx());
            let v = t.screen().clone();
            assert!(
                v.oob.is_empty(),
                "text past the screen {ctx}: {:?}\n{}",
                v.oob,
                t.ctx()
            );
            if let (Some(lw), Some(_)) = (list_width(&v), rule_row(&v)) {
                for y in body(&v) {
                    assert_eq!(
                        v.grid[y][lw].ch,
                        "│",
                        "list text crossed the divider {ctx}\n{}",
                        t.ctx()
                    );
                }
            }
            if mode_of(w as usize, h as usize) == Mode::TooSmall {
                assert!(
                    rule_row(&v).is_none() && !v.contains("│"),
                    "only the size message below 40x10 {ctx}\n{}",
                    t.ctx()
                );
            }
        }
    }
    resize_and_settle(&mut t, 120, 40);
    t.quit();
    assert!(!t.raw().contains("panicked"), "{}", t.ctx());
}

#[test]
fn ac525_a_following_pane_rewraps_at_the_new_width_and_stays_at_the_bottom() {
    let rb = RpcBox::new();
    let (_, h) = many_items(&rb.bx, 0x525, 30, long_text);
    let mut t = start(&rb, &["watch", &h], 120, 40);
    for (w, hh) in [(120u16, 40u16), (100, 40), (160, 40), (80, 30), (130, 45)] {
        resize_and_settle(&mut t, w, hh);
        let want = show_lines(&rb, &h, pane_width(w as usize, hh as usize), &[]);
        t.wait_for(
            &format!("the last page re-wrapped at {w}x{hh}"),
            UI,
            move |v| pane_matches(v, &want),
        );
    }
    t.quit();
}

#[test]
fn ac525_a_scrolled_pane_keeps_its_top_item_at_the_top() {
    let rb = RpcBox::new();
    let (_, h) = many_items(&rb.bx, 0x5251, 30, long_text);
    let mut t = start(&rb, &["watch", &h], 120, 40);
    let old = show_lines(&rb, &h, pane_width(120, 40), &[]);
    let o2 = old.clone();
    t.wait_for("following", UI, move |v| pane_matches(v, &o2));
    t.focus_to(Focus::Transcript);
    t.key(PGUP);
    t.key(PGUP);
    t.settle(Duration::from_millis(150));
    let mut k0 =
        window_offset(t.screen(), &old).unwrap_or_else(|| panic!("no window\n{}", t.ctx()));
    while old[k0].is_empty() {
        t.key(b"k");
        t.settle(Duration::from_millis(100));
        k0 = window_offset(t.screen(), &old).unwrap();
    }
    let item = old[..=k0]
        .iter()
        .filter(|l| l.starts_with("❯ user"))
        .count()
        - 1;
    for (w, hh) in [(100u16, 40u16), (160, 40)] {
        resize_and_settle(&mut t, w, hh);
        let new = show_lines(&rb, &h, pane_width(w as usize, hh as usize), &[]);
        let starts: Vec<usize> = new
            .iter()
            .enumerate()
            .filter(|(_, l)| l.starts_with("❯ user"))
            .map(|(i, _)| i)
            .collect();
        let want = starts[item];
        let n2 = new.clone();
        t.wait_for(
            &format!("item {item} at the top at {w}x{hh}"),
            UI,
            move |v| window_offset(v, &n2) == Some(want),
        );
    }
    t.quit();
}

// =============================================================================================
// AC-518: colour.
// =============================================================================================

fn colour_scene(rb: &RpcBox, args: &[&str], env: &[(&str, &str)]) -> Tui {
    let mut t = start_env(rb, args, env, 120, 40);
    wait_pane(&mut t, "The test depends on map order. Fixed.");
    t.wait_for("the list", UI, |v| list_handles(v).len() == 4);
    t.settle(Duration::from_millis(150));
    t
}

fn assert_roles(t: &mut Tui, depth: Depth, theme: Theme) {
    let v = t.screen().clone();
    let r = rule_row(&v).unwrap();
    let ctx = t.ctx();
    let (uy, ux) = v.find("❯ user").unwrap();
    assert_eq!(
        v.grid[uy][ux].fg,
        token_fg("accent", depth, theme),
        "❯ user\n{ctx}"
    );
    assert!(v.grid[uy][ux].bold, "❯ user bold\n{ctx}");
    let (ay, ax) = v.find("◆ assistant").unwrap();
    assert_eq!(
        v.grid[ay][ax].fg,
        token_fg("accent_2", depth, theme),
        "◆ assistant\n{ctx}"
    );
    let (hy, hx) = list_row_of(&v, G.handle)
        .map(|(y, _)| {
            (
                y,
                v.find_all(G.handle)
                    .into_iter()
                    .find(|p| p.0 == y)
                    .unwrap()
                    .1,
            )
        })
        .unwrap();
    assert_eq!(
        v.grid[hy][hx].fg,
        token_fg("focus", depth, theme),
        "list handle\n{ctx}"
    );
    assert_eq!(
        v.grid[r][0].fg,
        token_fg("border", depth, theme),
        "prompt rule\n{ctx}"
    );
}

#[test]
fn ac518_truecolor_cells_use_only_palette_tokens_and_no_background() {
    let rb = RpcBox::new();
    let mut t = colour_scene(&rb, &["watch", A.handle], &[("COLORTERM", "truecolor")]);
    let ctx = t.ctx();
    let seen = palette_tokens(t.screen(), Depth::True, Theme::Dark, &ctx);
    for tok in ["focus", "accent", "accent_2", "muted", "border"] {
        assert!(seen.contains(tok), "{tok} unused: {seen:?}\n{ctx}");
    }
    assert_roles(&mut t, Depth::True, Theme::Dark);
    t.quit();
}

#[test]
fn ac518_the_theme_follows_glen_theme_then_colorfgbg_then_dark() {
    let rb = RpcBox::new();
    for (env, theme) in [
        (vec![("GLEN_THEME", "light")], Theme::Light),
        (vec![("COLORFGBG", "0;15")], Theme::Light),
        (vec![("COLORFGBG", "15;0")], Theme::Dark),
        (
            vec![("GLEN_THEME", "dark"), ("COLORFGBG", "0;15")],
            Theme::Dark,
        ),
        (vec![], Theme::Dark),
    ] {
        let mut e = env.clone();
        e.push(("COLORTERM", "truecolor"));
        let mut t = colour_scene(&rb, &["watch", A.handle], &e);
        let ctx = t.ctx();
        palette_tokens(t.screen(), Depth::True, theme, &ctx);
        assert_roles(&mut t, Depth::True, theme);
        t.quit();
    }
}

#[test]
fn ac518_256_and_16_colour_depths() {
    let rb = RpcBox::new();
    for (env, depth) in [
        (vec![("TERM", "xterm-256color")], Depth::X256),
        (vec![("TERM", "xterm")], Depth::X16),
        (vec![("TERM", "xterm"), ("COLORTERM", "24bit")], Depth::True),
    ] {
        let mut t = colour_scene(&rb, &["watch", A.handle], &env);
        let ctx = t.ctx();
        palette_tokens(t.screen(), depth, Theme::Dark, &ctx);
        assert_roles(&mut t, depth, Theme::Dark);
        if depth == Depth::X16 {
            let v = t.screen().clone();
            let (y, x) = v.find("· thinking level high").unwrap();
            assert!(
                v.grid[y][x].fg == Fg::Default && v.grid[y][x].dim,
                "16-colour muted is default + dim\n{ctx}"
            );
        }
        t.quit();
    }
}

#[test]
fn ac518_colour_off_drops_every_colour_and_modifier_but_keeps_the_glyphs() {
    let rb = RpcBox::new();
    for (args, env) in [
        (vec!["watch", A.handle], vec![("NO_COLOR", "1")]),
        (vec!["watch", A.handle], vec![("NO_COLOR", " ")]),
        (vec!["watch", A.handle, "--color", "never"], vec![]),
        (vec!["watch", A.handle], vec![("CLICOLOR", "0")]),
    ] {
        let mut e = env.clone();
        e.push(("COLORTERM", "truecolor"));
        let mut t = colour_scene(&rb, &args, &e);
        t.focus_to(Focus::Sessions);
        t.settle(Duration::from_millis(150));
        let v = t.screen().clone();
        let styled: Vec<String> = v
            .all_cells()
            .filter(|c| c.styled())
            .map(|c| c.ch.clone())
            .take(10)
            .collect();
        assert!(
            styled.is_empty(),
            "styled cells with {args:?} {env:?}: {styled:?}\n{}",
            t.ctx()
        );
        assert!(v.bg_sgr.is_empty(), "{}", t.ctx());
        assert!(
            v.row(1).starts_with("▍sessions"),
            "▍ marks the focused pane\n{}",
            t.ctx()
        );
        assert_eq!(
            selected_handle(&v).as_deref(),
            Some(A.handle),
            "▌ marks the selection\n{}",
            t.ctx()
        );
        assert!(v.contains("○ e27c0de"), "state glyph\n{}", t.ctx());
        t.quit();
    }
}

#[test]
fn ac518_color_always_keeps_colour_with_no_color_set() {
    let rb = RpcBox::new();
    let mut t = colour_scene(
        &rb,
        &["watch", A.handle, "--color", "always"],
        &[("NO_COLOR", "1"), ("COLORTERM", "truecolor")],
    );
    let ctx = t.ctx();
    let seen = palette_tokens(t.screen(), Depth::True, Theme::Dark, &ctx);
    assert!(
        !seen.is_empty(),
        "--color always lost every colour under NO_COLOR\n{ctx}"
    );
    assert_roles(&mut t, Depth::True, Theme::Dark);
    t.quit();
}

// =============================================================================================
// AC-519: untrusted text.
// =============================================================================================

#[test]
fn ac519_a_title_with_controls_shows_them_escaped() {
    let rb = RpcBox::new();
    let id = uuid(0x519);
    let h = tail7(&id);
    add_session(
        &rb.bx,
        ALPHA,
        &id,
        Some("evil \u{1b}[31mred\u{7} bell"),
        A.cwd,
        &chain(1, 1, short_item, None),
    );
    let mut t = start(&rb, &["watch"], 120, 40);
    wait_list(&mut t, &h);
    let v = t.screen().clone();
    let (y, _) = list_row_of(&v, &h).unwrap();
    assert!(
        v.text(y + 1, 0, 34).contains("evil \\x1b[31mred"),
        "list title\n{}",
        t.ctx()
    );
    assert!(
        tab_strip(&v).contains("evil \\x1b[31mred"),
        "transcript tab\n{}",
        t.ctx()
    );
    let b = t.bytes();
    assert!(
        find(&b, b"\x1b[31mred").is_none(),
        "the title's ESC reached the terminal"
    );
    assert!(!b.contains(&0x07), "the title's BEL reached the terminal");
    t.quit();
}

#[test]
fn ac519_reply_text_is_sanitised_in_the_strip_and_overlay() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    append_record(
        &inbox_path(&rb, A.id),
        &record(
            A.id,
            "ok \u{1b}]0;pwned\u{7} \u{1b}[2Jgone",
            false,
            jiff::Timestamp::now(),
        ),
    );
    t.wait_for("the escaped reply", UI, |v| {
        strip_find(v, "↩ ok \\x1b]0;pwned\\x07 \\x1b[2Jgone").is_some()
    });
    t.focus_to(Focus::Sessions);
    t.key(b"i");
    t.wait_text("ok \\x1b]0;pwned\\x07 \\x1b[2Jgone", UI);
    t.key(ESC);
    let b = t.bytes();
    assert!(
        find(&b, b"\x1b]0;pwned").is_none()
            && find(&b, b"\x1b[2Jgone").is_none()
            && find(&b, b"pwned\x07").is_none(),
        "raw controls from the reply reached the terminal"
    );
    t.quit();
}

#[test]
fn ac519_the_prompt_shows_escapes_but_sends_the_exact_text() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    t.focus_to(Focus::Prompt);
    t.paste(b"a\x1bb\tc");
    t.wait_for("the escaped prompt", UI, |v| input_text(v) == "a\\x1bb\\tc");
    t.key(ENTER);
    let id = rb.wait_one_job();
    assert_eq!(job_of(&rb, &id)["text"], "a\u{1b}b\tc");
    rb.wait_terminal(&id);
    let before = rb.job_ids();
    t.paste(b"l1\r\nl2\rl3");
    t.wait_for("three prompt lines", UI, |v| input_text(v) == "l1\nl2\nl3");
    t.key(ENTER);
    let id = new_job(&rb, &before);
    assert_eq!(
        job_of(&rb, &id)["text"],
        "l1\nl2\nl3",
        "CRLF and CR become LF, no doubles"
    );
    rb.wait_terminal(&id);
    t.quit();
    rb.assert_quiet();
}

// =============================================================================================
// AC-520: empty and interim states.
// =============================================================================================

fn either(v: &Vt, a: &str, b: &str) -> bool {
    v.contains(a) || v.contains(b)
}

/// The list region's rows (wide layout), trimmed.
fn list_texts(v: &Vt) -> Vec<String> {
    let w = list_width(v).unwrap_or(v.cols);
    body(v).map(|y| v.text(y, 0, w).trim().to_owned()).collect()
}

/// A named screen expectation.
type Check<'a> = (&'a str, &'a dyn Fn(&Vt) -> bool);

/// Evaluate every expectation and report all misses at once.
fn expect_all(t: &mut Tui, checks: &[Check<'_>]) {
    let deadline = Instant::now() + UI;
    loop {
        let v = t.screen().clone();
        let missed: Vec<&str> = checks
            .iter()
            .filter(|(_, f)| !f(&v))
            .map(|(n, _)| *n)
            .collect();
        if missed.is_empty() {
            return;
        }
        if Instant::now() >= deadline {
            panic!("missing empty-state texts: {missed:?}\n{}", t.ctx());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn ac520_no_sessions_dir_and_no_selection() {
    let rb = RpcBox::new();
    let home = fs::canonicalize(&rb.bx.home).unwrap();
    fs::create_dir_all(home.join("ae")).unwrap();
    let spec = Spec::new(&rb, &rb.bx.alpha, &["watch"]).env("PI_CODING_AGENT_DIR", home.join("ae"));
    let mut t = Tui::start(spec, 120, 40, Ctty::No);
    t.ready();
    expect_all(
        &mut t,
        &[
            ("list: no omp sessions yet", &|v: &Vt| {
                list_texts(v).iter().any(|l| l == "no omp sessions yet")
            }),
            ("list: (~/ae/sessions does not exist)", &|v: &Vt| {
                list_texts(v)
                    .iter()
                    .any(|l| l == "(~/ae/sessions does not exist)")
            }),
            ("transcript: select a session", &|v: &Vt| {
                pane_lines(v).iter().any(|l| l.trim() == "select a session")
            }),
            ("transcript: ↑↓ choose · ⏎ write", &|v: &Vt| {
                pane_lines(v)
                    .iter()
                    .any(|l| ["↑↓ choose · ⏎ write", "↑↓ choose · enter write"].contains(&l.trim()))
            }),
            ("prompt: select a session to write to it", &|v: &Vt| {
                (rule_row(v).unwrap_or(0) + 1..v.rows - 1)
                    .any(|y| v.row(y).contains("select a session to write to it"))
            }),
        ],
    );
    t.quit();
}

#[test]
fn ac520_an_empty_scope_names_the_project_and_the_other_sessions() {
    let rb = RpcBox::new();
    let gamma = rb.bx.work("gamma");
    let mut t = Tui::start(Spec::new(&rb, &gamma, &["watch"]), 120, 40, Ctty::No);
    t.ready();
    expect_all(
        &mut t,
        &[
            ("list: no omp sessions for ~/work/gamma", &|v: &Vt| {
                list_texts(v)
                    .iter()
                    .any(|l| l == "no omp sessions for ~/work/gamma")
            }),
            ("list: a shows 7 sessions in other projects", &|v: &Vt| {
                list_texts(v)
                    .iter()
                    .any(|l| fits(l, "a shows 7 sessions in other projects"))
            }),
        ],
    );
    t.quit();
}

#[test]
fn ac520_an_endpoint_only_session_is_live_but_not_persisted() {
    let lb = LiveBox::new();
    let id = uuid(0x5200);
    let h = tail7(&id);
    let alpha = s(&fs::canonicalize(&lb.rb.bx.alpha).unwrap());
    let _ext = lb.start_ext(&id, "ok", &[("FAKE_EXT_CWD", &alpha)], &[]);
    let mut t = start(&lb.rb, &["watch", &h], 120, 40);
    let want = format!("{h} is live but has not written a transcript yet");
    expect_all(
        &mut t,
        &[
            (
                "<handle> is live but has not written a transcript yet",
                &|v: &Vt| pane_lines(v).iter().any(|l| l.trim() == want),
            ),
            ("send a message to start it", &|v: &Vt| {
                pane_lines(v)
                    .iter()
                    .any(|l| l.trim() == "send a message to start it")
            }),
        ],
    );
    t.quit();
}

#[test]
fn ac520_a_session_without_messages() {
    let rb = RpcBox::new();
    let id = uuid(0x5201);
    add_session(&rb.bx, ALPHA, &id, Some("Nothing yet"), A.cwd, &[]);
    let mut t = start(&rb, &["watch", &tail7(&id)], 120, 40);
    t.wait_for("no messages yet", UI, |v| {
        pane_lines(v).iter().any(|l| l.trim() == "no messages yet")
    });
    t.quit();
}

#[test]
fn ac520_the_empty_inbox_overlay() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    t.focus_to(Focus::Sessions);
    t.key(b"i");
    t.wait_text("peek only · glen inbox marks these read", UI);
    assert!(
        t.screen().contains("no messages from omp yet"),
        "{}",
        t.ctx()
    );
    t.quit();
}

#[test]
fn ac520_the_prompt_placeholders() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    t.focus_to(Focus::Sessions);
    t.wait_for("the unfocused placeholder", UI, |v| {
        either(v, "⇥ to write to e27c0de", "tab to write to e27c0de")
    });
    t.focus_to(Focus::Prompt);
    t.wait_for("the focused placeholder", UI, |v| {
        either(
            v,
            "message e27c0de · ⏎ send",
            "message e27c0de · enter send",
        )
    });
    t.quit();
}

#[test]
fn ac520_an_unreadable_transcript_shows_the_open_error() {
    let rb = RpcBox::new();
    let a = rb.bx.path(&A);
    let mut t = start(&rb, &["watch"], 120, 40);
    wait_list(&mut t, A.handle);
    set_mode(&a, 0o000);
    let _restore = RestoreMode(a.clone(), 0o644);
    t.focus_to(Focus::Sessions);
    t.key(b"G");
    t.wait_for("the open error", UI, |v| {
        selected_handle(v).as_deref() == Some(A.handle)
            && pane_lines(v).iter().any(|l| l.starts_with("✗ "))
    });
    let v = t.screen().clone();
    let (y, x) = v
        .find_all("✗ ")
        .into_iter()
        .find(|(y, _)| *y < rule_row(&v).unwrap())
        .unwrap();
    assert_eq!(
        v.grid[y][x].fg,
        token_fg("danger", Depth::X256, Theme::Dark),
        "{}",
        t.ctx()
    );
    t.quit();
}
// =============================================================================================
// AC-521: the keymap.
// =============================================================================================

#[test]
fn ac521_tab_and_shift_tab_cycle_sessions_transcript_prompt() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    let order = [Focus::Sessions, Focus::Transcript, Focus::Prompt];
    let at = |f: Option<Focus>| order.iter().position(|o| Some(*o) == f).expect("a focus");
    for (key, step) in [(TAB, 1usize), (BACKTAB, 2)] {
        for _ in 0..3 {
            let now = at(focus(t.screen()));
            let want = order[(now + step) % 3];
            t.key(key);
            t.wait_for(&format!("{want:?}"), UI, move |v| focus(v) == Some(want));
        }
    }
    t.quit();
}

#[test]
fn ac521_printable_keys_type_only_in_the_prompt_and_q_quits_only_from_a_pane() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    for f in [Focus::Sessions, Focus::Transcript] {
        t.focus_to(f);
        t.typ("xyz");
    }
    t.focus_to(Focus::Prompt);
    t.wait_for("the prompt focused", UI, |v| {
        focus(v) == Some(Focus::Prompt)
    });
    assert!(
        !input_text(t.screen()).contains("xyz"),
        "keys typed into an unfocused prompt\n{}",
        t.ctx()
    );
    t.typ("q?");
    t.wait_for("q and ? type in the prompt", UI, |v| input_text(v) == "q?");
    assert!(t.alive(), "q quit from the prompt");
    t.key(ESC);
    t.wait_for("the transcript focused", UI, |v| {
        focus(v) == Some(Focus::Transcript)
    });
    t.send(b"q");
    assert_eq!(t.wait_exit(UI), 0, "{}", t.ctx());
    t.assert_restored();
}

#[test]
fn ac521_ctrl_c_clears_a_non_empty_prompt_then_quits() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    t.focus_to(Focus::Prompt);
    t.typ("abc");
    t.wait_for("abc", UI, |v| input_text(v) == "abc");
    t.key(CTRL_C);
    t.wait_for("cleared", UI, |v| !input_text(v).contains("abc"));
    assert!(t.alive(), "Ctrl-C quit with text in the prompt");
    t.send(CTRL_C);
    assert_eq!(t.wait_exit(UI), 0, "{}", t.ctx());
    t.assert_restored();
}

#[test]
fn ac521_ctrl_d_quits_only_from_an_empty_focused_prompt() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    for f in [Focus::Sessions, Focus::Transcript] {
        t.focus_to(f);
        t.key(CTRL_D);
        std::thread::sleep(Duration::from_millis(300));
        assert!(t.alive(), "Ctrl-D quit from {f:?}\n{}", t.ctx());
    }
    t.focus_to(Focus::Prompt);
    t.typ("abc");
    t.key(CTRL_D);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        t.alive(),
        "Ctrl-D quit from a non-empty prompt\n{}",
        t.ctx()
    );
    t.key(CTRL_E);
    for _ in 0..3 {
        t.key(BACKSPACE);
    }
    t.wait_for("empty", UI, |v| !input_text(v).contains('a'));
    t.send(CTRL_D);
    assert_eq!(t.wait_exit(UI), 0, "{}", t.ctx());
    t.assert_restored();
}

#[test]
fn ac521_session_keys_move_the_selection_and_enter_focuses_the_prompt() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch"], 120, 40);
    t.wait_for("G selected", UI, |v| {
        selected_handle(v).as_deref() == Some(G.handle)
    });
    t.focus_to(Focus::Sessions);
    // ls order G, C, B, A; B continues in C, so the walk never rests on B.
    for (key, want) in [
        (b"j".as_slice(), C.handle),
        (b"k", G.handle),
        (DOWN, C.handle),
        (UP, G.handle),
        (b"G", A.handle),
        (b"g", G.handle),
        (END, A.handle),
        (HOME, G.handle),
        (PGDN, A.handle),
        (PGUP, G.handle),
    ] {
        t.key(key);
        t.wait_for(&format!("{want} after {key:?}"), UI, move |v| {
            selected_handle(v).as_deref() == Some(want) && tab_strip(v).contains(want)
        });
    }
    t.key(ENTER);
    t.wait_for("the prompt for G", UI, |v| {
        focus(v) == Some(Focus::Prompt)
            && input_row(v).is_some_and(|y| v.row(y).starts_with(&format!("{} ❯", G.handle)))
    });
    t.quit();
}

#[test]
fn ac521_transcript_keys_scroll_lines_pages_and_ends() {
    let rb = RpcBox::new();
    let (_, h) = many_items(&rb.bx, 0x521, 60, short_item);
    let w = pane_width(120, 40);
    let lines = show_lines(&rb, &h, w, &[]);
    let mut t = start(&rb, &["watch", &h], 120, 40);
    let l2 = lines.clone();
    t.wait_for("following", UI, move |v| pane_matches(v, &l2));
    t.focus_to(Focus::Transcript);
    let off = |t: &mut Tui| {
        t.settle(Duration::from_millis(100));
        window_offset(t.screen(), &lines).unwrap_or_else(|| panic!("no window\n{}", t.ctx()))
    };
    let bottom = lines.len() - pane_lines(t.screen()).len();
    t.key(b"k");
    let o1 = off(&mut t);
    assert_eq!(o1, bottom - 1, "k scrolls one line\n{}", t.ctx());
    t.key(b"k");
    t.key(UP);
    assert_eq!(off(&mut t), bottom - 3, "↑ scrolls one line\n{}", t.ctx());
    t.key(b"j");
    assert_eq!(off(&mut t), bottom - 2, "j\n{}", t.ctx());
    t.key(DOWN);
    assert_eq!(off(&mut t), bottom - 1, "↓\n{}", t.ctx());
    let page = pane_lines(t.screen()).len();
    let before = off(&mut t);
    t.key(PGUP);
    let after = off(&mut t);
    assert!(
        (page / 2 + 1..=page + 1).contains(&(before - after)),
        "PgUp moved {} (page {page})\n{}",
        before - after,
        t.ctx()
    );
    let before = after;
    t.key(CTRL_U);
    let after = off(&mut t);
    assert!(
        (page / 2 - 2..=page / 2 + 2).contains(&(before - after)),
        "Ctrl-U moved {}\n{}",
        before - after,
        t.ctx()
    );
    let before = after;
    t.key(CTRL_D);
    let after = off(&mut t);
    assert!(
        (page / 2 - 2..=page / 2 + 2).contains(&(after - before)),
        "Ctrl-D moved {}\n{}",
        after - before,
        t.ctx()
    );
    t.key(PGDN);
    assert!(off(&mut t) > after, "PgDn\n{}", t.ctx());
    for key in [b"g".as_slice(), HOME] {
        t.key(b"j");
        t.key(key);
        assert_eq!(off(&mut t), 0, "{key:?} goes to the top\n{}", t.ctx());
    }
    for key in [b"G".as_slice(), END] {
        t.key(b"g");
        t.key(key);
        let l2 = lines.clone();
        t.wait_for("the bottom, following", UI, move |v| pane_matches(v, &l2));
    }
    t.quit();
}

#[test]
fn ac521_prompt_editing_keys_and_history() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    t.focus_to(Focus::Prompt);
    let step = |t: &mut Tui, keys: &[&[u8]], want: &str| {
        for k in keys {
            t.key(k);
        }
        let w = want.to_owned();
        t.wait_for(&format!("prompt {want:?} after {keys:?}"), UI, move |v| {
            input_text(v) == w
        });
    };
    step(&mut t, &[b"hello world"], "hello world");
    step(&mut t, &[CTRL_W, b"there"], "hello there");
    step(
        &mut t,
        &[LEFT, LEFT, LEFT, LEFT, LEFT, CTRL_K, b"X"],
        "hello X",
    );
    step(&mut t, &[CTRL_A, b">"], ">hello X");
    step(&mut t, &[CTRL_E, b"<"], ">hello X<");
    step(&mut t, &[HOME, b"["], "[>hello X<");
    step(&mut t, &[END, b"]"], "[>hello X<]");
    step(&mut t, &[BACKSPACE], "[>hello X<");
    step(&mut t, &[HOME, DELETE], ">hello X<");
    step(&mut t, &[RIGHT, RIGHT, CTRL_U], "ello X<");
    step(
        &mut t,
        &[END, CTRL_U, b"a", ALT_ENTER, b"b", CTRL_J, b"c"],
        "a\nb\nc",
    );
    t.key(ENTER);
    let id = rb.wait_one_job();
    assert_eq!(job_of(&rb, &id)["text"], "a\nb\nc");
    rb.wait_terminal(&id);
    let before = rb.job_ids();
    step(&mut t, &[b"first line"], "first line");
    t.key(ENTER);
    let id2 = new_job(&rb, &before);
    rb.wait_terminal(&id2);
    t.wait_for("cleared", UI, |v| !input_text(v).contains("first line"));
    step(&mut t, &[UP], "first line");
    t.quit();
    rb.assert_quiet();
}

#[test]
fn ac521_release_events_are_ignored_and_repeat_moves_only() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 50);
    let plain = show_lines(&rb, A.handle, pane_width(120, 50), &[]);
    let thinking = show_lines(&rb, A.handle, pane_width(120, 50), &["--thinking"]);
    let p2 = plain.clone();
    t.wait_for("A's transcript", UI, move |v| pane_matches(v, &p2));
    t.focus_to(Focus::Transcript);
    t.send(b"\x1b[116;1:2u"); // t, Repeat: not a movement key
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        pane_matches(t.screen(), &plain),
        "a repeated t toggled thinking\n{}",
        t.ctx()
    );
    t.send(b"\x1b[116u"); // t, Press
    t.wait_for("thinking on", UI, move |v| pane_matches(v, &thinking));
    t.focus_to(Focus::Sessions);
    t.send(b"\x1b[113;1:3u"); // q, Release
    std::thread::sleep(Duration::from_millis(500));
    assert!(t.alive(), "a q release quit");
    t.send(b"\x1b[107;1:2u"); // k, Repeat: movement
    t.wait_for("k repeat moved the selection", UI, |v| {
        selected_handle(v).as_deref() != Some(A.handle)
    });
    t.quit();
}

#[test]
fn ac521_a_toggles_this_project_and_all_projects() {
    let rb = RpcBox::new();
    let all: Vec<String> = ls_rows(&rb, &rb.bx.alpha, &["--all"])
        .into_iter()
        .map(|r| r.0)
        .collect();
    let here: Vec<String> = ls_rows(&rb, &rb.bx.alpha, &[])
        .into_iter()
        .map(|r| r.0)
        .collect();
    let mut t = start(&rb, &["watch"], 120, 40);
    let h2 = here.clone();
    t.wait_for("this project", UI, move |v| list_handles(v) == h2);
    t.focus_to(Focus::Sessions);
    t.key(b"a");
    t.wait_for("all projects", Duration::from_secs(5), move |v| {
        list_handles(v) == all && tab_strip(v).contains("sessions · all projects")
    });
    t.key(b"a");
    t.wait_for("back to this project", Duration::from_secs(5), move |v| {
        list_handles(v) == here
    });
    t.quit();
}

// =============================================================================================
// Amendments 5 and 6 (TUI reference study).
// =============================================================================================

const HINT_GLYPHS: [char; 5] = ['⇥', '⏎', '⌥', '⌃', '⇧'];

#[test]
fn amendment6_hint_rows_use_the_ascii_register() {
    let rb = RpcBox::new();
    let mut t = start(&rb, &["watch", A.handle], 120, 40);
    let mut bad = Vec::new();
    for f in [Focus::Sessions, Focus::Transcript, Focus::Prompt] {
        t.focus_to(f);
        t.settle(Duration::from_millis(100));
        let bar = status_bar(t.screen());
        assert!(!bar.is_empty(), "empty status bar in {f:?}");
        if bar.chars().any(|c| HINT_GLYPHS.contains(&c)) {
            bad.push(format!("{f:?}: {bar}"));
        }
    }
    open_help(&mut t, Focus::Sessions);
    let bar = status_bar(t.screen());
    if bar.chars().any(|c| HINT_GLYPHS.contains(&c)) {
        bad.push(format!("help overlay: {bar}"));
    }
    t.key(ESC);
    assert!(
        bad.is_empty(),
        "hint rows use key glyphs instead of tab/enter/esc/^t:\n{}",
        bad.join("\n")
    );
    t.quit();
}

fn segments(v: &Vt) -> (Vec<String>, Vec<String>) {
    let split2 = |s: &str| -> Vec<String> {
        s.split("  ")
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let title: Vec<String> = split2(&title_bar(v))
        .into_iter()
        .flat_map(|p| p.split(" · ").map(str::to_owned).collect::<Vec<_>>())
        .collect();
    (title, split2(&status_bar(v)))
}

/// `seg` is `full` or a `…` truncation of it.
fn same_segment(seg: &str, full: &str) -> bool {
    seg == full || seg.strip_suffix('…').is_some_and(|p| full.starts_with(p))
}

#[test]
fn amendment5_title_and_status_bars_collapse_monotonically() {
    let rb = RpcBox::new();
    append_record(
        &inbox_path(&rb, A.id),
        &record(A.id, "an unread reply", false, jiff::Timestamp::now()),
    );
    let mut t = start(&rb, &["watch", A.handle], 200, 30);
    t.wait_for("the unread count", UI, |v| title_bar(v).contains("↩ 1"));
    t.focus_to(Focus::Sessions);
    t.settle(Duration::from_millis(150));
    let (full_title, full_status) = segments(t.screen());
    let mut prev: Option<(BTreeSet<usize>, BTreeSet<usize>)> = None;
    let mut widths: Vec<u16> = (40..=200).rev().step_by(3).collect();
    widths.extend([100, 99, 72, 71]);
    widths.sort_unstable_by(|a, b| b.cmp(a));
    widths.dedup();
    for w in widths {
        resize_and_settle(&mut t, w, 30);
        let (ti, st) = segments(t.screen());
        let map = |segs: &[String], full: &[String], what: &str, t: &mut Tui| -> BTreeSet<usize> {
            segs.iter()
                .map(|s| {
                    full.iter().position(|f| same_segment(s, f)).unwrap_or_else(|| {
                        panic!("{what} segment {s:?} at width {w} is not in the widest bar {full:?}\n{}", t.ctx())
                    })
                })
                .collect()
        };
        let cur = (
            map(&ti, &full_title, "title", &mut t),
            map(&st, &full_status, "status", &mut t),
        );
        if let Some((pt, ps)) = &prev {
            assert!(
                cur.0.is_subset(pt),
                "a title segment reappeared at width {w}\n{}",
                t.ctx()
            );
            assert!(
                cur.1.is_subset(ps),
                "a status segment reappeared at width {w}\n{}",
                t.ctx()
            );
        }
        prev = Some(cur);
    }
    t.quit();
}

// =============================================================================================
// AC-522, AC-523, AC-524: restore, signals, panics.
// =============================================================================================

#[test]
fn ac522_q_restores_within_250ms_and_exits_0() {
    let rb = RpcBox::new();
    for ctty in [Ctty::No, Ctty::Yes] {
        let mut t = Tui::start(
            Spec::new(&rb, &rb.bx.alpha, &["watch", A.handle]),
            120,
            40,
            ctty,
        );
        t.ready();
        if ctty == Ctty::No || cfg!(target_os = "linux") {
            t.assert_raw();
        }
        t.focus_to(Focus::Sessions);
        let t0 = Instant::now();
        t.send(b"q");
        let restore =
            time_to_restore(&t, t0, UI).unwrap_or_else(|| panic!("no restore\n{}", t.ctx()));
        let exit = time_to_exit(&mut t, t0, UI).unwrap_or_else(|| panic!("no exit\n{}", t.ctx()));
        eprintln!("{ctty:?}: q → restore {restore:?}, exit {exit:?}");
        assert!(
            restore <= Duration::from_millis(250) + RESTORE_SLACK,
            "restore took {restore:?}"
        );
        assert!(
            exit <= Duration::from_millis(300) + RESTORE_SLACK,
            "exit took {exit:?}"
        );
        assert_eq!(t.wait_exit(UI), 0, "{}", t.ctx());
        t.assert_restored();
    }
    rb.bx.assert_no_state();
}

#[test]
fn ac522_quit_waits_for_an_in_flight_submit() {
    if !cfg!(debug_assertions) {
        return;
    }
    let rb = RpcBox::new();
    let gate = rb.bx.root.join("gate");
    let mut t = start_env(
        &rb,
        &["watch", A.handle],
        &[("FAKE_OMP_LEASE_PROBE_GATE", &s(&gate))],
        120,
        40,
    );
    t.focus_to(Focus::Prompt);
    t.typ("in flight");
    t.key(ENTER);
    wait_file_exists(&ready_path(&gate), Duration::from_secs(5));
    t.key(ESC);
    t.wait_for("the transcript focused", UI, |v| {
        focus(v) == Some(Focus::Transcript)
    });
    let t0 = Instant::now();
    t.send(b"q");
    let restore = time_to_restore(&t, t0, UI).unwrap_or_else(|| panic!("no restore\n{}", t.ctx()));
    eprintln!("q with a submit in flight → restore {restore:?}");
    assert!(
        restore <= Duration::from_millis(250) + RESTORE_SLACK,
        "restore waited for the submit: {restore:?}"
    );
    std::thread::sleep(Duration::from_millis(700));
    assert!(t.alive(), "exited without waiting for the in-flight submit");
    fs::write(&gate, "").unwrap();
    let exit = time_to_exit(&mut t, t0, Duration::from_secs(6) + SLACK)
        .unwrap_or_else(|| panic!("no exit within the 6 s cap\n{}", t.ctx()));
    eprintln!("exit after {exit:?}");
    assert_eq!(t.wait_exit(UI), 0);
    t.assert_restored();
    for id in rb.job_ids() {
        rb.wait_terminal(&id);
    }
    rb.assert_quiet();
}

#[test]
fn ac523_signals_restore_and_exit_128_plus_n() {
    let rb = RpcBox::new();
    for (sig, code) in [(Signal::INT, 130), (Signal::TERM, 143), (Signal::HUP, 129)] {
        let mut t = start(&rb, &["watch", A.handle], 120, 40);
        t.assert_raw();
        let t0 = Instant::now();
        t.signal(sig);
        let restore = time_to_restore(&t, t0, UI)
            .unwrap_or_else(|| panic!("no restore on {sig:?}\n{}", t.ctx()));
        eprintln!("{sig:?} → restore {restore:?}");
        assert!(
            restore <= Duration::from_millis(250) + RESTORE_SLACK,
            "{sig:?}: restore took {restore:?}"
        );
        assert_eq!(t.wait_exit(UI), code, "{sig:?}\n{}", t.ctx());
        t.assert_restored();
    }
}

/// Run watch with stdin on one pty and stdout on another, close the output terminal after the
/// first frame, press a key, and return (exit code, stderr when piped, stdin's termios after).
/// (exit code, stderr when piped, stdin's (ICANON, ECHO) after exit).
type FrameFailure = (Option<i32>, String, Option<(bool, bool)>);

fn frame_write_failure(stderr_on_tty: bool) -> FrameFailure {
    let rb = RpcBox::new();
    let input = Pty::open(120, 40);
    let output = Pty::open(120, 40);
    let mut cmd = Spec::new(&rb, &rb.bx.alpha, &["watch", A.handle]).cmd;
    cmd.stdin(Stdio::from(input.open_slave()))
        .stdout(Stdio::from(output.open_slave()));
    if stderr_on_tty {
        cmd.stderr(Stdio::from(output.open_slave()));
    } else {
        cmd.stderr(Stdio::piped());
    }
    unsafe {
        cmd.pre_exec(|| {
            rustix::process::setsid()
                .map(|_| ())
                .map_err(std::io::Error::from)
        });
    }
    let mut child = {
        let _fork = FORK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        cmd.spawn().unwrap()
    };
    drop(cmd);
    // Drain the output pty until the first frame, then close every master fd of it.
    let stop = Arc::new(AtomicBool::new(false));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut reader = output.master.try_clone().unwrap();
    let (st, sink) = (Arc::clone(&stop), Arc::clone(&seen));
    let drain = std::thread::spawn(move || {
        let mut buf = [0u8; 65536];
        while !st.load(Ordering::Relaxed) {
            let mut pfd = libc::pollfd {
                fd: reader.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            if unsafe { libc::poll(&mut pfd, 1, 50) } > 0
                && pfd.revents & libc::POLLIN != 0
                && let Ok(n) = reader.read(&mut buf)
            {
                sink.lock().unwrap().extend_from_slice(&buf[..n]);
            }
        }
    });
    poll_until("the first frame", UI, || {
        let b = seen.lock().unwrap().clone();
        let mut vt = Vt::new(120, 40);
        vt.feed(&b);
        rule_row(&vt).is_some().then_some(())
    });
    assert_eq!(
        input.cooked(),
        Some((false, false)),
        "raw mode on stdin's terminal"
    );
    stop.store(true, Ordering::Relaxed);
    drain.join().unwrap();
    let Pty { master, slave, .. } = output;
    drop(master);
    drop(slave);
    // A key forces a redraw onto the dead terminal.
    let mut m = input.master.try_clone().unwrap();
    m.write_all(b"\t").unwrap();
    let t0 = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break st;
        }
        if t0.elapsed() > UI {
            let _ = child.kill();
            panic!("glen kept running after its terminal went away");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut err);
    }
    (status.code(), err, input.cooked())
}

#[test]
fn ac523_a_failed_frame_write_restores_and_exits_1() {
    let mut bad = Vec::new();
    for on_tty in [true, false] {
        let (code, err, termios) = frame_write_failure(on_tty);
        let wiring = if on_tty {
            "stderr on the dead terminal"
        } else {
            "stderr on a pipe"
        };
        if code != Some(1) {
            bad.push(format!("{wiring}: exit {code:?}, stderr {err:?}"));
        }
        if termios != Some((true, true)) {
            bad.push(format!("{wiring}: stdin's terminal left raw ({termios:?})"));
        }
    }
    assert!(
        bad.is_empty(),
        "a failed frame write must restore and exit 1:\n{}",
        bad.join("\n")
    );
}

#[test]
fn ac524_a_panic_restores_the_terminal_before_the_message_and_exits_101() {
    if !cfg!(debug_assertions) {
        return; // the panic knob exists only in debug builds
    }
    let rb = RpcBox::new();
    for knob in ["ui:400", "worker:400"] {
        let spec =
            Spec::new(&rb, &rb.bx.alpha, &["watch", A.handle]).env("GLEN_WATCH_TEST_PANIC", knob);
        let mut t = Tui::start(spec, 120, 40, Ctty::No);
        assert_eq!(t.wait_exit(UI), 101, "{knob}\n{}", t.ctx());
        let b = t.bytes();
        let on = rfind(&b, ALT_ON).unwrap_or_else(|| panic!("{knob}: never drew\n{}", t.ctx()));
        let off = find(&b[on..], ALT_OFF)
            .map(|p| p + on)
            .unwrap_or_else(|| panic!("{knob}: no restore\n{}", t.ctx()));
        let msg = find(&b, b"panicked")
            .unwrap_or_else(|| panic!("{knob}: no panic message\n{}", t.ctx()));
        assert!(
            off < msg,
            "{knob}: the panic message came before the restore\n{}",
            t.ctx()
        );
        t.assert_restored();
    }
}

#[test]
fn orchestrator_decision_2_a_second_signal_while_waiting_exits_at_once() {
    if !cfg!(debug_assertions) {
        return;
    }
    let rb = RpcBox::new();
    let gate = rb.bx.root.join("gate");
    let mut t = start_env(
        &rb,
        &["watch", A.handle],
        &[("FAKE_OMP_LEASE_PROBE_GATE", &s(&gate))],
        120,
        40,
    );
    t.focus_to(Focus::Prompt);
    t.typ("in flight");
    t.key(ENTER);
    wait_file_exists(&ready_path(&gate), Duration::from_secs(5));
    let t0 = Instant::now();
    t.signal(Signal::INT);
    time_to_restore(&t, t0, UI).unwrap_or_else(|| panic!("no restore\n{}", t.ctx()));
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        t.alive(),
        "the first signal did not wait for the in-flight submit"
    );
    let t1 = Instant::now();
    t.signal(Signal::INT);
    let exit = time_to_exit(&mut t, t1, Duration::from_secs(10))
        .unwrap_or_else(|| panic!("no exit 10 s after the second SIGINT\n{}", t.ctx()));
    eprintln!("second SIGINT → exit {exit:?}");
    assert!(
        exit <= Duration::from_millis(500) + RESTORE_SLACK,
        "the second SIGINT exited only after {exit:?} (the submit was still held)"
    );
    assert_eq!(t.wait_exit(UI), 130);
    t.assert_restored();
    fs::write(&gate, "").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    rb.assert_quiet();
}

// =============================================================================================
// AC-526: the pure core and quiet watch paths (the AC's own source-level rule).
// =============================================================================================

/// Source text with `//` comments and `#[cfg(test)]` modules removed.
fn runtime_source(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let code = match text.find("#[cfg(test)]") {
        Some(i) => &text[..i],
        None => &text,
    };
    code.lines()
        .map(|l| match l.find("//") {
            Some(i) if !l[..i].contains('"') => &l[..i],
            _ => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn ac526_the_core_does_no_io_and_watch_paths_never_print() {
    let src = repo_root().join("src");
    let mut bad = Vec::new();
    for f in [
        "state", "update", "prompt", "layout", "view", "theme", "ansi",
    ] {
        let p = src.join("tui").join(format!("{f}.rs"));
        assert!(
            p.is_file(),
            "AC-526 names {} and it is missing",
            p.display()
        );
        let code = runtime_source(&p);
        for token in [
            "std::fs",
            "std::process",
            "std::net",
            "UnixStream",
            "File::",
            "thread::spawn",
            "lease::probe",
            "JobStore",
            "registry::",
        ] {
            if code.contains(token) {
                bad.push(format!("src/tui/{f}.rs contains {token}"));
            }
        }
    }
    let mut quiet = Vec::new();
    rs_files(&src.join("tui"), &mut quiet);
    quiet.push(src.join("commands/follow.rs"));
    quiet.push(src.join("inbox.rs"));
    for p in quiet {
        let code = runtime_source(&p);
        for token in ["eprintln!", "println!", "print!(", "eprint!("] {
            if code.contains(token) {
                bad.push(format!(
                    "{} contains {token}",
                    p.strip_prefix(&src).unwrap().display()
                ));
            }
        }
    }
    assert!(bad.is_empty(), "AC-526:\n{}", bad.join("\n"));
}
