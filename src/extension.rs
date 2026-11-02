use crate::{error::Warning, install_guard, live};
use std::{cmp::Ordering, fs, io, path::Path};
use xxhash_rust::xxh64::xxh64;
pub const SOURCE: &str = include_str!("../extension/glen/index.ts");
pub const PROTOCOL: u32 = crate::ops::PROTOCOL;
pub fn render(version: &str, home: &Path) -> String {
    let body = SOURCE
        .split_once('\n')
        .map_or("", |(_, b)| b)
        .replace(
            "\"__GLEN_VERSION__\"",
            &serde_json::json!(version).to_string(),
        )
        .replace(
            "\"__GLEN_HOME__\"",
            &serde_json::json!(home.to_string_lossy()).to_string(),
        );
    format!(
        "// glen-extension version={version} protocol={PROTOCOL} digest={:016x}\n{body}",
        xxh64(body.as_bytes(), 0)
    )
}
#[derive(Debug, PartialEq, Eq)]
pub enum State {
    NotInstalled,
    Current,
    Outdated(String),
    Newer(String),
    DifferentHome,
    Modified,
    Foreign,
}
impl State {
    pub fn name(&self) -> &'static str {
        match self {
            Self::NotInstalled => "not_installed",
            Self::Current => "current",
            Self::Outdated(_) => "outdated",
            Self::Newer(_) => "newer",
            Self::DifferentHome => "different_home",
            Self::Modified => "modified",
            Self::Foreign => "foreign",
        }
    }
}
/// How `install-extension` writes `extensions/glen/index.ts`.
pub(crate) const GUARD: install_guard::Spec = install_guard::Spec {
    noun: "extension",
    temp_prefix: ".index.ts",
    file_mode: 0o600,
    dir_mode: 0o700,
};
#[derive(Debug, PartialEq, Eq)]
pub struct Inspection {
    pub state: State,
    /// The version in the installed header, when glen wrote it.
    pub version: Option<String>,
    /// A symlink under the extensions directory; never replaced, even with `--force`.
    pub unsafe_path: bool,
}
/// Why a symlinked extension path is left alone; doctor and install-extension share it.
pub fn unsafe_path_message(path: &Path) -> String {
    format!(
        "refusing symlink in extension path {}; --force cannot override this",
        path.display()
    )
}
/// Classify the extension at `path`. A symlink at or under `extensions_dir` is
/// `Foreign` with `unsafe_path`, before anything is read through it (W8 DEBT-2).
pub fn inspect(
    path: &Path,
    extensions_dir: &Path,
    version: &str,
    home: &Path,
) -> io::Result<Inspection> {
    if install_guard::Guard::inspect(path, GUARD)?.unsafe_path(extensions_dir) {
        return Ok(Inspection {
            state: State::Foreign,
            version: None,
            unsafe_path: true,
        });
    }
    let (state, version) = classify(path, version, home)?;
    Ok(Inspection {
        state,
        version,
        unsafe_path: false,
    })
}
fn classify(path: &Path, version: &str, home: &Path) -> io::Result<(State, Option<String>)> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((State::NotInstalled, None)),
        Err(e) => return Err(e),
    };
    let Some(end) = bytes.iter().position(|b| *b == b'\n') else {
        return Ok((State::Foreign, None));
    };
    let Ok(line) = std::str::from_utf8(&bytes[..end]) else {
        return Ok((State::Foreign, None));
    };
    let fields = line
        .strip_prefix("// glen-extension version=")
        .map(|s| s.split(' ').collect::<Vec<_>>());
    let Some(fields) = fields.filter(|f| f.len() == 3 && !f[0].is_empty()) else {
        return Ok((State::Foreign, None));
    };
    let valid_protocol = fields[1]
        .strip_prefix("protocol=")
        .is_some_and(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()));
    let digest = fields[2]
        .strip_prefix("digest=")
        .filter(|s| {
            s.len() == 16
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .and_then(|s| u64::from_str_radix(s, 16).ok());
    if !valid_protocol || digest.is_none() || fields[0].chars().any(char::is_whitespace) {
        return Ok((State::Foreign, None));
    }
    let installed = Some(fields[0].to_owned());
    if digest != Some(xxh64(&bytes[end + 1..], 0)) {
        return Ok((State::Modified, installed));
    }
    if home.to_str().is_some() && bytes == render(version, home).as_bytes() {
        return Ok((State::Current, installed));
    }
    let state = match crate::version::compare_versions(fields[0], version) {
        Ordering::Less => State::Outdated(fields[0].into()),
        Ordering::Greater => State::Newer(fields[0].into()),
        Ordering::Equal => {
            let baked = std::str::from_utf8(&bytes[end + 1..])
                .ok()
                .and_then(|body| {
                    body.lines()
                        .find_map(|line| line.strip_prefix("const BAKED_GLEN_HOME = "))
                })
                .and_then(|literal| {
                    serde_json::from_str::<String>(literal.trim_end_matches(';')).ok()
                });
            if baked.as_deref().map(Path::new) == Some(home) {
                State::Outdated(format!("{} (older build)", fields[0]))
            } else {
                State::DifferentHome
            }
        }
    };
    Ok((state, installed))
}
pub fn socket_path_warning(home: &Path) -> Option<Warning> {
    let p = home.join("run/live/9999999.sock");
    (!live::socket_path_ok(&p)).then(||Warning{code:"socket_path_too_long".into(),message:format!("GLEN_HOME is too long for a unix socket path ({} bytes, limit {}); live delivery will be off; shorten GLEN_HOME",p.as_os_str().len(),live::SUN_PATH_MAX)})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_contract() {
        for (declaration, expected) in [
            ("export const OPS", crate::ops::IMPLEMENTED),
            ("export const RESERVED_OPS", crate::ops::RESERVED),
        ] {
            let literal = SOURCE
                .lines()
                .find(|line| line.starts_with(declaration))
                .unwrap()
                .split_once('=')
                .unwrap()
                .1
                .trim()
                .trim_end_matches(';');
            let actual: Vec<String> = serde_json::from_str(literal).unwrap();
            assert_eq!(actual, expected);
        }
        for marker in ["__GLEN_HOME__", "__GLEN_VERSION__"] {
            assert_eq!(SOURCE.matches(marker).count(), 1);
        }
        assert!(SOURCE.contains(&format!("export const PROTOCOL = {PROTOCOL};")));
        assert!(!SOURCE.contains("Bun."));
        for line in SOURCE.lines() {
            if line.contains("setTimeout(") || line.contains("setInterval(") {
                assert!(
                    line.contains("ctx.setTimeout(")
                        || line.contains("ctx.setInterval(")
                        || line.contains("conn.setTimeout(")
                );
            }
            if line.starts_with("import ") {
                assert!(line.contains("\"node:") || line.starts_with("import type "));
            }
        }
        let rendered = render("0.1.0", Path::new("/tmp/a\"\\b"));
        assert!(!rendered.contains("__GLEN_"));
        let literal = rendered
            .lines()
            .find_map(|l| l.strip_prefix("const BAKED_GLEN_HOME = "))
            .unwrap()
            .trim_end_matches(';');
        assert_eq!(
            serde_json::from_str::<String>(literal).unwrap(),
            "/tmp/a\"\\b"
        );
    }
    #[test]
    fn inspection_table() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("index.ts");
        let h = Path::new("/tmp/g");
        assert_eq!(
            inspect(&p, t.path(), "0.1.0", h).unwrap().state,
            State::NotInstalled
        );
        for (body, want) in [
            (render("0.1.0", h), State::Current),
            (render("0.0.1", h), State::Outdated("0.0.1".into())),
            (render("9.0.0", h), State::Newer("9.0.0".into())),
            (
                render("0.1.0", Path::new("/tmp/other")),
                State::DifferentHome,
            ),
            (format!("{}changed", render("0.1.0", h)), State::Modified),
            ("foreign".into(), State::Foreign),
        ] {
            fs::write(&p, body).unwrap();
            assert_eq!(inspect(&p, t.path(), "0.1.0", h).unwrap().state, want);
        }
    }
    #[test]
    fn same_version_intact_older_body_is_outdated_only_for_the_same_home() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("index.ts");
        let home = Path::new("/tmp/g");
        for (baked, expected) in [
            (home, State::Outdated("0.1.0 (older build)".into())),
            (Path::new("/tmp/other"), State::DifferentHome),
        ] {
            let old = render("0.1.0", baked);
            let body = old
                .split_once('\n')
                .unwrap()
                .1
                .replace("export const OPS =", "export const OLD_OPS =");
            let installed = format!(
                "// glen-extension version=0.1.0 protocol=1 digest={:016x}\n{body}",
                xxh64(body.as_bytes(), 0)
            );
            fs::write(&path, installed).unwrap();
            assert_eq!(
                inspect(&path, t.path(), "0.1.0", home).unwrap().state,
                expected
            );
        }
    }
    #[test]
    fn pre_release_versions_order_by_semver() {
        // W8 DEBT-5: 0.1.0 is older than 0.2.0-rc.1, and rc.10 is newer than rc.9.
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("index.ts");
        let h = Path::new("/tmp/g");
        for (installed, current, want) in [
            ("0.1.0", "0.2.0-rc.1", State::Outdated("0.1.0".into())),
            ("0.2.0-rc.1", "0.1.1", State::Newer("0.2.0-rc.1".into())),
            (
                "0.2.0-rc.10",
                "0.2.0-rc.9",
                State::Newer("0.2.0-rc.10".into()),
            ),
            ("0.2.0-rc.1", "0.2.0", State::Outdated("0.2.0-rc.1".into())),
        ] {
            fs::write(&p, render(installed, h)).unwrap();
            assert_eq!(
                inspect(&p, t.path(), current, h).unwrap().state,
                want,
                "{installed} vs {current}"
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_under_the_extensions_dir_are_foreign_and_unsafe() {
        use std::os::unix::fs::symlink;
        let h = Path::new("/tmp/g");
        for component in ["extensions", "glen", "index.ts"] {
            let t = tempfile::tempdir().unwrap();
            let outside = t.path().join("outside");
            fs::create_dir_all(outside.join("glen")).unwrap();
            fs::write(outside.join("index.ts"), render("0.0.9", h)).unwrap();
            fs::write(outside.join("glen/index.ts"), render("0.0.9", h)).unwrap();
            let extensions = t.path().join("extensions");
            match component {
                "extensions" => symlink(&outside, &extensions).unwrap(),
                "glen" => {
                    fs::create_dir(&extensions).unwrap();
                    symlink(&outside, extensions.join("glen")).unwrap();
                }
                _ => {
                    fs::create_dir_all(extensions.join("glen")).unwrap();
                    symlink(outside.join("index.ts"), extensions.join("glen/index.ts")).unwrap();
                }
            }
            let i = inspect(&extensions.join("glen/index.ts"), &extensions, "0.1.0", h).unwrap();
            assert_eq!(
                i,
                Inspection {
                    state: State::Foreign,
                    version: None,
                    unsafe_path: true
                },
                "{component}"
            );
        }
        // A symlink above the extensions directory is the owner's layout, not ours to judge.
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("real");
        fs::create_dir_all(real.join("extensions/glen")).unwrap();
        fs::write(real.join("extensions/glen/index.ts"), render("0.1.0", h)).unwrap();
        symlink(&real, t.path().join("agent")).unwrap();
        let extensions = t.path().join("agent/extensions");
        let i = inspect(&extensions.join("glen/index.ts"), &extensions, "0.1.0", h).unwrap();
        assert_eq!(i.state, State::Current);
        assert!(!i.unsafe_path);
    }
}
