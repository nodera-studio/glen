//! Pin directory descriptors and revalidate the inspected identities before publishing.
//!
//! Shared by `install-extension` and `install-skill` (W8 DEBT-4); a [`Spec`] carries what differs
//! between the two: the noun in messages, the temp-file prefix and the modes.
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    fs, io,
    path::{Path, PathBuf},
};
use xxhash_rust::xxh64::xxh64;

#[cfg(unix)]
use crate::atomic_swap;
use crate::atomic_swap::Applied;

/// What one installed file looks like on disk.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Spec {
    /// The noun used in error and warning messages, e.g. "extension".
    pub(crate) noun: &'static str,
    /// The temp file is `{temp_prefix}.{uuid}.tmp`, next to the target.
    pub(crate) temp_prefix: &'static str,
    pub(crate) file_mode: u32,
    /// Mode for directories this install creates.
    pub(crate) dir_mode: u32,
}

#[derive(Debug, PartialEq, Eq)]
struct Identity {
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    digest: Option<u64>,
    link: Option<PathBuf>,
}
fn identity(path: &Path, noun: &str) -> io::Result<Option<Identity>> {
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
            return Err(io::Error::other(format!(
                "{noun} changed to a non-regular file"
            )));
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
    spec: Spec,
    entries: Vec<(PathBuf, Option<Identity>)>,
    /// Set when the file was published but its directory could not be synced.
    pub(crate) sync_warning: Option<String>,
}
impl Guard {
    pub(crate) fn inspect(path: &Path, spec: Spec) -> io::Result<Self> {
        let entries = path
            .ancestors()
            .map(|p| Ok((p.to_path_buf(), identity(p, spec.noun)?)))
            .collect::<io::Result<_>>()?;
        Ok(Self {
            spec,
            entries,
            sync_warning: None,
        })
    }
    /// Whether the target or one of its ancestors under `scope` is a symlink.
    pub(crate) fn unsafe_path(&self, scope: &Path) -> bool {
        self.entries.iter().any(|(path, id)| {
            path.starts_with(scope) && id.as_ref().is_some_and(|id| id.link.is_some())
        })
    }
    pub(crate) fn unchanged(&self) -> bool {
        for (path, before) in &self.entries {
            if !matches!(identity(path, self.spec.noun), Ok(ref now) if now == before) {
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
        let spec = self.spec;
        let noun = spec.noun;
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
                .ok_or_else(|| io::Error::other(format!("{noun} has no parent")))?;
            let nearest = self
                .entries
                .iter()
                .skip(1)
                .find(|(_, i)| i.is_some())
                .ok_or_else(|| io::Error::other(format!("{noun} has no existing ancestor")))?
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
                    at::mkdirat(
                        &dir,
                        part.as_os_str(),
                        Mode::from_raw_mode(spec.dir_mode as _),
                    )
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
                    *before = identity(p, noun)?;
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
                .ok_or_else(|| io::Error::other(format!("{noun} has no file name")))?;
            let Some(bytes) = bytes else {
                if !self.unchanged() {
                    return Ok(Applied::Lost);
                }
                at::unlinkat(&dir, name, AtFlags::empty())?;
                published = true;
                return Ok(Applied::Published);
            };
            let existing = self.entries[0].1.as_ref().map(|i| (i.dev, i.ino, i.digest));
            let temp = format!("{}.{}.tmp", spec.temp_prefix, uuid::Uuid::now_v7());
            let fd = at::openat(
                &dir,
                &temp,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(spec.file_mode as _),
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
                        noun,
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
                eprintln!("warning: cannot clean temporary {noun} file {temp}: {e}");
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
                        "warning: cannot clean {noun} directory {}: {e}",
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
        let noun = self.spec.noun;
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
                .ok_or_else(|| io::Error::other(format!("{noun} has no parent")))?;
            let grandparent = parent
                .parent()
                .ok_or_else(|| io::Error::other(format!("{noun} has no grandparent")))?;
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
                    .ok_or_else(|| io::Error::other(format!("{noun} directory has no name")))?,
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
                .ok_or_else(|| io::Error::other(format!("{} has no parent", self.spec.noun)))?,
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
                    .ok_or_else(|| io::Error::other(format!("{} has no parent", self.spec.noun)))?,
            )?;
            if !self.unchanged() {
                return Ok(Applied::Lost);
            }
            crate::glen_home::write_atomic(path, bytes, self.spec.file_mode)?;
        } else {
            fs::remove_file(path)?;
        }
        Ok(Applied::Published)
    }
}
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    /// Each spec with the directory and file name its command installs to.
    fn cases() -> [(Spec, &'static str, &'static str); 2] {
        [
            (crate::extension::GUARD, "extensions", "index.ts"),
            (crate::skill::GUARD, "skills", "SKILL.md"),
        ]
    }
    #[test]
    fn created_ancestors_cleaned_on_mkdir_failure_and_late_change() {
        for (spec, top, file) in cases() {
            for failure in ["mkdir", "write", "revalidate"] {
                let t = tempfile::tempdir().unwrap();
                let root = t.path().join("scope");
                fs::create_dir(&root).unwrap();
                let path = root.join(top).join("glen").join(file);
                let mut guard = Guard::inspect(&path, spec).unwrap();
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
                assert!(!root.join(top).exists(), "{} {failure}", spec.noun);
                assert!(!old.join(top).exists(), "{} {failure}", spec.noun);
            }
        }
    }
    #[test]
    fn published_file_survives_directory_fsync_failure_as_success() {
        for (spec, _, file) in cases() {
            let t = tempfile::tempdir().unwrap();
            let path = t.path().join(file);
            let mut guard = Guard::inspect(&path, spec).unwrap();
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
    }
    #[test]
    fn temp_cleanup_preserves_original_error() {
        for (spec, _, file) in cases() {
            let t = tempfile::tempdir().unwrap();
            let path = t.path().join(file);
            let mut guard = Guard::inspect(&path, spec).unwrap();
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
    }
    #[test]
    fn parent_swapped_at_unlink_is_changed_skip() {
        for (spec, _, file) in cases() {
            let t = tempfile::tempdir().unwrap();
            let parent = t.path().join("glen");
            fs::create_dir(&parent).unwrap();
            let path = parent.join(file);
            fs::write(&path, b"old").unwrap();
            let mut guard = Guard::inspect(&path, spec).unwrap();
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
    }
    #[test]
    fn competing_create_never_clobbers_the_appeared_file() {
        for (spec, top, file) in cases() {
            let t = tempfile::tempdir().unwrap();
            let parent = t.path().join(top).join("glen");
            fs::create_dir_all(&parent).unwrap();
            let path = parent.join(file);
            let mut guard = Guard::inspect(&path, spec).unwrap();
            fs::write(&path, b"external").unwrap();
            assert_eq!(
                guard.apply(&path, Some(b"replacement")).unwrap(),
                Applied::Lost
            );
            assert_eq!(fs::read(&path).unwrap(), b"external");
        }
    }
    #[test]
    fn competing_edits_replacements_and_symlinks_never_publish_or_unlink() {
        for (spec, top, file) in cases() {
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
                    let parent = t.path().join(top).join("glen");
                    fs::create_dir_all(&parent).unwrap();
                    let path = parent.join(file);
                    if change != "appeared" {
                        fs::write(&path, b"before").unwrap();
                    }
                    let mut guard = Guard::inspect(&path, spec).unwrap();
                    let outside = t.path().join("outside");
                    fs::create_dir(&outside).unwrap();
                    fs::write(outside.join(file), b"external").unwrap();
                    match change {
                        "edit" | "appeared" => fs::write(&path, b"edit").unwrap(),
                        "inode" => {
                            let replacement = t.path().join("replacement");
                            fs::write(&replacement, b"before").unwrap();
                            fs::rename(replacement, &path).unwrap();
                        }
                        "ancestor_file" => {
                            fs::rename(t.path().join(top), t.path().join("old")).unwrap();
                            fs::write(t.path().join(top), b"file").unwrap();
                        }
                        "file" => {
                            fs::remove_file(&path).unwrap();
                            symlink(outside.join(file), &path).unwrap();
                        }
                        "parent" => {
                            fs::rename(&parent, t.path().join("old")).unwrap();
                            symlink(&outside, &parent).unwrap();
                        }
                        "ancestor" => {
                            fs::rename(t.path().join(top), t.path().join("old")).unwrap();
                            symlink(&outside, t.path().join(top)).unwrap();
                        }
                        _ => unreachable!(),
                    }
                    assert_eq!(
                        guard
                            .apply(&path, if remove { None } else { Some(b"replacement") })
                            .unwrap(),
                        Applied::Lost,
                        "{} {change}",
                        spec.noun
                    );
                    assert_eq!(fs::read(outside.join(file)).unwrap(), b"external");
                }
            }
        }
    }
    #[test]
    fn competing_replacement_during_the_swap_window_is_lost_not_clobbered() {
        for (spec, top, file) in cases() {
            let t = tempfile::tempdir().unwrap();
            let parent = t.path().join(top).join("glen");
            fs::create_dir_all(&parent).unwrap();
            let path = parent.join(file);
            fs::write(&path, b"before").unwrap();
            let mut guard = Guard::inspect(&path, spec).unwrap();
            // A competing writer replaces the file in place (same inode story as a plain
            // edit): unchanged() must catch this before any swap is attempted.
            fs::write(&path, b"competitor").unwrap();
            assert_eq!(guard.apply(&path, Some(b"after")).unwrap(), Applied::Lost);
            assert_eq!(fs::read(&path).unwrap(), b"competitor");
        }
    }
    #[test]
    fn replacing_an_existing_file_swaps_atomically_and_leaves_no_temp() {
        for (spec, top, file) in cases() {
            let t = tempfile::tempdir().unwrap();
            let parent = t.path().join(top).join("glen");
            fs::create_dir_all(&parent).unwrap();
            let path = parent.join(file);
            fs::write(&path, b"before").unwrap();
            let mut guard = Guard::inspect(&path, spec).unwrap();
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
    }
    #[test]
    fn forced_fallback_still_publishes_without_exchange() {
        for (spec, top, file) in cases() {
            let t = tempfile::tempdir().unwrap();
            let parent = t.path().join(top).join("glen");
            fs::create_dir_all(&parent).unwrap();
            let path = parent.join(file);
            fs::write(&path, b"before").unwrap();
            let mut guard = Guard::inspect(&path, spec).unwrap();
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
    #[test]
    fn created_file_and_directories_use_the_spec_modes() {
        for (spec, top, file) in cases() {
            let t = tempfile::tempdir().unwrap();
            let path = t.path().join(top).join("glen").join(file);
            let mut guard = Guard::inspect(&path, spec).unwrap();
            assert_eq!(
                guard.apply(&path, Some(b"new")).unwrap(),
                Applied::Published
            );
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            // The process umask can only clear bits, never add them.
            assert_eq!(mode(&path) & !spec.file_mode, 0, "{}", spec.noun);
            assert_eq!(
                mode(path.parent().unwrap()) & !spec.dir_mode,
                0,
                "{}",
                spec.noun
            );
        }
    }
}
