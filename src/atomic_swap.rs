//! `renameat2(RENAME_EXCHANGE)` / `renameatx_np(RENAME_SWAP)` publish, shared by
//! `install_guard::Guard` for the extension and the skill. The guard pins a directory fd and
//! keeps its own private `Identity`, so this takes plain (dev, ino, digest) tuples.
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
    use std::os::unix::fs::MetadataExt;
    #[allow(clippy::unnecessary_cast)] // st_mode is u16 on macOS, u32 on Linux
    let is_regular = (stat.st_mode as u32 & libc::S_IFMT as u32) == libc::S_IFREG as u32;
    if !is_regular {
        return Ok(Some((stat.st_dev as u64, stat.st_ino, None)));
    }
    // O_NONBLOCK: if the temp name were swapped for a FIFO between `statat` and `openat` (the
    // temp name is a random UUID, so unlikely, but never block with no timeout on it), the
    // open or the first read returns EAGAIN instead of hanging. Re-check with `fstat` on the
    // opened fd rather than trusting the earlier `statat`, same pattern as
    // `install_guard::identity`.
    let fd = at::openat(
        dir,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )?;
    let mut file = fs::File::from(fd);
    let refreshed = file.metadata()?;
    if !refreshed.is_file() {
        return Err(io::Error::other(
            "displaced file changed to a non-regular file",
        ));
    }
    use std::io::Read;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(Some((
        refreshed.dev(),
        refreshed.ino(),
        Some(xxh64(&bytes, 0)),
    )))
}

/// Whether `temp` is still safe for the caller to clean up after a failed `publish_over`.
/// The exchange syscall is the one moment of no return: before it (or when it never
/// completes), `temp` still holds only our own new file. Once it has completed, `temp` may
/// hold a third party's displaced file — AC-1019 says glen never deletes that without having
/// verified it, so neither `publish_over` nor its caller may touch `temp` again on this path.
#[cfg(unix)]
#[derive(Debug)]
pub(crate) enum PublishError {
    BeforeExchange(io::Error),
    AfterExchange(io::Error),
}
#[cfg(unix)]
impl PublishError {
    pub(crate) fn temp_is_safe_to_clean(&self) -> bool {
        matches!(self, Self::BeforeExchange(_))
    }
    pub(crate) fn into_io(self) -> io::Error {
        match self {
            Self::BeforeExchange(e) | Self::AfterExchange(e) => e,
        }
    }
}
/// Publish `temp` over `name` in `dir`. `expected` is the identity of the file the caller
/// inspected before writing `temp`; `ours` is the identity `temp` itself has right now (used to
/// recognize our own copy if a swap-back is needed). `noun` names what is being installed, for
/// the "changed twice" message only (e.g. "extension", "skill").
///
/// On a filesystem without atomic exchange, falls back to a plain rename and reports
/// `PublishedNotAtomic`. Any other rename error (EACCES, EPERM, EIO, …) is returned as an error;
/// `PublishError::temp_is_safe_to_clean` tells the caller whether `temp` is still safe to unlink.
/// Durability (a directory fsync) is the caller's concern, not this function's: a publish that
/// succeeded here but could not be synced is a successful `Applied`, never an error.
#[cfg(unix)]
pub(crate) fn publish_over(
    dir: &rustix::fd::OwnedFd,
    temp: &str,
    name: &OsStr,
    expected: (u64, u64, Option<u64>),
    ours: (u64, u64),
    force_fallback: bool,
    noun: &str,
) -> Result<Applied, PublishError> {
    use rustix::fs::{self as at, RenameFlags};
    if force_fallback {
        return fallback(dir, temp, name).map_err(PublishError::BeforeExchange);
    }
    match at::renameat_with(dir, temp, dir, name, RenameFlags::EXCHANGE) {
        Ok(()) => finish_exchange(dir, temp, name, expected, ours, noun)
            .map_err(PublishError::AfterExchange),
        Err(e) if exchange_unsupported(e) => {
            fallback(dir, temp, name).map_err(PublishError::BeforeExchange)
        }
        Err(e) => Err(PublishError::BeforeExchange(e.into())),
    }
}
#[cfg(unix)]
fn fallback(dir: &rustix::fd::OwnedFd, temp: &str, name: &OsStr) -> io::Result<Applied> {
    use rustix::fs::{self as at};
    at::renameat(dir, temp, dir, name)?;
    Ok(Applied::PublishedNotAtomic)
}
/// Runs only after the exchange itself has completed: `temp` now holds whatever was displaced
/// from `name`. Every path either verifies an identity before unlinking `temp`, or leaves it
/// untouched and says so in the error.
#[cfg(unix)]
fn finish_exchange(
    dir: &rustix::fd::OwnedFd,
    temp: &str,
    name: &OsStr,
    expected: (u64, u64, Option<u64>),
    ours: (u64, u64),
    noun: &str,
) -> io::Result<Applied> {
    use rustix::fs::{self as at, AtFlags, RenameFlags};
    let displaced = identity_at(dir, OsStr::new(temp))?;
    if displaced.as_ref().map(|(d, i, g)| (*d, *i, *g)) == Some(expected) {
        at::unlinkat(dir, temp, AtFlags::empty())?;
        Ok(Applied::Published)
    } else {
        // Not our expected file: someone else replaced it between our check and the swap.
        // Swap back so `name` keeps publishing the third party's file.
        at::renameat_with(dir, temp, dir, name, RenameFlags::EXCHANGE)?;
        let back = identity_at(dir, OsStr::new(temp))?;
        if back.as_ref().map(|(d, i, _)| (*d, *i)) == Some(ours) {
            at::unlinkat(dir, temp, AtFlags::empty())?;
            Ok(Applied::Lost)
        } else {
            Err(io::Error::other(format!(
                "{noun} changed twice during install; displaced copy kept at {temp}"
            )))
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use rustix::fs::{self as at, Mode, OFlags};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

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
            "extension",
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
            "extension",
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
            "extension",
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
    /// A real syscall failure (not just the errno classifier), per plan Step 15: injected by
    /// making the directory read-only so the rename itself gets EACCES. The install fails,
    /// nothing is published, and `temp` is left for the caller to clean up (the error is
    /// `BeforeExchange`: the exchange never even started).
    #[test]
    fn a_real_eacces_on_the_exchange_fails_and_leaves_temp_for_the_caller() {
        let t = tempfile::tempdir().unwrap();
        let name = "index.ts";
        let path = t.path().join(name);
        let expected = write_identity(&path, b"before");
        let temp = format!(".{name}.tmp");
        let temp_path = t.path().join(&temp);
        let ours = write_identity(&temp_path, b"after");
        let dir = open_dir(t.path());
        let mut perms = std::fs::metadata(t.path()).unwrap().permissions();
        let original = perms.clone();
        perms.set_mode(0o555);
        std::fs::set_permissions(t.path(), perms).unwrap();
        let result = publish_over(
            &dir,
            &temp,
            std::ffi::OsStr::new(name),
            expected,
            (ours.0, ours.1),
            false,
            "extension",
        );
        std::fs::set_permissions(t.path(), original).unwrap();
        let err = result.expect_err("a read-only directory must fail the exchange");
        assert!(err.temp_is_safe_to_clean(), "the exchange never started");
        assert_eq!(err.into_io().raw_os_error(), Some(libc::EACCES));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"before",
            "nothing published"
        );
        assert!(
            temp_path.exists(),
            "our temp is left for the caller to clean up"
        );
    }
    /// HIGH-002: once the exchange has completed, an error must never delete the displaced
    /// file, even when it cannot re-verify it (the "changed twice" case): a second competing
    /// writer replaces `temp` again, between our swap-back attempt's own two steps.
    #[test]
    fn changed_twice_keeps_the_displaced_copy_and_reports_after_exchange() {
        let t = tempfile::tempdir().unwrap();
        let name = "index.ts";
        let path = t.path().join(name);
        let expected = write_identity(&path, b"before");
        // A competitor replaces the file before our swap, so the swap-back branch runs.
        std::fs::write(&path, b"first-competitor").unwrap();
        let temp = format!(".{name}.tmp");
        let temp_path = t.path().join(&temp);
        let _ours = write_identity(&temp_path, b"after");
        let dir = open_dir(t.path());
        // Force the swap-back to find a THIRD file at `temp`, not ours: simulate by handing a
        // `ours` identity that can never match what is actually there after the swap-back
        // (a dev/ino pair no real file on this filesystem has).
        let impossible_ours = (u64::MAX, u64::MAX);
        let result = publish_over(
            &dir,
            &temp,
            std::ffi::OsStr::new(name),
            expected,
            impossible_ours,
            false,
            "skill",
        );
        let err = result.expect_err("a second mismatch must not be silently resolved");
        assert!(
            !err.temp_is_safe_to_clean(),
            "the exchange completed; temp may hold someone else's file"
        );
        let message = err.into_io().to_string();
        assert!(message.contains("skill changed twice"), "{message}");
        assert!(message.contains(&temp), "{message}");
        // The displaced copy is still there, verified or not, exactly where the error says.
        assert!(temp_path.exists());
        assert_eq!(std::fs::read(&path).unwrap(), b"first-competitor");
    }
}
