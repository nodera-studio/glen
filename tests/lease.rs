#![cfg(unix)]
mod common;
use common::{OmpBox, data};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
#[test]
#[ignore]
fn lease_holder() {
    let Some(path) = std::env::var_os("GLEN_TEST_HOLD_LOCK") else {
        return;
    };
    let path = PathBuf::from(path);
    #[cfg(not(target_os = "linux"))]
    let _held = {
        let f = fs::File::options()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        f.lock().unwrap();
        f
    };
    #[cfg(target_os = "linux")]
    let _held = {
        use std::os::linux::net::SocketAddrExt;
        let name = glen::omp::lease::memory_lock_name(path.to_str().unwrap());
        let addr = std::os::unix::net::SocketAddr::from_abstract_name(name).unwrap();
        std::os::unix::net::UnixDatagram::bind_addr(&addr).unwrap()
    };
    println!("held");
    std::io::stdout().flush().unwrap();
    std::io::stdin().read_to_end(&mut Vec::new()).unwrap();
}
struct Holder(Child);
impl Holder {
    fn new(path: &Path) -> Self {
        let mut c = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lease_holder", "--ignored", "--nocapture"])
            .env_clear()
            .env("GLEN_TEST_HOLD_LOCK", path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let out = c.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut ready = Some(tx);
            for l in BufReader::new(out).lines() {
                if l.unwrap() == "held"
                    && let Some(tx) = ready.take()
                {
                    tx.send(()).unwrap();
                }
            }
        });
        let holder = Self(c);
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        holder
    }
    fn release(&mut self) {
        drop(self.0.stdin.take());
        let start = Instant::now();
        loop {
            if let Some(s) = self.0.try_wait().unwrap() {
                assert!(s.success());
                return;
            }
            assert!(start.elapsed() < Duration::from_secs(3));
            thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for Holder {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
            let start = Instant::now();
            while self.0.try_wait().unwrap().is_none() {
                assert!(start.elapsed() < Duration::from_secs(3));
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
}
fn lock(sb: &OmpBox, key: char) -> PathBuf {
    let s = glen::omp::session::Session::load(&sb.session(key)).unwrap();
    let dir = sb.home.join(".omp/run/session-owners");
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join(format!("{}.lock", s.header.id));
    fs::write(&p, b"preserve").unwrap();
    p
}
#[test]
fn ac108_109_cross_process() {
    let sb = OmpBox::new();
    for (key, handle, state) in [('B', "f13a5b6", "busy"), ('A', "e27c0de", "live")] {
        let p = lock(&sb, key);
        let before = fs::metadata(&p).unwrap();
        let mut holder = Holder::new(&p);
        let d = data(&sb.glen(&["ls", "--all", "--json"], &sb.alpha()));
        let rows = d["sessions"].as_array().unwrap();
        for r in rows {
            assert_eq!(
                r["state"],
                if r["handle"] == handle { state } else { "idle" }
            );
        }
        let live = data(&sb.glen(&["ls", "--all", "--live"], &sb.alpha()));
        assert_eq!(live["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(live["sessions"][0]["handle"], handle);
        let after = fs::metadata(&p).unwrap();
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
        assert_eq!(before.len(), after.len());
        assert_eq!(fs::read(&p).unwrap(), b"preserve");
        holder.release();
        let d = data(&sb.glen(&["ls", "--all"], &sb.alpha()));
        assert!(
            d["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["state"] == "idle")
        );
    }
    assert!(!sb.glen_home.exists());
}
#[cfg(not(target_os = "linux"))]
#[test]
fn ac108_permission_is_unknown() {
    use std::os::unix::fs::PermissionsExt;
    let sb = OmpBox::new();
    let p = lock(&sb, 'B');
    let dir = p.parent().unwrap();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(&p).is_ok() {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        return;
    }
    let o = sb.glen(&["ls", "--all"], &sb.alpha());
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    let d = data(&o);
    let b = d["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["handle"] == "f13a5b6")
        .unwrap();
    assert_eq!(b["state"], "unknown");
    assert!(b["state_detail"].is_string());
}
