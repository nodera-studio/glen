use crate::{
    env::Env,
    error::{ErrorCode, GlenError},
};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
#[derive(Clone, Debug)]
pub struct GlenHome {
    pub root: PathBuf,
}
pub fn resolve(env: &Env) -> Result<GlenHome, GlenError> {
    let home = || {
        env.home()
            .ok_or_else(|| GlenError::new(ErrorCode::InvalidEnv, "HOME is not set"))
    };
    let root = if let Some(value) = env.non_empty("GLEN_HOME") {
        let p = if value == "~" {
            home()?.to_path_buf()
        } else if let Some(rest) = value.strip_prefix("~/") {
            home()?.join(rest)
        } else {
            PathBuf::from(value)
        };
        if !p.is_absolute() {
            return Err(GlenError::new(
                ErrorCode::InvalidEnv,
                "GLEN_HOME must be an absolute path",
            ));
        }
        p
    } else {
        home()?.join(".glen")
    };
    Ok(GlenHome { root })
}
/// SEC-3 read paths: [`resolve`] plus [`GlenHome::verify_private`] for each runtime subtree the
/// caller is about to read (`run/live`, `jobs`, `inbox/.read`), so a planted registry, job
/// record or reply in an untrusted home is refused with `invalid_env` instead of trusted.
pub fn resolve_trusted(env: &Env, subs: &[&str]) -> Result<GlenHome, GlenError> {
    let home = resolve(env)?;
    for sub in subs {
        home.verify_walk(Path::new(sub), false)?;
    }
    Ok(home)
}
impl GlenHome {
    pub fn ensure_dir(&self, sub: &str) -> io::Result<PathBuf> {
        let path = self.root.join(sub);
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        // SEC-3: glen never chmods an existing directory, but it refuses to trust one another
        // user could write to. The GlenError rides inside the io::Error so callers that map
        // through `sandbox::map_home_write` keep its `invalid_env` code and hint. Checked
        // before creating, so nothing is made inside an untrusted home, and again after, for
        // the directories the create just made or raced with.
        self.verify_walk(Path::new(sub), false)
            .map_err(io::Error::other)?;
        builder.create(&path)?;
        self.verify_private(Path::new(sub))
            .map_err(io::Error::other)?;
        Ok(path)
    }
    /// SEC-3: checks the root and every existing directory under it on the path to `rel`.
    /// Each must be a real directory (not a symlink) owned by the effective uid with no
    /// group/other write bits; a missing component ends the walk (nothing deeper exists).
    pub fn verify_private(&self, rel: &Path) -> Result<(), GlenError> {
        self.verify_walk(rel, true)
    }
    /// The walk behind [`Self::verify_private`]. With `strict_dirs` false a plain file on the
    /// path ends the walk instead of being refused: nothing can be created or read beneath
    /// it, so it is not a trust problem, and the caller's own create or read reports it as
    /// the io error it always was. Symlinks, other owners and writable modes are refused.
    fn verify_walk(&self, rel: &Path, strict_dirs: bool) -> Result<(), GlenError> {
        #[cfg(unix)]
        {
            use std::{os::unix::fs::MetadataExt, path::Component};
            let euid = rustix::process::geteuid().as_raw();
            let refuse = |path: &Path, why: String| {
                GlenError::new(
                    ErrorCode::InvalidEnv,
                    format!("refusing GLEN_HOME directory {}: {why}", path.display()),
                )
                .with_hint("point GLEN_HOME at a private directory you own (mkdir -m 700)")
                .with_detail("path", path.to_string_lossy().into_owned())
            };
            let mut path = self.root.clone();
            let mut rest = rel.components();
            loop {
                let meta = match fs::symlink_metadata(&path) {
                    Ok(m) => m,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                    Err(e) => {
                        return Err(GlenError::new(
                            ErrorCode::Io,
                            format!("cannot inspect {}: {e}", path.display()),
                        ));
                    }
                };
                if meta.file_type().is_symlink() {
                    return Err(refuse(&path, "it is a symlink".into()));
                }
                if !meta.is_dir() {
                    if !strict_dirs {
                        return Ok(());
                    }
                    return Err(refuse(&path, "it is not a directory".into()));
                }
                if meta.uid() != euid {
                    return Err(refuse(
                        &path,
                        format!(
                            "it is owned by uid {}, not the current uid {euid}",
                            meta.uid()
                        ),
                    ));
                }
                if meta.mode() & 0o022 != 0 {
                    return Err(refuse(
                        &path,
                        format!(
                            "it is group- or other-writable (mode {:o})",
                            meta.mode() & 0o7777
                        ),
                    ));
                }
                match rest.next() {
                    Some(Component::Normal(c)) => path.push(c),
                    Some(Component::CurDir) => continue,
                    Some(_) => {
                        return Err(GlenError::new(
                            ErrorCode::Internal,
                            format!("GLEN_HOME subpath must be relative: {}", rel.display()),
                        ));
                    }
                    None => return Ok(()),
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (rel, strict_dirs);
            Ok(())
        }
    }
}
/// Recovers the SEC-3 refusal carried inside an `ensure_dir` io error; any other error is
/// handed back unchanged.
pub fn take_refusal(err: io::Error) -> Result<GlenError, io::Error> {
    if !err.get_ref().is_some_and(|e| e.is::<GlenError>()) {
        return Err(err);
    }
    match err.into_inner().map(|e| e.downcast::<GlenError>()) {
        Some(Ok(e)) => Ok(*e),
        _ => unreachable!("checked above"),
    }
}
pub fn write_private_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic(path, bytes, 0o600)
}
pub fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file name required"))?
        .to_string_lossy();
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_file_name(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    let mut file = options.open(&tmp)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
        fs::File::open(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new(".")),
        )?
        .sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}
pub fn mode_is_private(path: &Path) -> Option<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .ok()
            .map(|s| s.permissions().mode() & 0o077 == 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolution_is_lazy() {
        for (vars, want) in [
            (vec![], "/h/.glen"),
            (vec![("GLEN_HOME", "/x/y")], "/x/y"),
            (vec![("GLEN_HOME", "~/g")], "/h/g"),
            (vec![("GLEN_HOME", "~")], "/h"),
        ] {
            let env = Env::for_test(Some(Path::new("/h")), Path::new("/w"), &vars);
            assert_eq!(resolve(&env).unwrap().root, Path::new(want));
        }
        let env = Env::for_test(
            Some(Path::new("/h")),
            Path::new("/w"),
            &[("GLEN_HOME", "rel")],
        );
        assert_eq!(resolve(&env).unwrap_err().code, ErrorCode::InvalidEnv);
    }
    #[test]
    fn private_directories_and_atomic_files() {
        let t = tempfile::tempdir().unwrap();
        let home = GlenHome {
            root: t.path().join("glen"),
        };
        let out = home.ensure_dir("out").unwrap();
        let file = out.join("test.txt");
        write_private_atomic(&file, b"exact bytes").unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"exact bytes");
        assert_eq!(fs::read_dir(&out).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&home.root).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(out).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    // SEC-3 re-derives this test: an existing root is still never chmodded (design.md:199),
    // and a 0755 root owned by the current user is still accepted, but a 0777 root is now
    // refused with invalid_env instead of trusted.
    #[cfg(unix)]
    #[test]
    fn existing_root_mode_unchanged() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        fs::set_permissions(t.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let h = GlenHome {
            root: t.path().into(),
        };
        h.ensure_dir("out").unwrap();
        assert_eq!(
            fs::metadata(t.path()).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(mode_is_private(t.path()), Some(false));
        fs::set_permissions(t.path(), fs::Permissions::from_mode(0o777)).unwrap();
        let err = take_refusal(h.ensure_dir("jobs").unwrap_err()).unwrap();
        assert_eq!(err.code, ErrorCode::InvalidEnv);
        assert_eq!(err.code.exit_code(), 2);
        assert!(err.message.contains(&t.path().display().to_string()));
        assert!(err.hint.unwrap().contains("mkdir -m 700"));
        assert_eq!(
            fs::metadata(t.path()).unwrap().permissions().mode() & 0o777,
            0o777,
            "glen never chmods an existing home"
        );
        fs::set_permissions(t.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn writable_or_symlinked_subdirectories_are_refused() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let h = GlenHome {
            root: t.path().join("glen"),
        };
        h.ensure_dir("run/live").unwrap();
        h.verify_private(Path::new("run/live")).unwrap();
        // A missing component ends the walk: nothing deeper can exist.
        h.verify_private(Path::new("jobs/abc")).unwrap();
        fs::set_permissions(h.root.join("run"), fs::Permissions::from_mode(0o777)).unwrap();
        for rel in ["run/live", "run/locks", "run"] {
            let err = h.verify_private(Path::new(rel)).unwrap_err();
            assert_eq!(err.code, ErrorCode::InvalidEnv, "{rel}");
            assert!(err.message.contains("group- or other-writable"), "{rel}");
        }
        let err = take_refusal(h.ensure_dir("run/locks").unwrap_err()).unwrap();
        assert_eq!(err.code, ErrorCode::InvalidEnv);
        fs::set_permissions(h.root.join("run"), fs::Permissions::from_mode(0o700)).unwrap();
        h.ensure_dir("run/locks").unwrap();
        // The inbox (or any subdirectory) as a symlink to a directory the user owns.
        let elsewhere = t.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        fs::set_permissions(&elsewhere, fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink(&elsewhere, h.root.join("inbox")).unwrap();
        let err = take_refusal(h.ensure_dir("inbox").unwrap_err()).unwrap();
        assert_eq!(err.code, ErrorCode::InvalidEnv);
        assert!(err.message.contains("symlink"));
        // A root that is a symlink is refused too.
        let link = t.path().join("link");
        std::os::unix::fs::symlink(&h.root, &link).unwrap();
        let err = GlenHome { root: link }
            .verify_private(Path::new("run"))
            .unwrap_err();
        assert!(err.message.contains("symlink"));
        // A plain file is not a directory.
        fs::write(h.root.join("out"), b"x").unwrap();
        let err = h.verify_private(Path::new("out")).unwrap_err();
        assert!(err.message.contains("not a directory"));
    }
    // LOW-007 (W8 review): an untrusted root is refused before anything is created in it.
    #[cfg(unix)]
    #[test]
    fn nothing_is_created_inside_an_untrusted_root() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let h = GlenHome {
            root: t.path().join("glen"),
        };
        fs::create_dir(&h.root).unwrap();
        fs::set_permissions(&h.root, fs::Permissions::from_mode(0o777)).unwrap();
        for sub in ["out", "run/locks", "inbox/.read"] {
            let err = take_refusal(h.ensure_dir(sub).unwrap_err()).unwrap();
            assert_eq!(err.code, ErrorCode::InvalidEnv, "{sub}");
        }
        assert_eq!(fs::read_dir(&h.root).unwrap().count(), 0);
        fs::set_permissions(&h.root, fs::Permissions::from_mode(0o700)).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn a_directory_owned_by_another_uid_is_refused() {
        use std::os::unix::fs::MetadataExt;
        // `/` is owned by root; meaningful only when the tests do not run as root.
        let euid = rustix::process::geteuid().as_raw();
        if fs::metadata("/").unwrap().uid() == euid {
            return;
        }
        let err = GlenHome { root: "/".into() }
            .verify_private(Path::new(""))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidEnv);
        assert!(err.message.contains("owned by uid"));
    }
    #[test]
    fn non_refusal_io_errors_pass_through() {
        let e = take_refusal(io::Error::from(io::ErrorKind::PermissionDenied)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
        let e = take_refusal(io::Error::other("plain")).unwrap_err();
        assert_eq!(e.to_string(), "plain");
    }
    #[test]
    fn concurrent_atomic_writes_do_not_share_temp_files() {
        use std::sync::{Arc, Barrier};
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("shared");
        let barrier = Arc::new(Barrier::new(8));
        let writers: Vec<_> = (0..8)
            .map(|n| {
                let file = file.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    write_private_atomic(&file, &vec![n; 65536])
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap().unwrap();
        }
        let bytes = fs::read(&file).unwrap();
        assert_eq!(bytes.len(), 65536);
        assert!(bytes.iter().all(|v| *v == bytes[0]));
        assert_eq!(fs::read_dir(t.path()).unwrap().count(), 1);
    }
}
