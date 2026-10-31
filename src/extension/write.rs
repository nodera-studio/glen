//! Pin directory descriptors and revalidate the inspected identities before publishing.
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    fs, io,
    path::{Path, PathBuf},
};
use xxhash_rust::xxh64::xxh64;

use crate::atomic_swap::{self, Applied};

#[derive(Debug, PartialEq, Eq)]
struct Identity {
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    digest: Option<u64>,
    link: Option<PathBuf>,
}
fn identity(path: &Path) -> io::Result<Option<Identity>> {
    let mut meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let digest = if meta.is_file() {
        use std::io::Read;
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let mut file = options.open(path)?;
        meta = file.metadata()?;
        if !meta.is_file() {
            return Err(io::Error::other("extension changed to a non-regular file"));
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Some(xxh64(&bytes, 0))
    } else {
        None
    };
    Ok(Some(Identity {
        #[cfg(unix)]
        dev: meta.dev(),
        #[cfg(unix)]
        ino: meta.ino(),
        digest,
        link: if meta.is_symlink() {
            Some(fs::read_link(path)?)
        } else {
            None
        },
    }))
}
#[derive(Debug)]
pub(crate) struct Guard {
    entries: Vec<(PathBuf, Option<Identity>)>,
}
impl Guard {
    pub(crate) fn inspect(path: &Path) -> io::Result<Self> {
        let entries = path
            .ancestors()
            .map(|p| Ok((p.to_path_buf(), identity(p)?)))
            .collect::<io::Result<_>>()?;
        Ok(Self { entries })
    }
    pub(crate) fn unsafe_path(&self, scope: &Path) -> bool {
        self.entries.iter().any(|(path, id)| {
            path.starts_with(scope) && id.as_ref().is_some_and(|id| id.link.is_some())
        })
    }
    fn unchanged(&self) -> bool {
        for (path, before) in &self.entries {
            if !matches!(identity(path), Ok(ref now) if now == before) {
                return false;
            }
        }
        true
    }
    /// `Applied::Lost` means a competing edit/replacement won. Never follow a new directory
    /// symlink.
    #[cfg(unix)]
    pub(crate) fn apply(&mut self, path: &Path, bytes: Option<&[u8]>) -> io::Result<Applied> {
        self.apply_with(path, bytes, false)
    }
    #[cfg(all(unix, test))]
    pub(crate) fn apply_forcing_fallback(
        &mut self,
        path: &Path,
        bytes: Option<&[u8]>,
    ) -> io::Result<Applied> {
        self.apply_with(path, bytes, true)
    }
    #[cfg(unix)]
    fn apply_with(
        &mut self,
        path: &Path,
        bytes: Option<&[u8]>,
        force_fallback: bool,
    ) -> io::Result<Applied> {
        use rustix::fs::{self as at, AtFlags, Mode, OFlags};
        use std::io::Write;
        if !self.unchanged() {
            return Ok(Applied::Lost);
        }
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("extension has no parent"))?;
        let nearest = self
            .entries
            .iter()
            .skip(1)
            .find(|(_, i)| i.is_some())
            .ok_or_else(|| io::Error::other("extension has no existing ancestor"))?
            .0
            .clone();
        let canonical = fs::canonicalize(&nearest)?;
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let mut dir = at::open("/", flags, Mode::empty())?;
        for part in canonical.components().skip(1) {
            dir = at::openat(&dir, part.as_os_str(), flags, Mode::empty())?;
        }
        if !self.unchanged() {
            return Ok(Applied::Lost);
        }
        let expected = fs::metadata(&nearest)?;
        let pinned = at::fstat(&dir)?;
        if expected.dev() != pinned.st_dev as u64 || expected.ino() != pinned.st_ino {
            return Ok(Applied::Lost);
        }
        let mut created_path = nearest.clone();
        for part in parent
            .strip_prefix(&nearest)
            .map_err(io::Error::other)?
            .components()
        {
            match at::mkdirat(&dir, part.as_os_str(), Mode::from_raw_mode(0o700)) {
                Ok(()) => {}
                Err(rustix::io::Errno::EXIST) => return Ok(Applied::Lost),
                Err(e) => return Err(e.into()),
            }
            dir = at::openat(&dir, part.as_os_str(), flags, Mode::empty())?;
            // Only directories created by this invocation may replace an inspected absence.
            let pinned = at::fstat(&dir)?;
            created_path.push(part.as_os_str());
            let created = self
                .entries
                .iter_mut()
                .find(|(p, i)| i.is_none() && p == &created_path);
            if let Some((p, before)) = created {
                *before = identity(p)?;
                if before
                    .as_ref()
                    .is_none_or(|i| i.dev != pinned.st_dev as u64 || i.ino != pinned.st_ino)
                {
                    return Ok(Applied::Lost);
                }
            }
        }
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::other("extension has no file name"))?;
        let Some(bytes) = bytes else {
            if !self.unchanged() {
                return Ok(Applied::Lost);
            }
            at::unlinkat(&dir, name, AtFlags::empty())?;
            return Ok(Applied::Published);
        };
        let existing = self.entries[0].1.as_ref().map(|i| (i.dev, i.ino, i.digest));
        let temp = format!(".index.ts.{}.tmp", uuid::Uuid::now_v7());
        let fd = at::openat(
            &dir,
            &temp,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?;
        let mut file = fs::File::from(fd);
        // `publish_over` only needs this to recognize our own temp copy if a competing writer
        // forces a swap-back; take it from the fd we already hold, never a path lookup.
        let ours = file
            .metadata()
            .map(|m| (m.dev(), m.ino()))
            .unwrap_or((0, 0));
        let mut swapped = false;
        let result = (|| {
            file.write_all(bytes)?;
            file.sync_all()?;
            if !self.unchanged() {
                return Ok(Applied::Lost);
            }
            let applied = match existing {
                None => {
                    match at::renameat_with(&dir, &temp, &dir, name, at::RenameFlags::NOREPLACE) {
                        Ok(()) => Applied::Published,
                        Err(rustix::io::Errno::EXIST) => Applied::Lost,
                        Err(e) => return Err(e.into()),
                    }
                }
                Some(expected) => {
                    swapped = true;
                    atomic_swap::publish_over(&dir, &temp, name, expected, ours, force_fallback)?
                }
            };
            if matches!(applied, Applied::Published | Applied::PublishedNotAtomic) {
                at::fsync(&dir)?;
            }
            Ok(applied)
        })();
        // `publish_over`'s own swap path already unlinks `temp` on both Published and Lost; the
        // create (NOREPLACE) path and any error before the swap leave `temp` for us to clean up.
        if !swapped
            && !matches!(result, Ok(Applied::Published))
            && at::statat(&dir, temp.as_str(), AtFlags::SYMLINK_NOFOLLOW).is_ok()
        {
            let _ = at::unlinkat(&dir, &temp, AtFlags::empty());
        }
        result
    }
    #[cfg(unix)]
    pub(crate) fn remove_empty_parent(&mut self, path: &Path) -> io::Result<bool> {
        use rustix::fs::{self as at, AtFlags, Mode, OFlags};
        if let Some((_, before)) = self.entries.iter_mut().find(|(p, _)| p == path) {
            *before = None;
        }
        if !self.unchanged() {
            return Ok(false);
        }
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("extension has no parent"))?;
        let grandparent = parent
            .parent()
            .ok_or_else(|| io::Error::other("extension has no grandparent"))?;
        let canonical = fs::canonicalize(grandparent)?;
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let mut dir = at::open("/", flags, Mode::empty())?;
        for part in canonical.components().skip(1) {
            dir = at::openat(&dir, part.as_os_str(), flags, Mode::empty())?;
        }
        if !self.unchanged() {
            return Ok(false);
        }
        let expected = fs::metadata(grandparent)?;
        let pinned = at::fstat(&dir)?;
        if expected.dev() != pinned.st_dev as u64 || expected.ino() != pinned.st_ino {
            return Ok(false);
        }
        at::unlinkat(
            &dir,
            parent
                .file_name()
                .ok_or_else(|| io::Error::other("extension directory has no name"))?,
            AtFlags::REMOVEDIR,
        )?;
        Ok(true)
    }
}
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    #[test]
    fn competing_create_never_clobbers_the_appeared_file() {
        let t = tempfile::tempdir().unwrap();
        let parent = t.path().join("extensions/glen");
        fs::create_dir_all(&parent).unwrap();
        let path = parent.join("index.ts");
        let mut guard = Guard::inspect(&path).unwrap();
        fs::write(&path, b"external").unwrap();
        assert_eq!(
            guard.apply(&path, Some(b"replacement")).unwrap(),
            Applied::Lost
        );
        assert_eq!(fs::read(&path).unwrap(), b"external");
    }
    #[test]
    fn competing_edits_replacements_and_symlinks_never_publish_or_unlink() {
        for remove in [false, true] {
            for change in ["edit", "file", "parent", "ancestor", "appeared"] {
                let t = tempfile::tempdir().unwrap();
                let parent = t.path().join("extensions/glen");
                fs::create_dir_all(&parent).unwrap();
                let path = parent.join("index.ts");
                if change != "appeared" {
                    fs::write(&path, b"before").unwrap();
                }
                let mut guard = Guard::inspect(&path).unwrap();
                let outside = t.path().join("outside");
                fs::create_dir(&outside).unwrap();
                fs::write(outside.join("index.ts"), b"external").unwrap();
                match change {
                    "edit" | "appeared" => fs::write(&path, b"edit").unwrap(),
                    "file" => {
                        fs::remove_file(&path).unwrap();
                        symlink(outside.join("index.ts"), &path).unwrap();
                    }
                    "parent" => {
                        fs::rename(&parent, t.path().join("old")).unwrap();
                        symlink(&outside, &parent).unwrap();
                    }
                    "ancestor" => {
                        fs::rename(t.path().join("extensions"), t.path().join("old")).unwrap();
                        symlink(&outside, t.path().join("extensions")).unwrap();
                    }
                    _ => unreachable!(),
                }
                assert_eq!(
                    guard
                        .apply(&path, if remove { None } else { Some(b"replacement") })
                        .unwrap(),
                    Applied::Lost,
                    "{change}"
                );
                assert_eq!(fs::read(outside.join("index.ts")).unwrap(), b"external");
            }
        }
    }
    #[test]
    fn replacing_an_existing_file_swaps_atomically_and_leaves_no_temp() {
        let t = tempfile::tempdir().unwrap();
        let parent = t.path().join("extensions/glen");
        fs::create_dir_all(&parent).unwrap();
        let path = parent.join("index.ts");
        fs::write(&path, b"before").unwrap();
        let mut guard = Guard::inspect(&path).unwrap();
        assert_eq!(
            guard.apply(&path, Some(b"after")).unwrap(),
            Applied::Published
        );
        assert_eq!(fs::read(&path).unwrap(), b"after");
        assert_eq!(
            fs::read_dir(&parent).unwrap().count(),
            1,
            "no temp left behind"
        );
    }
    #[test]
    fn forced_fallback_still_publishes_without_exchange() {
        let t = tempfile::tempdir().unwrap();
        let parent = t.path().join("extensions/glen");
        fs::create_dir_all(&parent).unwrap();
        let path = parent.join("index.ts");
        fs::write(&path, b"before").unwrap();
        let mut guard = Guard::inspect(&path).unwrap();
        assert_eq!(
            guard.apply_forcing_fallback(&path, Some(b"after")).unwrap(),
            Applied::PublishedNotAtomic
        );
        assert_eq!(fs::read(&path).unwrap(), b"after");
        assert_eq!(
            fs::read_dir(&parent).unwrap().count(),
            1,
            "no temp left behind"
        );
    }
    #[test]
    fn competing_replacement_during_the_swap_window_is_lost_not_clobbered() {
        let t = tempfile::tempdir().unwrap();
        let parent = t.path().join("extensions/glen");
        fs::create_dir_all(&parent).unwrap();
        let path = parent.join("index.ts");
        fs::write(&path, b"before").unwrap();
        let mut guard = Guard::inspect(&path).unwrap();
        // A competing writer replaces the file in place (same inode story as a plain
        // edit): unchanged() must catch this before any swap is attempted.
        fs::write(&path, b"competitor").unwrap();
        assert_eq!(guard.apply(&path, Some(b"after")).unwrap(), Applied::Lost);
        assert_eq!(fs::read(&path).unwrap(), b"competitor");
    }
}
