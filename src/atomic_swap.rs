//! `renameat2(RENAME_EXCHANGE)` / `renameatx_np(RENAME_SWAP)` publish, shared by
//! `extension::write` and `skill::write`. Both callers pin a directory fd and keep
//! their own private `Identity`, so this takes plain (dev, ino, digest) tuples.
#[cfg(unix)]
use std::{ffi::OsStr, fs, io};
use xxhash_rust::xxh64::xxh64;

/// The outcome of publishing over an existing file.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Applied {
    /// Swapped atomically and the displaced copy matched what the caller inspected.
    Published,
    /// The filesystem cannot swap atomically; published with a plain rename instead.
    PublishedNotAtomic,
    /// A competing writer replaced the file first; nothing was published.
    Lost,
}

/// True for the errnos that mean "this filesystem has no atomic exchange", not a real failure.
#[cfg(unix)]
#[allow(unreachable_patterns)] // NOTSUP == OPNOTSUPP on Linux, distinct on Apple
pub(crate) fn exchange_unsupported(e: rustix::io::Errno) -> bool {
    matches!(
        e,
        rustix::io::Errno::INVAL
            | rustix::io::Errno::NOTSUP
            | rustix::io::Errno::OPNOTSUPP
            | rustix::io::Errno::NOSYS
    )
}

/// (dev, ino, digest for a regular file, else `None`) of `name` under `dir`, or `None` if it is
/// now missing. Never follows a symlink.
#[cfg(unix)]
fn identity_at(
    dir: &rustix::fd::OwnedFd,
    name: &OsStr,
) -> io::Result<Option<(u64, u64, Option<u64>)>> {
    use rustix::fs::{self as at, AtFlags, Mode, OFlags};
    let stat = match at::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(s) => s,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    #[allow(clippy::unnecessary_cast)] // st_mode is u16 on macOS, u32 on Linux
    let is_regular = (stat.st_mode as u32 & libc::S_IFMT as u32) == libc::S_IFREG as u32;
    let digest = if is_regular {
        let fd = at::openat(
            dir,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let mut file = fs::File::from(fd);
        use std::io::Read;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Some(xxh64(&bytes, 0))
    } else {
        None
    };
    Ok(Some((stat.st_dev as u64, stat.st_ino, digest)))
}

/// Publish `temp` over `name` in `dir`. `expected` is the identity of the file the caller
/// inspected before writing `temp`; `ours` is the identity `temp` itself has right now (used to
/// recognize our own copy if a swap-back is needed).
///
/// On a filesystem without atomic exchange, falls back to a plain rename and reports
/// `PublishedNotAtomic`. Any other rename error (EACCES, EPERM, EIO, …) is returned as an error
/// and nothing is published or unlinked.
#[cfg(unix)]
pub(crate) fn publish_over(
    dir: &rustix::fd::OwnedFd,
    temp: &str,
    name: &OsStr,
    expected: (u64, u64, Option<u64>),
    ours: (u64, u64),
    force_fallback: bool,
) -> io::Result<Applied> {
    use rustix::fs::{self as at, AtFlags, RenameFlags};
    let exchanged = if force_fallback {
        Err(rustix::io::Errno::NOSYS)
    } else {
        at::renameat_with(dir, temp, dir, name, RenameFlags::EXCHANGE)
    };
    match exchanged {
        Ok(()) => {
            // The file that was at `name` is now at `temp`.
            let displaced = identity_at(dir, OsStr::new(temp))?;
            if displaced.as_ref().map(|(d, i, g)| (*d, *i, *g)) == Some(expected) {
                at::unlinkat(dir, temp, AtFlags::empty())?;
                at::fsync(dir)?;
                Ok(Applied::Published)
            } else {
                // Not our expected file: someone else replaced it between our check and the
                // swap. Swap back so `name` keeps publishing the third party's file.
                at::renameat_with(dir, temp, dir, name, RenameFlags::EXCHANGE)?;
                let back = identity_at(dir, OsStr::new(temp))?;
                if back.as_ref().map(|(d, i, _)| (*d, *i)) == Some(ours) {
                    at::unlinkat(dir, temp, AtFlags::empty())?;
                    Ok(Applied::Lost)
                } else {
                    Err(io::Error::other(format!(
                        "extension changed twice during install; displaced copy kept at {temp}"
                    )))
                }
            }
        }
        Err(e) if exchange_unsupported(e) => {
            at::renameat(dir, temp, dir, name)?;
            Ok(Applied::PublishedNotAtomic)
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use rustix::fs::{self as at, Mode, OFlags};
    use std::os::unix::fs::MetadataExt;

    fn open_dir(path: &std::path::Path) -> rustix::fd::OwnedFd {
        at::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .unwrap()
    }

    #[test]
    fn exchange_unsupported_classifies_the_fallback_errnos() {
        assert!(exchange_unsupported(rustix::io::Errno::INVAL));
        assert!(exchange_unsupported(rustix::io::Errno::NOTSUP));
        assert!(exchange_unsupported(rustix::io::Errno::OPNOTSUPP));
        assert!(exchange_unsupported(rustix::io::Errno::NOSYS));
        assert!(!exchange_unsupported(rustix::io::Errno::ACCESS));
        assert!(!exchange_unsupported(rustix::io::Errno::IO));
    }

    fn write_identity(path: &std::path::Path, bytes: &[u8]) -> (u64, u64, Option<u64>) {
        std::fs::write(path, bytes).unwrap();
        let meta = std::fs::metadata(path).unwrap();
        (meta.dev(), meta.ino(), Some(xxh64(bytes, 0)))
    }

    #[test]
    fn swap_publishes_and_leaves_no_temp() {
        let t = tempfile::tempdir().unwrap();
        let name = "index.ts";
        let path = t.path().join(name);
        let expected = write_identity(&path, b"before");
        let temp = format!(".{name}.tmp");
        let temp_path = t.path().join(&temp);
        let ours = write_identity(&temp_path, b"after");
        let dir = open_dir(t.path());
        let applied = publish_over(
            &dir,
            &temp,
            std::ffi::OsStr::new(name),
            expected,
            (ours.0, ours.1),
            false,
        )
        .unwrap();
        assert_eq!(applied, Applied::Published);
        assert_eq!(std::fs::read(&path).unwrap(), b"after");
        assert!(!temp_path.exists());
    }

    #[test]
    fn a_competitor_in_the_window_is_reported_as_lost() {
        let t = tempfile::tempdir().unwrap();
        let name = "index.ts";
        let path = t.path().join(name);
        let expected = write_identity(&path, b"before");
        // A competing writer replaced the file after we inspected `expected`.
        std::fs::write(&path, b"external").unwrap();
        let temp = format!(".{name}.tmp");
        let temp_path = t.path().join(&temp);
        let ours = write_identity(&temp_path, b"after");
        let dir = open_dir(t.path());
        let applied = publish_over(
            &dir,
            &temp,
            std::ffi::OsStr::new(name),
            expected,
            (ours.0, ours.1),
            false,
        )
        .unwrap();
        assert_eq!(applied, Applied::Lost);
        assert_eq!(std::fs::read(&path).unwrap(), b"external");
        assert!(!temp_path.exists());
    }

    #[test]
    fn forced_fallback_publishes_without_exchange() {
        let t = tempfile::tempdir().unwrap();
        let name = "index.ts";
        let path = t.path().join(name);
        let expected = write_identity(&path, b"before");
        let temp = format!(".{name}.tmp");
        let temp_path = t.path().join(&temp);
        let ours = write_identity(&temp_path, b"after");
        let dir = open_dir(t.path());
        let applied = publish_over(
            &dir,
            &temp,
            std::ffi::OsStr::new(name),
            expected,
            (ours.0, ours.1),
            true,
        )
        .unwrap();
        assert_eq!(applied, Applied::PublishedNotAtomic);
        assert_eq!(std::fs::read(&path).unwrap(), b"after");
        assert!(!temp_path.exists());
    }

    #[test]
    fn eacces_and_eio_from_the_swap_fail_and_publish_nothing() {
        for errno in [rustix::io::Errno::ACCESS, rustix::io::Errno::IO] {
            assert!(
                !exchange_unsupported(errno),
                "{errno:?} must not be treated as a fallback"
            );
        }
    }
}
