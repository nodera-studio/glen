use crate::env::Env;
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
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
        let meta = fs::metadata(name).map_err(|_| format!("omp binary not found: {name}"))?;
        if !meta.is_file() {
            return Err(format!("GLEN_OMP_BIN is not a regular file: {name}"));
        }
        if !executable(Path::new(name)) {
            return Err(format!("GLEN_OMP_BIN is not executable: {name}"));
        }
        return Ok(PathBuf::from(name));
    }
    if name.contains('/') {
        return Err("GLEN_OMP_BIN must be an absolute path or a bare executable name".into());
    }
    for dir in env.get("PATH").into_iter().flat_map(std::env::split_paths) {
        if !dir.is_absolute() {
            continue;
        }
        let path = dir.join(name);
        if executable(&path) {
            return Ok(path);
        }
    }
    Err(format!("{name} not found on PATH"))
}
// The group may outlive its leader when a wrapper leaves inherited pipes open.
fn terminate(child: &mut Child) -> Result<(), String> {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        let pid = i32::try_from(child.id()).map_err(|e| e.to_string())?;
        // SAFETY: kill takes integer values, and a negative child pid targets only
        // the private process group created below. SIGKILL is 9 on supported Unix.
        if unsafe { kill(-pid, 9) } != 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(3) {
                // ESRCH: the group already exited.
                return Err(format!("cannot kill omp process group: {e}"));
            }
        }
    }
    #[cfg(not(unix))]
    child.kill().map_err(|e| e.to_string())?;
    let start = Instant::now();
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(_) => return Ok(()),
            None if start.elapsed() >= Duration::from_secs(1) => {
                return Err("timed out reaping omp".into());
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    }
}
pub fn omp_version(
    bin: &Path,
    timeout: Duration,
    profile_flag: Option<&str>,
) -> Result<String, String> {
    let mut command = Command::new(bin);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if profile_flag.is_some() {
        command.env_remove("OMP_PROFILE").env_remove("PI_PROFILE");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
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
    let err = read(Box::new(stderr));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {}
            Err(e) => {
                terminate(&mut child)?;
                return Err(e.to_string());
            }
        }
        if start.elapsed() >= timeout {
            terminate(&mut child)?;
            return Err("omp --version timed out".into());
        }
        thread::sleep(Duration::from_millis(20));
    };
    if !status.success() {
        return Err(format!("omp --version failed ({status})"));
    }
    // Do not wait on a reader if a descendant inherited the output descriptor.
    while !out.is_finished() || !err.is_finished() {
        if start.elapsed() >= timeout {
            terminate(&mut child)?;
            return Err("omp --version output timed out".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
    let bytes = out
        .join()
        .map_err(|_| "cannot read omp version".to_string())?;
    err.join()
        .map_err(|_| "cannot read omp stderr".to_string())?;
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
        assert_eq!(find_omp(&env).unwrap_err(), "omp not found on PATH");
        let env = Env::for_test(
            Some(t.path()),
            t.path(),
            &[("GLEN_OMP_BIN", bin.to_str().unwrap())],
        );
        assert_eq!(
            find_omp(&env).unwrap_err(),
            format!("GLEN_OMP_BIN is not executable: {}", bin.display())
        );
    }
    #[cfg(unix)]
    #[test]
    fn absolute_override_requires_a_regular_file() {
        let t = tempfile::tempdir().unwrap();
        for (path, message) in [
            (t.path().to_path_buf(), "GLEN_OMP_BIN is not a regular file"),
            (t.path().join("missing"), "omp binary not found"),
        ] {
            let env = Env::for_test(
                Some(t.path()),
                t.path(),
                &[("GLEN_OMP_BIN", path.to_str().unwrap())],
            );
            assert_eq!(
                find_omp(&env).unwrap_err(),
                format!("{message}: {}", path.display())
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn path_search_skips_unset_empty_and_relative_entries() {
        let t = tempfile::tempdir().unwrap();
        script(t.path(), "omp", "echo omp/18.14.0");
        let rel = t.path().join("relative");
        fs::create_dir(&rel).unwrap();
        script(&rel, "omp", "echo omp/18.14.0");
        for vars in [vec![], vec![("PATH", "")], vec![("PATH", ":.:relative:")]] {
            let env = Env::for_test(Some(t.path()), t.path(), &vars);
            assert!(find_omp(&env).is_err(), "{vars:?}");
        }
        let path = format!(":relative:{}:", t.path().display());
        let env = Env::for_test(Some(t.path()), t.path(), &[("PATH", &path)]);
        assert_eq!(find_omp(&env).unwrap(), t.path().join("omp"));
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
            let result = omp_version(&bin, Duration::from_secs(1), None);
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
        let result = omp_version(&bin, Duration::from_millis(60), None);
        assert!(result.unwrap_err().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[cfg(unix)]
    #[test]
    fn version_timeout_kills_descendants_and_inherited_pipes() {
        let t = tempfile::tempdir().unwrap();
        let start = Instant::now();
        thread::scope(|scope| {
            let mut cases = Vec::new();
            for (name, redirect, ending, message) in [
                ("waiting", "", "wait", "omp --version timed out"),
                ("stdout", "", "echo omp/18.14.0; exit 0", "output timed out"),
                (
                    "stderr",
                    ">/dev/null",
                    "echo omp/18.14.0; exit 0",
                    "output timed out",
                ),
            ] {
                let bin = script(
                    t.path(),
                    name,
                    &format!(
                        "( /bin/sleep 1; echo leaked > \"$0.marker\" ) {redirect} &\n{ending}"
                    ),
                );
                cases.push(scope.spawn(move || {
                    let start = Instant::now();
                    let result = omp_version(&bin, Duration::from_millis(300), None);
                    let error = result.unwrap_err();
                    assert!(error.contains(message), "{name}: {error}");
                    assert!(start.elapsed() < Duration::from_secs(2));
                    thread::sleep(Duration::from_millis(1100));
                    assert!(
                        !bin.with_extension("marker").exists(),
                        "surviving descendant of {name}"
                    );
                }));
            }
            for case in cases {
                case.join().unwrap();
            }
        });
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
