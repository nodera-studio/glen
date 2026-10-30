use crate::env::Env;
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
fn executable(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
pub fn find_omp(env: &Env) -> Result<PathBuf, String> {
    let name = env.non_empty("GLEN_OMP_BIN").unwrap_or("omp");
    if Path::new(name).is_absolute() {
        return if Path::new(name).is_file() {
            Ok(PathBuf::from(name))
        } else {
            Err(format!("omp binary not found: {name}"))
        };
    }
    if name.contains('/') {
        return Err("GLEN_OMP_BIN must be an absolute path or a bare executable name".into());
    }
    for dir in env.get("PATH").unwrap_or("").split(':') {
        let base = if Path::new(dir).is_absolute() {
            PathBuf::from(dir)
        } else {
            env.cwd().join(dir)
        };
        let path = base.join(name);
        if executable(&path) {
            return Ok(path);
        }
    }
    Err(format!("{name} not found on PATH"))
}
pub fn omp_version(bin: &Path, timeout: Duration) -> Result<String, String> {
    let mut child = Command::new(bin)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    // Drain both pipes while polling so a noisy child cannot fill a pipe and block exit.
    // Readers retain at most 64 KiB and have the same deadline as the child.
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "missing omp stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "missing omp stderr".to_string())?;
    let read = |mut pipe: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut saved = Vec::new();
            let mut buf = [0; 4096];
            loop {
                match pipe.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let room = 65536usize.saturating_sub(saved.len());
                        saved.extend_from_slice(&buf[..n.min(room)]);
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            saved
        })
    };
    let out = read(Box::new(stdout));
    let _err = read(Box::new(stderr));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {}
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e.to_string());
            }
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err("omp --version timed out".into());
        }
        thread::sleep(Duration::from_millis(20));
    };
    if !status.success() {
        return Err(format!("omp --version failed ({status})"));
    }
    // Do not wait on a reader if a descendant inherited the output descriptor.
    while !out.is_finished() {
        if start.elapsed() >= timeout {
            return Err("omp --version output timed out".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
    let bytes = out
        .join()
        .map_err(|_| "cannot read omp version".to_string())?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "omp version is not UTF-8".to_string())?;
    let line = text.lines().next().unwrap_or("").trim();
    let version = line.strip_prefix("omp/").unwrap_or(line).trim();
    let parts = version.split('.').collect::<Vec<_>>();
    if parts.len() != 3
        || !parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))
    {
        return Err("cannot parse omp --version".into());
    }
    Ok(version.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    fn script(dir: &Path, name: &str, text: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{text}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
    #[cfg(unix)]
    #[test]
    fn path_search_and_overrides() {
        let t = tempfile::tempdir().unwrap();
        let bin = script(t.path(), "omp", "echo omp/18.14.0");
        let path = t.path().to_str().unwrap();
        let env = Env::for_test(Some(t.path()), t.path(), &[("PATH", path)]);
        assert_eq!(find_omp(&env).unwrap(), bin);
        let override_bin = script(t.path(), "custom-omp", "echo omp/18.14.0");
        for name in ["custom-omp", override_bin.to_str().unwrap()] {
            let env = Env::for_test(
                Some(t.path()),
                t.path(),
                &[("PATH", path), ("GLEN_OMP_BIN", name)],
            );
            assert_eq!(find_omp(&env).unwrap(), override_bin);
        }
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(find_omp(&env).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn versions_and_failures() {
        let t = tempfile::tempdir().unwrap();
        for (name, body, want) in [
            ("prefix", "echo 'omp/18.14.0'", Some("18.14.0")),
            (
                "plain",
                "echo ' 18.0.9 '\necho 'ignored second line'",
                Some("18.0.9"),
            ),
            ("failed", "exit 1", None),
            ("garbage", "echo invalid", None),
            ("empty", "exit 0", None),
        ] {
            let bin = script(t.path(), name, body);
            let result = omp_version(&bin, Duration::from_secs(1));
            if let Some(want) = want {
                assert_eq!(result.unwrap(), want);
            } else {
                assert!(result.is_err());
            }
        }
    }
    #[cfg(unix)]
    #[test]
    fn version_wait_is_bounded() {
        let t = tempfile::tempdir().unwrap();
        let bin = script(t.path(), "slow", "while :; do :; done");
        let start = Instant::now();
        let result = omp_version(&bin, Duration::from_millis(60));
        assert!(result.unwrap_err().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
