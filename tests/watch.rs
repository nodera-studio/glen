#![cfg(unix)]
mod common;

use common::{A_ID, Holder, LiveBox, OmpBox, RpcBox, json, poll};
use rustix::{
    fs::{Mode, OFlags},
    process::{Pid, Signal},
    pty::{self, OpenptFlags},
    termios::{self, LocalModes, Winsize},
};
use serde_json::{Value, json as j};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, BorrowedFd, RawFd},
        unix::process::CommandExt,
    },
    path::Path,
    process::{Child, Command, ExitStatus, Output, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const START: Duration = Duration::from_secs(5);
const DATA: Duration = Duration::from_secs(2);
const EXIT: Duration = Duration::from_secs(1);

/// Replay the cursor/erase subset emitted by CrosstermBackend. Stripping ANSI alone loses
/// spaces and unchanged letters in ratatui's differential draws; assertions need the screen.
fn screen(bytes: &[u8], (cols, rows): (usize, usize), since: usize) -> String {
    if bytes.len() <= since {
        return String::new();
    }
    let text = String::from_utf8_lossy(bytes);
    let mut chars = text.chars().peekable();
    let mut grid = vec![vec![String::from(" "); cols]; rows];
    let (mut x, mut y) = (0usize, 0usize);
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                Some('[') => {
                    let mut params = String::new();
                    let mut op = None;
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            op = Some(c);
                            break;
                        }
                        params.push(c);
                    }
                    let values: Vec<usize> =
                        params.split(';').map(|n| n.parse().unwrap_or(0)).collect();
                    let n = values[0].max(1);
                    match op {
                        Some('H' | 'f') => {
                            y = n.saturating_sub(1).min(rows - 1);
                            x = values
                                .get(1)
                                .copied()
                                .unwrap_or(1)
                                .max(1)
                                .saturating_sub(1)
                                .min(cols - 1);
                        }
                        Some('G') => x = n.saturating_sub(1).min(cols - 1),
                        Some('d') => y = n.saturating_sub(1).min(rows - 1),
                        Some('A') => y = y.saturating_sub(n),
                        Some('B') => y = (y + n).min(rows - 1),
                        Some('C') => x = (x + n).min(cols - 1),
                        Some('D') => x = x.saturating_sub(n),
                        Some('J') => {
                            let mode = values[0];
                            for (row, cells) in grid.iter_mut().enumerate() {
                                for (col, cell) in cells.iter_mut().enumerate() {
                                    if mode == 2
                                        || mode == 3
                                        || (mode == 0 && (row, col) >= (y, x))
                                        || (mode == 1 && (row, col) <= (y, x))
                                    {
                                        *cell = " ".into();
                                    }
                                }
                            }
                        }
                        Some('K') => {
                            for (col, cell) in grid[y].iter_mut().enumerate() {
                                if values[0] == 2
                                    || (values[0] == 0 && col >= x)
                                    || (values[0] == 1 && col <= x)
                                {
                                    *cell = " ".into();
                                }
                            }
                        }
                        _ => (), // SGR and private cursor/paste/alternate-screen modes.
                    }
                }
                Some(']') => {
                    // OSC ends with BEL or ST.
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.next() == Some('\\')) {
                            break;
                        }
                    }
                }
                _ => (),
            },
            '\r' => x = 0,
            '\n' => y = (y + 1).min(rows - 1),
            c if !c.is_control() => {
                let width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                if width == 0 {
                    if x > 0 {
                        grid[y][(x - 1).min(cols - 1)].push(c);
                    }
                } else {
                    if x + width > cols {
                        x = 0;
                        y = (y + 1).min(rows - 1);
                    }
                    grid[y][x] = c.to_string();
                    if width == 2 && x + 1 < cols {
                        grid[y][x + 1].clear();
                    }
                    x += width;
                }
            }
            _ => (),
        }
    }
    grid.into_iter()
        .map(|row| row.concat())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn cursor_updates_reconstruct_visible_spaces_and_unchanged_letters() {
    let bytes =
        b"\x1b[2J\x1b[1;1Hreply:\x1b[1;8Hwatch steer message\x1b[1;14Hauto \x1b[1;19Hmessage\x1b[K";
    assert!(screen(bytes, (40, 3), 0).starts_with("reply: watch auto message"));
    assert!(
        !anstream::adapter::strip_str(std::str::from_utf8(bytes).unwrap())
            .to_string()
            .contains("reply: watch auto message")
    );
}

/// Every directly spawned child is reaped, including on a failed assertion.
struct Process(Child);
impl Process {
    fn wait(&mut self, timeout: Duration) -> ExitStatus {
        poll(timeout, || self.0.try_wait().unwrap())
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(Some(_))) {
            return;
        }
        let killed = self.0.kill();
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until {
            if matches!(self.0.try_wait(), Ok(Some(_))) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        if !thread::panicking() {
            panic!("child {} not reaped after kill: {killed:?}", self.0.id());
        }
    }
}

struct Pty {
    master: File,
    slave: File,
    child: Option<Process>,
    raw_observed: Option<RawFd>,
    controlling: bool,
    size: (usize, usize),
    output: Arc<Mutex<Vec<u8>>>,
    stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<io::Result<()>>>,
}
impl Pty {
    fn open(cols: u16, rows: u16) -> Self {
        let master = pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
        // openpt on macOS has no CLOEXEC flag. Do not leak the master into glen or its workers.
        rustix::io::fcntl_setfd(&master, rustix::io::FdFlags::CLOEXEC).unwrap();
        pty::grantpt(&master).unwrap();
        pty::unlockpt(&master).unwrap();
        let name = pty::ptsname(&master, Vec::new()).unwrap();
        let slave = rustix::fs::open(
            &name,
            OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .unwrap();
        termios::tcsetwinsize(
            &master,
            Winsize {
                ws_col: cols,
                ws_row: rows,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )
        .unwrap();
        assert!(
            termios::tcgetattr(&slave)
                .unwrap()
                .local_modes
                .contains(LocalModes::ICANON | LocalModes::ECHO)
        );
        let flags = rustix::fs::fcntl_getfl(&master).unwrap();
        rustix::fs::fcntl_setfl(&master, flags | OFlags::NONBLOCK).unwrap();
        let master = File::from(master);
        let mut read = master.try_clone().unwrap();
        let output = Arc::new(Mutex::new(Vec::new()));
        let bytes = output.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let reader = thread::spawn(move || {
            let mut buf = [0; 8192];
            loop {
                match read.read(&mut buf) {
                    Ok(0) => return Ok(()),
                    Ok(n) => bytes.lock().unwrap().extend_from_slice(&buf[..n]),
                    Err(e) if e.raw_os_error() == Some(libc::EIO) => return Ok(()),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        if stopped.load(Ordering::Acquire) {
                            return Ok(());
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => return Err(e),
                }
            }
        });
        Self {
            master,
            slave: slave.into(),
            child: None,
            raw_observed: None,
            controlling: !cfg!(target_os = "macos"),
            size: (cols.into(), rows.into()),
            output,
            stop,
            reader: Some(reader),
        }
    }
    fn spawn(&mut self, cmd: Command) {
        self.spawn_with_output(
            cmd,
            self.slave.try_clone().unwrap(),
            self.slave.try_clone().unwrap().into(),
        );
    }
    fn spawn_with_output(&mut self, mut cmd: Command, output: File, error: Stdio) {
        assert!(self.child.is_none());
        cmd.stdin(self.slave.try_clone().unwrap())
            .stdout(output)
            .stderr(error);
        let controlling = self.controlling;
        // SAFETY: after fork, only allocation-free OS calls run. Command has already duped
        // the owned slave onto fd 0; borrow_raw neither closes nor takes ownership of it.
        unsafe {
            cmd.pre_exec(move || {
                rustix::process::setsid()?;
                if controlling {
                    rustix::process::ioctl_tiocsctty(BorrowedFd::borrow_raw(0))?;
                }
                Ok(())
            });
        }
        self.child = Some(Process(cmd.spawn().unwrap()));
    }
    fn start(cmd: Command, cols: u16, rows: u16) -> Self {
        let mut pty = Self::open(cols, rows);
        pty.spawn(cmd);
        pty
    }
    fn bytes(&self) -> Vec<u8> {
        self.output.lock().unwrap().clone()
    }
    fn text_from(&self, offset: usize) -> String {
        let bytes = self.bytes();
        screen(&bytes, self.size, offset)
    }
    fn wait_from(&self, offset: usize, needle: &str, timeout: Duration) {
        let end = Instant::now() + timeout;
        loop {
            let text = self.text_from(offset);
            if text.contains(needle) {
                return;
            }
            assert!(
                Instant::now() < end,
                "missing {needle:?}; pty output:\n{text}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn wait_for(&self, needle: &str, timeout: Duration) {
        self.wait_from(0, needle, timeout);
    }
    fn ready(&mut self) {
        self.wait_for("Fix the flaky parser test", START);
        self.wait_for("e27c0de", START);
        self.wait_for("▌", START); // The selected row is loaded before a prompt can submit.
        self.assert_raw_while_running();
    }
    fn assert_raw_while_running(&mut self) {
        // Record evidence for this exact fd, not merely that watch drew a frame. Every
        // restoration assertion requires this witness, including callers that skip ready().
        self.raw_observed = None;
        let end = Instant::now() + START;
        loop {
            let child = &mut self.child.as_mut().expect("watch must be spawned").0;
            assert!(
                child.try_wait().unwrap().is_none(),
                "watch exited before raw mode was observed"
            );
            let flags = termios::tcgetattr(&self.slave)
                .expect("cannot observe the slave fd while watch is running")
                .local_modes;
            if !flags.intersects(LocalModes::ICANON | LocalModes::ECHO) {
                assert!(
                    child.try_wait().unwrap().is_none(),
                    "watch exited during the raw-mode observation"
                );
                self.raw_observed = Some(self.slave.as_raw_fd());
                return;
            }
            assert!(
                Instant::now() < end,
                "never observed both ICANON and ECHO cleared on slave fd {} while watch was running; flags: {flags:?}",
                self.slave.as_raw_fd()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn send(&mut self, mut bytes: &[u8]) {
        let end = Instant::now() + Duration::from_secs(1);
        while !bytes.is_empty() {
            match self.master.write(bytes) {
                Ok(0) => panic!("pty write returned EOF"),
                Ok(n) => bytes = &bytes[n..],
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < end, "pty input blocked");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("cannot write pty: {e}"),
            }
        }
    }
    fn signal(&mut self, signal: Signal) {
        self.assert_raw_while_running();
        self.signal_process(signal);
    }
    fn signal_process(&self, signal: Signal) {
        let pid = Pid::from_raw(self.child.as_ref().unwrap().0.id() as i32).unwrap();
        rustix::process::kill_process(pid, signal).unwrap();
    }
    fn resize(&mut self, cols: u16, rows: u16) {
        self.size = (cols.into(), rows.into());
        termios::tcsetwinsize(
            &self.master,
            Winsize {
                ws_col: cols,
                ws_row: rows,
                ws_xpixel: 0,
                ws_ypixel: 0,
            },
        )
        .unwrap();
        self.signal(Signal::WINCH);
    }
    fn wait_exit(&mut self, timeout: Duration) -> i32 {
        let status = self.child.as_mut().unwrap().wait(timeout);
        self.stop_reader();
        status
            .code()
            .unwrap_or_else(|| panic!("glen terminated by a signal: {status}"))
    }
    fn stop_reader(&mut self) {
        // Drain the final teardown bytes without relying on EOF: the parent keeps a slave open.
        self.stop.store(true, Ordering::Release);
        let reader = self.reader.take().unwrap();
        poll(Duration::from_secs(1), || {
            reader.is_finished().then_some(())
        });
        reader.join().unwrap().unwrap();
    }
    fn assert_escape_teardown(&self) {
        let bytes = String::from_utf8_lossy(&self.bytes()).into_owned();
        let entered = bytes
            .rfind("\x1b[?1049h")
            .expect("alternate screen entered");
        let after = &bytes[entered + "\x1b[?1049h".len()..];
        for sequence in ["\x1b[?1049l", "\x1b[?25h", "\x1b[?2004l"] {
            assert!(after.contains(sequence), "missing teardown {sequence:?}");
        }
        assert!(bytes.contains("\x1b[?2004h"));
    }
    fn assert_raw_observed(&self) {
        assert_eq!(
            self.raw_observed,
            Some(self.slave.as_raw_fd()),
            "raw mode was never observed on this slave fd while watch was running"
        );
    }
    fn assert_restored(&mut self) {
        self.assert_raw_observed();
        assert!(
            self.child.as_mut().unwrap().0.try_wait().unwrap().is_some(),
            "watch must exit before restoration is checked"
        );
        self.assert_escape_teardown();
        assert!(
            termios::tcgetattr(&self.slave)
                .unwrap()
                .local_modes
                .contains(LocalModes::ICANON | LocalModes::ECHO),
            "raw mode was not restored"
        );
    }
    fn assert_teardown(&mut self) {
        if cfg!(target_os = "macos") && self.controlling {
            // This is the approved escape-only smoke, paired with a non-controlling run.
            // macOS revokes this fd on exit; it cannot make a termios-restoration claim.
            self.assert_raw_observed();
            self.assert_escape_teardown();
        } else {
            self.assert_restored();
        }
    }
    fn quit(&mut self) {
        self.assert_raw_while_running();
        self.send(b"q");
        assert_eq!(self.wait_exit(EXIT), 0);
        self.assert_teardown();
    }
}
impl Drop for Pty {
    fn drop(&mut self) {
        drop(self.child.take());
        self.stop.store(true, Ordering::Release);
        if let Some(reader) = self.reader.take() {
            let end = Instant::now() + Duration::from_secs(1);
            while !reader.is_finished() && Instant::now() < end {
                thread::sleep(Duration::from_millis(5));
            }
            if reader.is_finished() {
                let result = reader.join();
                if !thread::panicking() {
                    result.unwrap().unwrap();
                }
            } else if !thread::panicking() {
                panic!("pty reader did not stop");
            }
        }
    }
}

fn command(b: &OmpBox, args: &[&str]) -> Command {
    let mut cmd = b.command(args, &b.alpha());
    cmd.env("TERM", "xterm-256color");
    cmd
}
fn pipe_output(mut cmd: Command) -> Output {
    let mut process = Process(
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    drop(process.0.stdin.take());
    let status = process.wait(START);
    let mut stdout = vec![];
    let mut stderr = vec![];
    process
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut stdout)
        .unwrap();
    process
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_end(&mut stderr)
        .unwrap();
    Output {
        status,
        stdout,
        stderr,
    }
}
fn append(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(bytes).unwrap();
    file.flush().unwrap();
}

#[test]
fn non_tty_gate_is_json_or_plain_human_and_creates_no_state() {
    let b = OmpBox::new();
    let out = pipe_output(command(&b, &["watch"]));
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stderr.is_empty());
    assert!(!out.stdout.contains(&27));
    let error = json(&out);
    assert_eq!(error["ok"], false);
    assert_eq!(error["error"]["code"], "usage");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("interactive terminal")
    );
    assert!(error["error"]["hint"].as_str().unwrap().contains("tail -f"));
    b.clean();
    let out = pipe_output(command(&b, &["watch", "--human"]));
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert!(out.stderr.starts_with(b"error:"));
    assert!(!out.stderr.contains(&27));
    b.clean();
}

#[test]
fn dumb_term_and_explicit_json_refuse_before_alternate_screen() {
    let b = OmpBox::new();
    for (args, dumb) in [(&["watch"][..], true), (&["watch", "--json"][..], false)] {
        let mut cmd = command(&b, args);
        if dumb {
            cmd.env("TERM", "dumb");
        }
        let mut pty = Pty::start(cmd, 120, 40);
        assert_eq!(pty.wait_exit(START), 2);
        pty.wait_for("interactive terminal", START);
        assert!(!pty.bytes().windows(8).any(|s| s == b"\x1b[?1049h"));
        // Gate refusals must never enter raw mode; this checks preservation, not restoration.
        assert!(
            termios::tcgetattr(&pty.slave)
                .unwrap()
                .local_modes
                .contains(LocalModes::ICANON | LocalModes::ECHO)
        );
        b.clean();
    }
}

#[test]
fn start_and_quit_restores_terminal_without_creating_home() {
    let b = OmpBox::new();
    for controlling in terminal_modes() {
        let mut pty = Pty::open(120, 40);
        pty.controlling = controlling;
        pty.spawn(command(&b, &["watch", "e27c0de"]));
        pty.ready();
        pty.quit();
        b.clean();
    }
}
#[test]
fn explicit_color_always_overrides_no_color_for_real_frame_writes() {
    let b = OmpBox::new();
    for (choice, colored) in [("always", true), ("auto", false), ("never", false)] {
        let mut cmd = command(&b, &["watch", "e27c0de", "--color", choice]);
        cmd.env("NO_COLOR", "1").env("COLORTERM", "truecolor");
        let mut pty = Pty::start(cmd, 120, 40);
        pty.ready();
        let output = pty.bytes();
        assert_eq!(
            output
                .windows(b"\x1b[38;2;".len())
                .any(|run| run == b"\x1b[38;2;"),
            colored,
            "--color {choice} did not honor Glen's resolved precedence"
        );
        pty.quit();
        b.clean();
    }
}

#[test]
fn separate_placeholder_row_keeps_the_draft_empty_for_ctrl_d() {
    let b = OmpBox::new();
    for controlling in terminal_modes() {
        let mut pty = Pty::open(60, 20);
        pty.controlling = controlling;
        pty.spawn(command(&b, &["watch", "e27c0de"]));
        pty.wait_for("tab to write to e27c0de", START);
        pty.assert_raw_while_running();
        pty.send(b"\t");
        let wait_empty_prompt = |pty: &Pty| {
            poll(DATA, || {
                let shown = pty.text_from(0);
                let lines = shown.lines().map(str::trim_end).collect::<Vec<_>>();
                lines
                    .windows(2)
                    .any(|rows| rows == ["message e27c0de · enter send", "e27c0de ❯"])
                    .then_some(())
            });
        };
        wait_empty_prompt(&pty);
        pty.send(b"abc");
        pty.wait_for("e27c0de ❯ abc", DATA);
        pty.send(b"\x04");
        // Queue another edit after Ctrl-D: its appearance proves the non-empty draft survived.
        pty.send(b"d");
        pty.wait_for("e27c0de ❯ abcd", DATA);
        pty.send(b"\x05\x7f\x7f\x7f\x7f");
        wait_empty_prompt(&pty);
        pty.send(b"\x04");
        assert_eq!(pty.wait_exit(EXIT), 0);
        pty.assert_teardown();
        b.clean();
    }
}

fn terminal_modes() -> Vec<bool> {
    if cfg!(target_os = "macos") {
        vec![true, false]
    } else {
        vec![true]
    }
}

#[test]
#[should_panic(expected = "raw mode was never observed on this slave fd while watch was running")]
fn restoration_rejects_a_cooked_slave_without_live_raw_evidence() {
    let b = OmpBox::new();
    let mut pty = Pty::start(command(&b, &["watch", "e27c0de"]), 120, 40);
    pty.ready();
    pty.quit();
    b.clean();
    // The after-state and escape teardown are correct, but neither substitutes for (a).
    pty.raw_observed = None;
    pty.assert_restored();
}

#[test]
fn restoration_rejects_raw_evidence_from_a_different_slave_fd() {
    let b = OmpBox::new();
    let mut pty = Pty::start(command(&b, &["watch", "e27c0de"]), 120, 40);
    pty.ready();
    pty.quit();
    let mut other = Pty::open(120, 40);
    std::mem::swap(&mut pty.slave, &mut other.slave);
    let error = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pty.assert_restored()))
        .expect_err("a different cooked slave must not satisfy restoration");
    std::mem::swap(&mut pty.slave, &mut other.slave);
    let message = error
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| error.downcast_ref::<&str>().copied())
        .unwrap();
    assert!(
        message.contains("raw mode was never observed on this slave fd while watch was running")
    );
    b.clean();
}

#[test]
fn resizing_switches_narrow_wide_and_too_small_layouts() {
    let b = OmpBox::new();
    let mut pty = Pty::start(command(&b, &["watch", "e27c0de"]), 60, 20);
    pty.wait_for("e27c0de ·", START);
    pty.wait_for("sessions", START);
    let offset = pty.bytes().len();
    pty.resize(120, 40);
    pty.wait_from(offset, "sessions ·", DATA);
    pty.wait_from(offset, "│", DATA);
    pty.wait_from(offset, "pending", DATA);
    let offset = pty.bytes().len();
    pty.resize(30, 8);
    poll(DATA, || {
        let shown = pty
            .text_from(offset)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        (shown == "glen watch needs at least 40×10 · now 30×8").then_some(())
    });
    pty.quit();
    b.clean();
}

/// F11: crossterm registers its SIGWINCH handler on the first event poll, which the input
/// thread makes. A resize that lands before that thread runs (forced here with a 1 s delay, as
/// a loaded CI host can) was lost: the next draw filled the resized screen with the old layout
/// and the wide layout never came.
#[test]
fn a_resize_before_the_input_thread_runs_still_switches_the_layout() {
    let b = OmpBox::new();
    let mut cmd = command(&b, &["watch", "e27c0de"]);
    cmd.env("GLEN_WATCH_TEST_INPUT_DELAY_MS", "1000");
    let mut pty = Pty::start(cmd, 60, 20);
    pty.wait_for("e27c0de ·", START);
    pty.wait_for("sessions", START);
    let offset = pty.bytes().len();
    pty.resize(120, 40);
    pty.wait_from(offset, "pending", DATA);
    pty.quit();
    b.clean();
}

fn signal_exit(signal: Signal, code: i32) {
    let b = OmpBox::new();
    for controlling in terminal_modes() {
        let mut pty = Pty::open(120, 40);
        pty.controlling = controlling;
        pty.spawn(command(&b, &["watch", "e27c0de"]));
        pty.ready();
        pty.signal(signal);
        assert_eq!(pty.wait_exit(EXIT), code);
        pty.assert_teardown();
        b.clean();
    }
}
#[test]
fn sigterm_restores_and_exits_143() {
    signal_exit(Signal::TERM, 143);
}
#[test]
fn sigint_restores_and_exits_130() {
    signal_exit(Signal::INT, 130);
}
#[test]
fn sighup_restores_and_exits_129() {
    signal_exit(Signal::HUP, 129);
}

#[test]
fn transcript_streaming_waits_for_the_complete_line() {
    let b = OmpBox::new();
    let path = b.session('A');
    let mut pty = Pty::start(command(&b, &["watch", "e27c0de"]), 120, 40);
    pty.ready();
    let message = |id, parent, text| {
        j!({"type":"message","id":id,"parentId":parent,"timestamp":"2026-10-30T12:00:00Z","message":{"role":"user","content":text}}).to_string()
    };
    let first = message("10000010", "10000009", "watch complete line arrived");
    append(&path, format!("{first}\n").as_bytes());
    pty.wait_for("watch complete line arrived", DATA);
    let partial = message("10000011", "10000010", "watch partial line arrived");
    append(&path, partial.as_bytes());
    // At least two follower polling periods pass; the complete JSON still lacks its newline.
    let end = Instant::now() + Duration::from_millis(650);
    while Instant::now() < end {
        assert!(!pty.text_from(0).contains("watch partial line arrived"));
        thread::sleep(Duration::from_millis(20));
    }
    append(&path, b"\n");
    pty.wait_for("watch partial line arrived", DATA);
    pty.quit();
    b.clean();
}

#[test]
fn pending_tools_headers_and_same_batch_pairing_match_show_tail_and_watch() {
    let b = OmpBox::new();
    let path = b.session('A');
    let mut pty = Pty::start(command(&b, &["watch", "e27c0de"]), 160, 50);
    pty.ready();
    let call = j!({"type":"message", "id":"watch-tools", "parentId":"10000009", "message":{
        "role":"assistant", "provider":"vendor", "model":"stream-model", "stopReason":"toolUse",
        "content":[{"type":"toolCall", "id":"watch-read", "name":"read", "arguments":{"path":"pending.rs"}},
                   {"type":"toolCall", "id":"watch-reply", "name":"glen_reply", "arguments":{"text":"awaiting reply"}}]
    }});
    append(&path, format!("{call}\n").as_bytes());
    pty.wait_for("read pending.rs · no result", DATA);
    pty.wait_for("glen_reply \"awaiting reply\" · no result", DATA);
    poll(DATA, || {
        pty.text_from(0)
            .lines()
            .nth(1)
            .is_some_and(|line| line.contains("stream-model"))
            .then_some(())
    });
    for name in ["show", "tail"] {
        let out = b.glen(&[name, "e27c0de", "--human"], &b.alpha());
        assert!(out.status.success());
        let output = String::from_utf8(out.stdout).unwrap();
        assert!(output.contains("read pending.rs · no result"), "{output}");
        assert!(
            output.contains("glen_reply \"awaiting reply\" · no result"),
            "{output}"
        );
        assert!(!output.contains("· · no result"));
    }
    let results = [
        j!({"type":"message", "id":"watch-read-result", "parentId":"watch-tools", "message":{
            "role":"toolResult", "toolCallId":"watch-read", "toolName":"read", "content":"streamed read result", "isError":false}}),
        j!({"type":"message", "id":"watch-reply-result", "parentId":"watch-read-result", "message":{
            "role":"toolResult", "toolCallId":"watch-reply", "toolName":"glen_reply", "content":"delivered talkback", "isError":false}}),
    ].into_iter().map(|entry| format!("{entry}\n")).collect::<String>();
    append(&path, results.as_bytes());
    pty.wait_for("streamed read result", DATA);
    pty.wait_for("delivered talkback", DATA);
    // Calls already streamed stay on screen; later results are standalone (AC-504).
    assert_eq!(pty.text_from(0).matches("no result").count(), 2);
    assert!(!pty.text_from(0).contains("· · no result"));
    for name in ["show", "tail"] {
        let out = b.glen(&[name, "e27c0de", "--human"], &b.alpha());
        assert!(out.status.success());
        let output = String::from_utf8(out.stdout).unwrap();
        assert!(
            !output.contains("no result"),
            "within-batch results must pair: {output}"
        );
        assert!(output.contains("streamed read result"));
        assert!(output.contains("delivered talkback"));
    }
    let old_title = "Fix the flaky parser test";
    let title = "Updated watch tab";
    let replacement = format!("{title}{}", " ".repeat(old_title.len() - title.len()));
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace(old_title, &replacement),
    )
    .unwrap();
    pty.send(b"r");
    poll(START, || {
        pty.text_from(0)
            .lines()
            .nth(1)
            .is_some_and(|line| line.contains(title))
            .then_some(())
    });
    pty.quit();
    b.clean();
}

#[test]
fn same_inode_rewrite_is_noticed_and_fresh_content_is_visible() {
    let b = OmpBox::new();
    let path = b.session('A');
    let mut pty = Pty::start(command(&b, &["watch", "e27c0de"]), 120, 40);
    pty.ready();
    let before = fs::metadata(&path).unwrap();
    let mut content = fs::read_to_string(&path)
        .unwrap()
        .replace("10000007", "90000007")
        .replace(
            "The test depends on map order. Fixed.",
            "watch rewritten answer is visible",
        );
    content.push_str(&format!(
        "{}\n",
        j!({"type":"message","id":"watch-rewrite","parentId":"10000009",
        "message":{"role":"user","content":"after the in-place rewrite"}})
    ));
    assert!(content.len() as u64 >= before.len());
    fs::write(&path, content).unwrap();
    use std::os::unix::fs::MetadataExt;
    assert_eq!(before.ino(), fs::metadata(&path).unwrap().ino());
    pty.wait_for("── session file rewritten ──", DATA);
    pty.wait_for("watch rewritten answer is visible", DATA);
    pty.wait_for("after the in-place rewrite", DATA);
    pty.quit();
    b.clean();
}

#[test]
fn equal_size_early_prefix_rewrite_reaches_watch_with_an_unchanged_tail_window() {
    let b = OmpBox::new();
    let path = b.session('A');
    let original = fs::read_to_string(&path).unwrap()
        + &format!(
            "{}\n",
            j!({"type":"future", "id":"padding", "parentId":"10000009", "data":"x".repeat(9000)})
        );
    fs::write(&path, &original).unwrap();
    let mut pty = Pty::start(command(&b, &["watch", "e27c0de"]), 120, 40);
    pty.ready();
    let before = fs::metadata(&path).unwrap();
    let rewritten = original
        .replace("10000003", "90000003")
        .replace("Why does parser_test flake?", "prefix rewrite detected    ");
    assert_eq!(original.len(), rewritten.len());
    assert_eq!(
        &original.as_bytes()[original.len() - 4096..],
        &rewritten.as_bytes()[rewritten.len() - 4096..]
    );
    fs::write(&path, rewritten).unwrap();
    File::open(&path)
        .unwrap()
        .set_modified(before.modified().unwrap() + Duration::from_secs(1))
        .unwrap();
    use std::os::unix::fs::MetadataExt;
    assert_eq!(before.ino(), fs::metadata(&path).unwrap().ino());
    pty.wait_for("── session file rewritten ──", DATA);
    pty.wait_for("prefix rewrite detected", DATA);
    append(&path, format!("{}\n", j!({"type":"message", "id":"after-prefix", "parentId":"padding", "message":{"role":"user", "content":"normal append after prefix rewrite"}})).as_bytes());
    pty.wait_for("normal append after prefix rewrite", DATA);
    pty.quit();
    b.clean();
}
#[test]
fn v1_rewrite_rebases_watch_occurrences_so_the_next_duplicate_is_visible() {
    let b = OmpBox::new();
    let path = b.session('A');
    let header = include_str!("fixtures/v1-session.jsonl")
        .lines()
        .next()
        .unwrap()
        .replace("legacy-v1", A_ID);
    let a = format!(
        "{}\n",
        j!({"type":"message", "message":{"role":"user", "content":"old prompt"}})
    );
    let other = a.replace("old prompt", "another prompt");
    fs::write(&path, format!("{header}\n{a}{a}{other}")).unwrap();
    let mut pty = Pty::start(command(&b, &["watch", "e27c0de"]), 120, 40);
    pty.wait_for("another prompt", START);
    pty.assert_raw_while_running();
    assert_eq!(pty.text_from(0).matches("old prompt").count(), 2);
    fs::write(&path, format!("{header}\n{a}{other}")).unwrap();
    pty.wait_for("── session file rewritten ──", DATA);
    assert_eq!(pty.text_from(0).matches("old prompt").count(), 2);
    append(&path, a.as_bytes());
    poll(DATA, || {
        (pty.text_from(0).matches("old prompt").count() == 3).then_some(())
    });
    assert_eq!(pty.text_from(0).matches("another prompt").count(), 1);
    pty.quit();
    b.clean();
}

#[test]
fn v1_rewrite_does_not_duplicate_messages_when_ordinals_shift() {
    let b = OmpBox::new();
    let path = b.session('A');
    let original = include_str!("fixtures/v1-session.jsonl").replace("legacy-v1", A_ID);
    fs::write(&path, &original).unwrap();
    let mut pty = Pty::start(command(&b, &["watch", "e27c0de"]), 120, 40);
    pty.wait_for("old answer", START);
    pty.wait_for("old prompt", START);
    pty.assert_raw_while_running();
    let (header, entries) = original.split_once('\n').unwrap();
    let fresh = "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"watch fresh after rewrite\"}}\n";
    let staged = path.with_extension("staged");
    fs::write(
        &staged,
        format!("{header}\n{{\"type\":\"future_metadata\"}}\n{entries}{fresh}"),
    )
    .unwrap();
    fs::rename(staged, &path).unwrap();
    pty.wait_for("── session file rewritten ──", DATA);
    pty.wait_for("watch fresh after rewrite", DATA);
    let shown = pty.text_from(0);
    assert_eq!(shown.matches("old prompt").count(), 1);
    assert_eq!(shown.matches("old answer").count(), 1);
    pty.quit();
    b.clean();
}

#[test]
fn inbox_talk_back_is_visible_without_marking_read() {
    let b = OmpBox::new();
    let mut pty = Pty::start(command(&b, &["watch", "e27c0de"]), 120, 40);
    pty.ready();
    let dir = b.glen_home.join("inbox");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{A_ID}.ndjson"));
    File::create(&path).unwrap();
    let record = j!({"v":1,"id":"watch-talk-back","session_id":A_ID,"job_id":null,"request_id":null,"tool_call_id":null,"text":"watch inbox talk-back arrived","final":true,"at":"2026-10-30T12:00:00Z"});
    append(&path, format!("{record}\n").as_bytes());
    pty.wait_for("watch inbox talk-back arrived", DATA);
    assert!(!dir.join(".read").exists());
    pty.quit();
    assert!(!dir.join(".read").exists());
    assert!(!b.home.join(".omp").exists());
}

#[test]
#[ignore = "child entry point for common::Holder"]
fn lease_holder() {
    common::hold_lease_from_env();
}

#[test]
fn held_session_refuses_with_hint_and_creates_no_job() {
    let b = RpcBox::new();
    fs::create_dir_all(b.owners()).unwrap();
    let lock = b.owners().join(format!("{A_ID}.lock"));
    File::create(&lock).unwrap();
    let mut holder = Holder::new(&lock);
    let mut cmd = b.glen(&["watch", "e27c0de"]);
    cmd.env("TERM", "xterm-256color");
    let mut pty = Pty::start(cmd, 120, 40);
    pty.ready();
    pty.send(b"\theld route test\r");
    pty.wait_for("open in another omp process", START);
    pty.wait_for("glen install-extension", DATA);
    assert!(!b.omp.glen_home.join("jobs").exists());
    assert!(b.fake_log().is_empty());
    pty.send(b"\x1b[Z");
    pty.quit();
    holder.release();
    assert!(!b.omp.glen_home.exists());
    assert!(lock.exists(), "the test peer owns this lease file");
}

/// Durable workers outlive watch. Bound cleanup independently and verify both the worker
/// identity and fake omp process group are gone before RpcBox's fixture directory is removed.
struct Workers<'a>(&'a RpcBox);
impl Workers<'_> {
    fn gone(&self) -> bool {
        let workers = fs::read_dir(self.0.omp.glen_home.join("jobs"))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .all(|entry| {
                let status = fs::read(entry.path().join("status.json"))
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
                status.is_some_and(|s| {
                    s["worker_pid"].as_u64().map_or_else(
                        || matches!(s["status"].as_str(), Some("failed" | "lost" | "aborted")),
                        |pid| {
                            matches!(
                                glen::jobs::liveness::start_token(pid as u32),
                                glen::jobs::liveness::Probe::Gone
                            )
                        },
                    )
                })
            });
        let peers = self
            .0
            .fake_log()
            .iter()
            .filter(|v| v["event"] == "start")
            .all(|v| {
                let pid = Pid::from_raw(v["pgid"].as_i64().unwrap() as i32).unwrap();
                rustix::process::test_kill_process_group(pid) == Err(rustix::io::Errno::SRCH)
            });
        workers && peers
    }
}
impl Drop for Workers<'_> {
    fn drop(&mut self) {
        // Test commands use a 5s job deadline, including failure paths during assertions.
        let end = Instant::now() + Duration::from_secs(10);
        while !self.gone() && Instant::now() < end {
            thread::sleep(Duration::from_millis(20));
        }
        if !self.gone() {
            // A broken deadline must not leave a process holding a deleted fixture directory.
            // Only kill workers whose persisted start identity still matches this test's job.
            for entry in fs::read_dir(self.0.omp.glen_home.join("jobs"))
                .into_iter()
                .flatten()
            {
                let entry = entry.unwrap();
                let Ok(bytes) = fs::read(entry.path().join("status.json")) else {
                    continue;
                };
                let s: glen::jobs::Status = serde_json::from_slice(&bytes).unwrap();
                if let (Some(pid), Some(start)) = (s.worker_pid, s.worker_start.as_deref())
                    && glen::jobs::liveness::check(pid, start)
                        == glen::jobs::liveness::Liveness::Alive
                {
                    if let Some(omp) = s.omp_pid.and_then(|pid| Pid::from_raw(pid as i32)) {
                        match rustix::process::kill_process_group(omp, Signal::KILL) {
                            Ok(()) | Err(rustix::io::Errno::SRCH) => (),
                            Err(e) => panic!("cannot kill test omp process group: {e}"),
                        }
                    }
                    match rustix::process::kill_process(
                        Pid::from_raw(pid as i32).unwrap(),
                        Signal::KILL,
                    ) {
                        Ok(()) | Err(rustix::io::Errno::SRCH) => (),
                        Err(e) => panic!("cannot kill test worker: {e}"),
                    }
                }
            }
            let end = Instant::now() + Duration::from_secs(3);
            while !self.gone() && Instant::now() < end {
                thread::sleep(Duration::from_millis(20));
            }
            // Avoid a second panic aborting the whole integration-test binary during unwinding.
            if !thread::panicking() {
                panic!(
                    "fixture worker survived its deadline; forced cleanup complete: {}",
                    self.gone()
                );
            }
        }
    }
}
fn completed_job(b: &RpcBox, text: &str) -> Value {
    poll(Duration::from_secs(10), || {
        for entry in fs::read_dir(b.omp.glen_home.join("jobs")).ok()? {
            let dir = entry.unwrap().path();
            let job: Value = serde_json::from_slice(&fs::read(dir.join("job.json")).ok()?).unwrap();
            if job["text"] == text {
                let status: Value =
                    serde_json::from_slice(&fs::read(dir.join("status.json")).ok()?).unwrap();
                assert!(
                    !matches!(
                        status["status"].as_str(),
                        Some("failed" | "lost" | "aborted")
                    ),
                    "{status}"
                );
                if status["status"] == "completed" {
                    return Some(job);
                }
            }
        }
        None
    })
}

#[test]
fn rpc_send_steer_then_auto_shows_replies_and_completed_jobs() {
    let b = RpcBox::new();
    let _workers = Workers(&b);
    let mut cmd = b.glen(&["watch", "e27c0de"]);
    cmd.env("TERM", "xterm-256color")
        .env("GLEN_JOB_TIMEOUT", "5s");
    let mut pty = Pty::start(cmd, 120, 40);
    pty.ready();
    pty.send(b"\t\x14watch steer message\r");
    let job = completed_job(&b, "watch steer message");
    assert_eq!(job["route"], "rpc");
    assert_eq!(job["deliver_as"], "steer");
    pty.wait_for("reply: watch steer message", START);
    pty.wait_for("completed", START);
    pty.send(b"watch auto message\r");
    let job = completed_job(&b, "watch auto message");
    assert_eq!(job["route"], "rpc");
    assert_eq!(job["deliver_as"], "auto");
    pty.wait_for("reply: watch auto message", START);
    pty.send(b"\x1b[Z");
    pty.quit();
    assert!(!b.omp.glen_home.join("inbox/.read").exists());
}

#[test]
fn live_route_send_uses_the_existing_extension_peer() {
    let mut b = LiveBox::new();
    b.start_ext(A_ID, "ok", &[]);
    let _workers = Workers(&b.rpc);
    let mut cmd = b.rpc.glen(&["watch", "e27c0de"]);
    cmd.env("TERM", "xterm-256color")
        .env("GLEN_OMP_BIN", "/nonexistent")
        .env("GLEN_JOB_TIMEOUT", "5s");
    let mut pty = Pty::start(cmd, 120, 40);
    pty.ready();
    pty.send(b"\twatch live message\r");
    let job = completed_job(&b.rpc, "watch live message");
    assert_eq!(job["route"], "live");
    pty.wait_for("reply: watch live message", START);
    pty.wait_for("completed", START);
    pty.send(b"\x1b[Z");
    pty.quit();
    assert!(b.rpc.fake_log().is_empty());
    assert!(!b.rpc.omp.glen_home.join("inbox/.read").exists());
}

#[test]
fn failed_frame_write_with_dead_stdout_and_stderr_exits_1_without_panic() {
    for stderr_on_tty in [true, false] {
        let b = OmpBox::new();
        let mut input = Pty::open(120, 40);
        input.controlling = false; // Keep the original stdin slave inspectable on macOS.
        let mut output = Pty::open(120, 40);
        let error = if stderr_on_tty {
            output.slave.try_clone().unwrap().into()
        } else {
            Stdio::piped()
        };
        input.spawn_with_output(
            command(&b, &["watch", "e27c0de"]),
            output.slave.try_clone().unwrap(),
            error,
        );
        output.wait_for("Fix the flaky parser test", START);
        input.assert_raw_while_running();
        output.stop_reader();
        drop(output); // Close every master reference; both output descriptors now fail writes.
        input.send(b"\t");
        let code = input.wait_exit(START);
        let mut error = String::new();
        if let Some(mut pipe) = input.child.as_mut().unwrap().0.stderr.take() {
            pipe.read_to_string(&mut error).unwrap();
        }
        assert_eq!(code, 1, "stderr_on_tty={stderr_on_tty}; stderr: {error}");
        assert!(!error.contains("panicked"), "{error}");
        input.assert_raw_observed();
        assert!(
            termios::tcgetattr(&input.slave)
                .unwrap()
                .local_modes
                .contains(LocalModes::ICANON | LocalModes::ECHO)
        );
        if !stderr_on_tty {
            assert!(error.contains("cannot draw terminal frame"), "{error}");
        }
        b.clean();
    }
}

#[cfg(debug_assertions)]
#[test]
fn ui_and_worker_panics_restore_before_reporting_and_exit_101() {
    let b = OmpBox::new();
    for knob in ["ui:800", "worker:800"] {
        let mut cmd = command(&b, &["watch", "e27c0de"]);
        cmd.env("GLEN_WATCH_TEST_PANIC", knob);
        let mut pty = Pty::start(cmd, 120, 40);
        pty.ready();
        assert_eq!(pty.wait_exit(START), 101, "{knob}");
        let bytes = String::from_utf8_lossy(&pty.bytes()).into_owned();
        let entered = bytes.rfind("\x1b[?1049h").unwrap();
        let restored = entered + bytes[entered..].find("\x1b[?1049l").unwrap();
        let message = bytes.find("panicked").unwrap();
        assert!(
            restored < message,
            "{knob}: panic printed before restore: {bytes}"
        );
        assert!(bytes[message..].contains("GLEN_WATCH_TEST_PANIC"));
        pty.assert_restored();
        b.clean();
    }
}

#[cfg(debug_assertions)]
#[test]
fn second_signal_interrupts_the_wait_for_an_in_flight_submit() {
    let b = RpcBox::new();
    let gate = b.omp.tmp.path().join("held-probe");
    let ready = gate.with_file_name("held-probe.ready");
    let mut cmd = b.glen(&["watch", "e27c0de"]);
    cmd.env("TERM", "xterm-256color")
        .env("FAKE_OMP_LEASE_PROBE_GATE", &gate);
    let mut pty = Pty::start(cmd, 120, 40);
    pty.ready();
    pty.send(b"\tin flight\r");
    poll(START, || ready.exists().then_some(()));
    pty.signal(Signal::TERM);
    poll(DATA, || {
        (pty.bytes().windows(8).any(|b| b == b"\x1b[?1049l")
            && termios::tcgetattr(&pty.slave)
                .unwrap()
                .local_modes
                .contains(LocalModes::ICANON | LocalModes::ECHO))
        .then_some(())
    });
    assert!(
        pty.child.as_mut().unwrap().0.try_wait().unwrap().is_none(),
        "first signal must wait for the held submit"
    );
    let second = Instant::now();
    pty.signal_process(Signal::INT); // The first signal already restored cooked mode.
    assert_eq!(pty.wait_exit(EXIT), 130);
    assert!(
        second.elapsed() < EXIT,
        "second signal did not exit within 1s"
    );
    pty.assert_restored();
    assert!(!gate.exists());
    b.omp.clean();
}

#[cfg(debug_assertions)]
#[test]
fn back_to_back_signals_exit_within_one_second_and_finish_teardown() {
    for controlling in terminal_modes() {
        // Exercise the scheduling race repeatedly without pausing between the two sends.
        for _ in 0..4 {
            let b = RpcBox::new();
            let gate = b.omp.tmp.path().join("rapid-probe");
            let ready = gate.with_file_name("rapid-probe.ready");
            let mut cmd = b.glen(&["watch", "e27c0de"]);
            cmd.env("TERM", "xterm-256color")
                .env("FAKE_OMP_LEASE_PROBE_GATE", &gate);
            let mut pty = Pty::open(120, 40);
            pty.controlling = controlling;
            pty.spawn(cmd);
            pty.ready();
            pty.send(b"\tin flight\r");
            poll(START, || ready.exists().then_some(()));
            let signalled = Instant::now();
            pty.signal_process(Signal::TERM);
            pty.signal_process(Signal::INT);
            let code = pty.wait_exit(EXIT);
            assert!(signalled.elapsed() < EXIT, "rapid signals took at least 1s");
            // Distinct pending signals can be yielded in either order by the OS/iterator.
            assert!([130, 143].contains(&code), "unexpected signal exit: {code}");
            pty.assert_teardown();
            let bytes = pty.bytes();
            let leave = b"\x1b[?2004l\x1b[?1049l\x1b[?25h";
            assert_eq!(
                bytes
                    .windows(leave.len())
                    .filter(|bytes| *bytes == leave)
                    .count(),
                1,
                "teardown must be one complete, uninterrupted escape sequence: {bytes:?}"
            );
            assert!(!gate.exists());
            b.omp.clean();
        }
    }
}

#[test]
#[cfg(debug_assertions)]
fn repeated_sigint_interrupts_an_in_flight_submit_after_observed_delivery() {
    for controlling in terminal_modes() {
        for _ in 0..4 {
            let b = RpcBox::new();
            let gate = b.omp.tmp.path().join("identical-probe");
            let ready = gate.with_file_name("identical-probe.ready");
            let mut cmd = b.glen(&["watch", "e27c0de"]);
            cmd.env("TERM", "xterm-256color")
                .env("FAKE_OMP_LEASE_PROBE_GATE", &gate);
            let mut pty = Pty::open(120, 40);
            pty.controlling = controlling;
            pty.spawn(cmd);
            pty.ready();
            pty.send(b"\tin flight\r");
            poll(START, || ready.exists().then_some(()));
            let signalled = Instant::now();
            pty.signal_process(Signal::INT);
            // Standard signals may coalesce in the kernel before any handler runs.
            // Observe the first delivery before sending the same number again; the
            // unit counter test covers two deliveries before the watcher can poll.
            poll(EXIT, || {
                (pty.bytes().windows(8).any(|b| b == b"\x1b[?1049l")
                    && termios::tcgetattr(&pty.slave)
                        .unwrap()
                        .local_modes
                        .contains(LocalModes::ICANON | LocalModes::ECHO))
                .then_some(())
            });
            assert!(
                pty.child.as_mut().unwrap().0.try_wait().unwrap().is_none(),
                "first SIGINT must still wait for the held submit"
            );
            pty.signal_process(Signal::INT);
            assert_eq!(pty.wait_exit(EXIT), 130);
            assert!(
                signalled.elapsed() < EXIT,
                "identical signals took at least 1s"
            );
            pty.assert_teardown();
            let bytes = pty.bytes();
            let leave = b"\x1b[?2004l\x1b[?1049l\x1b[?25h";
            assert_eq!(
                bytes
                    .windows(leave.len())
                    .filter(|bytes| *bytes == leave)
                    .count(),
                1,
                "teardown must finish without interleaving: {bytes:?}"
            );
            assert!(!gate.exists());
            b.omp.clean();
        }
    }
}

#[test]
fn hung_endpoint_keeps_focus_responsive_refuses_and_bounds_quit() {
    let mut b = LiveBox::new();
    b.start_ext(A_ID, "hang", &[]);
    let mut cmd = b.rpc.glen(&["watch", "e27c0de"]);
    cmd.env("TERM", "xterm-256color");
    let mut pty = Pty::start(cmd, 120, 40);
    pty.ready();
    let submitted = Instant::now();
    pty.send(b"\thung route\r\t");
    pty.wait_for("▍sessions", EXIT);
    assert!(submitted.elapsed() < EXIT, "Tab waited for the endpoint");
    pty.wait_for(
        "cannot take messages",
        Duration::from_secs(3).saturating_sub(submitted.elapsed()),
    );
    assert!(submitted.elapsed() < Duration::from_secs(3));
    let hellos = || {
        b.ext_log()
            .iter()
            .filter(|v| v["dir"] == "in" && v["frame"]["op"] == "hello")
            .count()
    };
    let before = hellos();
    pty.send(b"\t\t\r"); // Retry the retained draft, then quit with routing still in flight.
    poll(EXIT, || (hellos() > before).then_some(()));
    pty.wait_for("sending", EXIT);
    pty.assert_raw_while_running();
    let quitting = Instant::now();
    pty.send(b"\x1b[Zq");
    assert_eq!(pty.wait_exit(Duration::from_secs(7)), 0);
    assert!(quitting.elapsed() < Duration::from_secs(7));
    pty.assert_restored();
    assert!(!b.rpc.omp.glen_home.join("jobs").exists());
}
