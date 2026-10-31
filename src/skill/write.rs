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
            return Err(io::Error::other("skill changed to a non-regular file"));
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
    pub(crate) sync_warning: Option<String>,
}
impl Guard {
    pub(crate) fn inspect(path: &Path) -> io::Result<Self> {
        let entries = path
            .ancestors()
            .map(|p| Ok((p.to_path_buf(), identity(p)?)))
            .collect::<io::Result<_>>()?;
        Ok(Self {
            entries,
            sync_warning: None,
        })
    }
    pub(crate) fn unchanged(&self) -> bool {
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
        self.apply_with(path, bytes, false, |_, _| Ok(()))
    }
    #[cfg(all(unix, test))]
    pub(crate) fn apply_forcing_fallback(
        &mut self,
        path: &Path,
        bytes: Option<&[u8]>,
    ) -> io::Result<Applied> {
        self.apply_with(path, bytes, true, |_, _| Ok(()))
    }
    #[cfg(unix)]
    fn apply_with(
        &mut self,
        path: &Path,
        bytes: Option<&[u8]>,
        force_fallback: bool,
        mut step: impl FnMut(&str, &Path) -> io::Result<()>,
    ) -> io::Result<Applied> {
        use rustix::fs::{self as at, AtFlags, Mode, OFlags};
        use std::io::Write;
        let mut published = false;
        let mut fallback_used = false;
        let mut temp_unsafe_to_clean = false;
        let mut created_dirs = Vec::new();
        let result = (|| {
            if !self.unchanged() {
                return Ok(Applied::Lost);
            }
            let parent = path
                .parent()
                .ok_or_else(|| io::Error::other("skill has no parent"))?;
            let nearest = self
                .entries
                .iter()
                .skip(1)
                .find(|(_, i)| i.is_some())
                .ok_or_else(|| io::Error::other("skill has no existing ancestor"))?
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
                match step("mkdir", &created_path.join(part.as_os_str())).and_then(|()| {
                    at::mkdirat(&dir, part.as_os_str(), Mode::from_raw_mode(0o755))
                        .map_err(io::Error::from)
                }) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        return Ok(Applied::Lost);
                    }
                    Err(e) => return Err(e),
                }
                let created = at::statat(&dir, part.as_os_str(), AtFlags::SYMLINK_NOFOLLOW)?;
                created_dirs.push((rustix::io::dup(&dir)?, part.as_os_str().to_owned(), created));
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
                .ok_or_else(|| io::Error::other("skill has no file name"))?;
            let Some(bytes) = bytes else {
                if !self.unchanged() {
                    return Ok(Applied::Lost);
                }
                at::unlinkat(&dir, name, AtFlags::empty())?;
                published = true;
                return Ok(Applied::Published);
            };
            let existing = self.entries[0].1.as_ref().map(|i| (i.dev, i.ino, i.digest));
            let temp = format!(".SKILL.md.{}.tmp", uuid::Uuid::now_v7());
            let fd = at::openat(
                &dir,
                &temp,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o644),
            )?;
            let mut file = fs::File::from(fd);
            let result = (|| {
                // LOW-002: a fallback like (0, 0) here would make `publish_over` unable to
                // recognize our own temp copy on a swap-back; propagate instead.
                let ours = file.metadata().map(|m| (m.dev(), m.ino()))?;
                step("write", path)?;
                file.write_all(bytes)?;
                file.sync_all()?;
                step("revalidate", path)?;
                if !self.unchanged() {
                    return Ok(Applied::Lost);
                }
                let applied = match existing {
                    None => {
                        match at::renameat_with(&dir, &temp, &dir, name, at::RenameFlags::NOREPLACE)
                        {
                            Ok(()) => Applied::Published,
                            Err(rustix::io::Errno::EXIST) => return Ok(Applied::Lost),
                            Err(e) => return Err(e.into()),
                        }
                    }
                    Some(expected) => atomic_swap::publish_over(
                        &dir,
                        &temp,
                        name,
                        expected,
                        ours,
                        force_fallback,
                        "skill",
                    )
                    .map_err(|e| {
                        temp_unsafe_to_clean = !e.temp_is_safe_to_clean();
                        e.into_io()
                    })?,
                };
                if matches!(applied, Applied::Lost) {
                    return Ok(Applied::Lost);
                }
                published = true;
                fallback_used = matches!(applied, Applied::PublishedNotAtomic);
                if let Err(e) =
                    step("fsync", path).and_then(|()| at::fsync(&dir).map_err(io::Error::from))
                {
                    // Rename has published the file; report success rather than claiming no write occurred.
                    self.sync_warning = Some(format!(
                        "installed {} but cannot sync its directory: {e}",
                        path.display()
                    ));
                }
                Ok(applied)
            })();
            // `publish_over`'s own swap path already unlinks `temp` on Published and on a
            // verified Lost; once the exchange has completed but could not be re-verified
            // (HIGH-002), `temp` is left untouched and unsafe for us to touch too.
            if !published
                && !temp_unsafe_to_clean
                && at::statat(&dir, temp.as_str(), AtFlags::SYMLINK_NOFOLLOW).is_ok()
                && let Err(e) = step("cleanup", path).and_then(|()| {
                    at::unlinkat(&dir, &temp, AtFlags::empty()).map_err(io::Error::from)
                })
            {
                eprintln!("warning: cannot clean temporary skill file {temp}: {e}");
            }
            result
        })();
        let result = match result {
            // Only when the exchange never completed: a changed ancestor/mkdir/revalidate
            // check failing with some unrelated error, while the directory has genuinely
            // moved on, is the same "competing write won" outcome as returning Lost directly.
            // Once the exchange has completed (HIGH-002), the error already says exactly what
            // happened and must reach the caller, never be silently downgraded to Lost.
            Err(_) if !published && !temp_unsafe_to_clean && !self.unchanged() => Ok(Applied::Lost),
            other => other,
        };
        let result = result.map(|applied| {
            if fallback_used && matches!(applied, Applied::Published) {
                Applied::PublishedNotAtomic
            } else {
                applied
            }
        });
        if !published {
            for (parent, name, created) in created_dirs.into_iter().rev() {
                // Remove only this run's still-identical empty directories, through pinned parents.
                if let Ok(now) = at::statat(&parent, &name, AtFlags::SYMLINK_NOFOLLOW)
                    && now.st_dev == created.st_dev
                    && now.st_ino == created.st_ino
                    && let Err(e) = at::unlinkat(&parent, &name, AtFlags::REMOVEDIR)
                    && e != rustix::io::Errno::NOTEMPTY
                {
                    eprintln!(
                        "warning: cannot clean skill directory {}: {e}",
                        name.to_string_lossy()
                    );
                }
            }
        }
        result
    }
    #[cfg(unix)]
    pub(crate) fn remove_empty_parent(&mut self, path: &Path) -> io::Result<bool> {
        self.remove_empty_parent_with(path, || Ok(()))
    }
    #[cfg(unix)]
    fn remove_empty_parent_with(
        &mut self,
        path: &Path,
        step: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<bool> {
        let result = (|| {
            use rustix::fs::{self as at, AtFlags, Mode, OFlags};
            if let Some((_, before)) = self.entries.iter_mut().find(|(p, _)| p == path) {
                *before = None;
            }
            if !self.unchanged() {
                return Ok(false);
            }
            let parent = path
                .parent()
                .ok_or_else(|| io::Error::other("skill has no parent"))?;
            let grandparent = parent
                .parent()
                .ok_or_else(|| io::Error::other("skill has no grandparent"))?;
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
            step()?;
            at::unlinkat(
                &dir,
                parent
                    .file_name()
                    .ok_or_else(|| io::Error::other("skill directory has no name"))?,
                AtFlags::REMOVEDIR,
            )?;
            Ok(true)
        })();
        match result {
            Err(_) if !self.unchanged() => Ok(false),
            other => other,
        }
    }
    #[cfg(not(unix))]
    pub(crate) fn remove_empty_parent(&mut self, path: &Path) -> io::Result<bool> {
        fs::remove_dir(
            path.parent()
                .ok_or_else(|| io::Error::other("skill has no parent"))?,
        )?;
        Ok(true)
    }
    #[cfg(not(unix))]
    pub(crate) fn apply(&mut self, path: &Path, bytes: Option<&[u8]>) -> io::Result<Applied> {
        if !self.unchanged() {
            return Ok(Applied::Lost);
        }
        if let Some(bytes) = bytes {
            fs::create_dir_all(
                path.parent()
                    .ok_or_else(|| io::Error::other("skill has no parent"))?,
            )?;
            if !self.unchanged() {
                return Ok(Applied::Lost);
            }
            crate::glen_home::write_atomic(path, bytes, 0o644)?;
        } else {
            fs::remove_file(path)?;
        }
        Ok(Applied::Published)
    }
}
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    #[test]
    fn cycle2_created_ancestors_cleaned_on_mkdir_failure_and_late_change() {
        for failure in ["mkdir", "write", "revalidate"] {
            let t = tempfile::tempdir().unwrap();
            let root = t.path().join("scope");
            fs::create_dir(&root).unwrap();
            let path = root.join("skills/glen/SKILL.md");
            let mut guard = Guard::inspect(&path).unwrap();
            let old = t.path().join("old");
            let result = guard.apply_with(&path, Some(b"new"), false, |stage, at| {
                if stage == failure {
                    if stage == "mkdir" && at.ends_with("glen") {
                        return Err(io::Error::from_raw_os_error(libc::EEXIST));
                    } else if stage == "write" {
                        return Err(io::Error::from_raw_os_error(libc::EIO));
                    } else if stage == "revalidate" {
                        fs::rename(&root, &old)?;
                        fs::create_dir(&root)?;
                    }
                }
                Ok(())
            });
            if failure == "write" {
                assert_eq!(result.unwrap_err().raw_os_error(), Some(libc::EIO));
            } else {
                assert_eq!(result.unwrap(), Applied::Lost);
            }
            assert!(!root.join("skills").exists());
            assert!(!old.join("skills").exists());
        }
    }
    #[test]
    fn cycle2_published_file_survives_directory_fsync_failure_as_success() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("SKILL.md");
        let mut guard = Guard::inspect(&path).unwrap();
        assert_eq!(
            guard
                .apply_with(&path, Some(b"installed"), false, |stage, _| {
                    if stage == "fsync" {
                        Err(io::Error::from_raw_os_error(libc::EIO))
                    } else {
                        Ok(())
                    }
                })
                .unwrap(),
            Applied::Published
        );
        assert_eq!(fs::read(&path).unwrap(), b"installed");
        assert!(guard.sync_warning.as_ref().unwrap().contains("cannot sync"));
    }
    #[test]
    fn cycle2_temp_cleanup_preserves_original_error() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("SKILL.md");
        let mut guard = Guard::inspect(&path).unwrap();
        let error = guard
            .apply_with(&path, Some(b"new"), false, |stage, _| match stage {
                "write" => Err(io::Error::from_raw_os_error(libc::ENOSPC)),
                "cleanup" => Err(io::Error::from_raw_os_error(libc::EACCES)),
                _ => Ok(()),
            })
            .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ENOSPC));
        assert!(!path.exists());
    }
    #[test]
    fn cycle2_parent_swapped_at_unlink_is_changed_skip() {
        let t = tempfile::tempdir().unwrap();
        let parent = t.path().join("glen");
        fs::create_dir(&parent).unwrap();
        let path = parent.join("SKILL.md");
        fs::write(&path, b"old").unwrap();
        let mut guard = Guard::inspect(&path).unwrap();
        assert_eq!(guard.apply(&path, None).unwrap(), Applied::Published);
        let outside = t.path().join("outside");
        fs::create_dir(&outside).unwrap();
        assert!(
            !guard
                .remove_empty_parent_with(&path, || {
                    fs::rename(&parent, t.path().join("old"))?;
                    symlink(&outside, &parent)
                })
                .unwrap()
        );
        assert!(fs::symlink_metadata(&parent).unwrap().is_symlink());
        assert!(outside.is_dir());
    }
    #[test]
    fn competing_edits_replacements_and_symlinks_never_publish_or_unlink() {
        for remove in [false, true] {
            for change in [
                "edit",
                "inode",
                "file",
                "parent",
                "ancestor",
                "ancestor_file",
                "appeared",
            ] {
                let t = tempfile::tempdir().unwrap();
                let parent = t.path().join("skills/glen");
                fs::create_dir_all(&parent).unwrap();
                let path = parent.join("SKILL.md");
                if change != "appeared" {
                    fs::write(&path, b"before").unwrap();
                }
                let mut guard = Guard::inspect(&path).unwrap();
                let outside = t.path().join("outside");
                fs::create_dir(&outside).unwrap();
                fs::write(outside.join("SKILL.md"), b"external").unwrap();
                match change {
                    "edit" | "appeared" => fs::write(&path, b"edit").unwrap(),
                    "inode" => {
                        let replacement = t.path().join("replacement");
                        fs::write(&replacement, b"before").unwrap();
                        fs::rename(replacement, &path).unwrap();
                    }
                    "ancestor_file" => {
                        fs::rename(t.path().join("skills"), t.path().join("old")).unwrap();
                        fs::write(t.path().join("skills"), b"file").unwrap();
                    }
                    "file" => {
                        fs::remove_file(&path).unwrap();
                        symlink(outside.join("SKILL.md"), &path).unwrap();
                    }
                    "parent" => {
                        fs::rename(&parent, t.path().join("old")).unwrap();
                        symlink(&outside, &parent).unwrap();
                    }
                    "ancestor" => {
                        fs::rename(t.path().join("skills"), t.path().join("old")).unwrap();
                        symlink(&outside, t.path().join("skills")).unwrap();
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
                assert_eq!(fs::read(outside.join("SKILL.md")).unwrap(), b"external");
            }
        }
    }
    #[test]
    fn replacing_an_existing_skill_swaps_atomically_and_leaves_no_temp() {
        let t = tempfile::tempdir().unwrap();
        let parent = t.path().join("skills/glen");
        fs::create_dir_all(&parent).unwrap();
        let path = parent.join("SKILL.md");
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
    fn forced_fallback_still_publishes_the_skill_without_exchange() {
        let t = tempfile::tempdir().unwrap();
        let parent = t.path().join("skills/glen");
        fs::create_dir_all(&parent).unwrap();
        let path = parent.join("SKILL.md");
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
}
