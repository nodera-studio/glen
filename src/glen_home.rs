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
        builder.create(&path)?;
        Ok(path)
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
