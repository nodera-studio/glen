//! Semver precedence without the `semver` crate, and the normalizer for `omp --version`.
use std::cmp::Ordering;
/// A pre-release identifier; a number sorts below a word.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Identifier {
    Number(u64),
    Word(String),
}
/// The precedence key of `major.minor.patch[-pre.release][+build]`: the numeric core, then
/// whether it is a release (above its pre-releases), then the pre-release identifiers. Build
/// metadata is ignored, so the derived order is semver precedence.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    core: [u64; 3],
    release: bool,
    pre: Vec<Identifier>,
}
impl Version {
    /// Parses `major.minor.patch[-pre.release][+build]`; build metadata after the first `+` is
    /// dropped. Pre-release identifiers are free text, so callers that show the input must
    /// check its characters themselves (as [`parse_omp_line`] does).
    pub fn parse(v: &str) -> Option<Version> {
        let v = v.split_once('+').map_or(v, |(v, _)| v);
        let (core, pre) = v.split_once('-').map_or((v, None), |(c, p)| (c, Some(p)));
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        let mut parts = core.split('.');
        let mut number = || parts.next().filter(|s| digits(s))?.parse::<u64>().ok();
        let core = [number()?, number()?, number()?];
        if parts.next().is_some() {
            return None;
        }
        let mut identifiers = Vec::new();
        for id in pre.map(|p| p.split('.')).into_iter().flatten() {
            if id.is_empty() {
                return None;
            }
            identifiers.push(match id.parse() {
                Ok(n) if digits(id) => Identifier::Number(n),
                _ => Identifier::Word(id.to_owned()),
            });
        }
        Some(Version {
            core,
            release: pre.is_none(),
            pre: identifiers,
        })
    }
}
/// Semver precedence (W8 DEBT-5): numeric `major.minor.patch`, a release above its
/// pre-releases, pre-release identifiers compared numerically when both are numbers and as
/// ASCII otherwise. A version that does not parse sorts below every valid one; two such
/// versions compare equal.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    match (Version::parse(a), Version::parse(b)) {
        (Some(a), Some(b)) => a.cmp(&b),
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}
/// A version read from `omp --version`: `text` is the normalized form (no `omp/` or `v`
/// prefix, pre-release and build metadata kept).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmpVersion {
    pub text: String,
    pub version: Version,
}
/// Normalizes one `omp --version` line: `[omp/][v]MAJOR.MINOR.PATCH[-PRE][+BUILD]`, trimmed,
/// where `PRE` and `BUILD` use only `[0-9A-Za-z.-]`. Anything else, an empty line included,
/// is `None`; the raw line is never kept.
pub fn parse_omp_line(line: &str) -> Option<OmpVersion> {
    let s = line.trim();
    let s = s.strip_prefix("omp/").unwrap_or(s);
    let s = s.strip_prefix('v').unwrap_or(s);
    let allowed = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+');
    if s.is_empty() || !s.bytes().all(allowed) {
        return None;
    }
    if let Some((_, build)) = s.split_once('+')
        && (build.is_empty() || build.contains('+'))
    {
        return None;
    }
    Some(OmpVersion {
        text: s.to_owned(),
        version: Version::parse(s)?,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn precedence() {
        for (a, b, expected) in [
            ("0.0.9", "0.1.0", Ordering::Less),
            ("0.1.10", "0.1.9", Ordering::Greater),
            ("dev", "0.1.0", Ordering::Less),
            // W8 DEBT-5: pre-releases.
            ("0.1.0", "0.2.0-rc.1", Ordering::Less),
            ("0.2.0-rc.1", "0.1.1", Ordering::Greater),
            ("0.2.0-rc.10", "0.2.0-rc.9", Ordering::Greater),
            ("0.2.0-rc.1", "0.2.0", Ordering::Less),
            ("0.2.0-rc.1", "0.2.0-rc.1.1", Ordering::Less),
            ("0.2.0-1", "0.2.0-alpha", Ordering::Less),
            ("0.2.0-alpha", "0.2.0-beta", Ordering::Less),
            ("0.2.0+build.1", "0.2.0", Ordering::Equal),
            ("0.2.0-", "0.1.0", Ordering::Less),
            ("0.2", "0.1.0", Ordering::Less),
            ("dev", "nightly", Ordering::Equal),
        ] {
            assert_eq!(compare_versions(a, b), expected, "{a} vs {b}");
        }
    }
    #[test]
    fn omp_lines_normalize() {
        for (line, text) in [
            ("omp/18.14.0", "18.14.0"),
            ("18.14.0", "18.14.0"),
            ("v18.14.0", "18.14.0"),
            ("omp/v18.14.0", "18.14.0"),
            (" 18.0.9 ", "18.0.9"),
            ("18.15.0-rc.1", "18.15.0-rc.1"),
            ("18.14.0+b1", "18.14.0+b1"),
        ] {
            let v = parse_omp_line(line).unwrap_or_else(|| panic!("{line:?} parses"));
            assert_eq!(v.text, text, "{line:?}");
            assert_eq!(Some(v.version), Version::parse(text));
        }
        for line in [
            "",
            "garbage",
            "18.14",
            "18.14.0.1",
            "18.14.0-",
            "18.14.0 extra",
            "18.14.0\u{1b}[31m",
            "18.14.0-rc.1\u{1b}[31m",
            "omp/",
            "v",
            "18.14.0+",
            "18.14.0+a+b",
        ] {
            assert_eq!(parse_omp_line(line), None, "{line:?}");
        }
        let v = |s: &str| parse_omp_line(s).unwrap().version;
        assert!(v("18.14.0-rc.1") < v("18.14.0"));
        assert_eq!(v("18.14.0+b1"), v("18.14.0"));
        assert!(v("18.15.0-rc.1") > v("18.14.0"));
    }
}
