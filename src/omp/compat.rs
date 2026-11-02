//! The supported omp range. `compat.json` is the single source: Rust, the nightly
//! `omp-latest` workflow and the site read it, and `TESTED_OMP_VERSION` derives from it.
use crate::version::Version;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::OnceLock;
const RAW: &str = include_str!("compat.json");
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    min: String,
    max_tested: String,
    tested: String,
}
#[derive(Debug)]
pub struct Compat {
    pub min: Version,
    pub max_tested: Version,
    /// The omp version glen's own tests and the `omp-rpc` pin target.
    pub tested: String,
    pub min_text: String,
    pub max_tested_text: String,
}
impl Compat {
    /// `{"min","max_tested"}`, as `doctor.data.omp.supported`.
    pub fn supported(&self) -> Value {
        json!({"min":self.min_text,"max_tested":self.max_tested_text})
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    Supported,
    Untested,
    Unsupported,
}
fn parse(raw: &str) -> Result<Compat, String> {
    let raw: Raw = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    let version = |key: &str, text: &str| {
        Version::parse(text).ok_or_else(|| format!("{key} {text:?} is not a version"))
    };
    let compat = Compat {
        min: version("min", &raw.min)?,
        max_tested: version("max_tested", &raw.max_tested)?,
        tested: raw.tested,
        min_text: raw.min,
        max_tested_text: raw.max_tested,
    };
    let tested = version("tested", &compat.tested)?;
    if !(compat.min <= tested && tested <= compat.max_tested) {
        return Err("tested must lie within min..=max_tested".into());
    }
    Ok(compat)
}
/// The checked-in range. A broken `compat.json` panics here; the unit tests keep it from
/// shipping.
pub fn get() -> &'static Compat {
    static COMPAT: OnceLock<Compat> = OnceLock::new();
    COMPAT.get_or_init(|| {
        parse(RAW).unwrap_or_else(|e| panic!("src/omp/compat.json is invalid: {e}"))
    })
}
/// Below `min` is unsupported (a pre-release of `min` included), above `max_tested` is
/// untested, otherwise supported.
pub fn classify_in(min: &Version, max_tested: &Version, version: &Version) -> Support {
    if version < min {
        Support::Unsupported
    } else if version > max_tested {
        Support::Untested
    } else {
        Support::Supported
    }
}
pub fn classify(version: &Version) -> Support {
    let c = get();
    classify_in(&c.min, &c.max_tested, version)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn v(s: &str) -> Version {
        crate::version::parse_omp_line(s).unwrap().version
    }
    #[test]
    fn checked_in_range_is_valid() {
        let c = get();
        assert!(c.min <= c.max_tested);
        assert!(!c.tested.contains('-'), "tested has no pre-release");
        assert_eq!(*crate::omp::TESTED_OMP_VERSION, c.tested);
        assert_eq!(
            c.supported(),
            json!({"min":c.min_text,"max_tested":c.max_tested_text})
        );
        // One key per line, exactly these keys: the nightly and the site read the file.
        let keys: Vec<_> = RAW
            .lines()
            .filter_map(|l| l.trim().strip_prefix('"')?.split('"').next())
            .collect();
        assert_eq!(keys, ["min", "max_tested", "tested"]);
    }
    #[test]
    fn tested_matches_the_omp_rpc_tag() {
        let line = include_str!("../../Cargo.toml")
            .lines()
            .find(|l| l.starts_with("omp-rpc"))
            .unwrap();
        let tag = line
            .split("tag = \"v")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .unwrap();
        assert_eq!(
            tag,
            get().tested,
            "bump compat.json and the omp-rpc tag together"
        );
    }
    #[test]
    fn broken_files_are_rejected() {
        for raw in [
            "{}",
            r#"{"min":"18.14.0","max_tested":"18.14.0"}"#,
            r#"{"min":"x","max_tested":"18.14.0","tested":"18.14.0"}"#,
            r#"{"min":"18.14.0","max_tested":"18.14.0","tested":"18.15.0"}"#,
            r#"{"min":"18.14.0","max_tested":"18.14.0","tested":"18.14.0","extra":1}"#,
        ] {
            assert!(parse(raw).is_err(), "{raw}");
        }
    }
    #[test]
    fn classification() {
        use Support::*;
        let (min, max) = (v("18.10.0"), v("18.14.0"));
        for (version, want) in [
            ("18.10.0", Supported),
            ("18.12.3", Supported),
            ("18.14.0+b1", Supported),
            ("18.14.0-rc.1", Supported),
            ("18.10.0-rc.1", Unsupported),
            ("18.9.9", Unsupported),
            ("18.14.1", Untested),
            ("18.15.0-rc.1", Untested),
        ] {
            assert_eq!(classify_in(&min, &max, &v(version)), want, "{version}");
        }
        // The real file: min = max_tested = 18.14.0 at v0.2 W1.
        for (version, want) in [
            ("18.14.0", Supported),
            ("18.14.0+b1", Supported),
            ("v18.14.0", Supported),
            ("18.14.1", Untested),
            ("18.15.0-rc.1", Untested),
            ("19.0.0", Untested),
            ("18.14.0-rc.1", Unsupported),
            ("18.13.9", Unsupported),
            ("18.0.9", Unsupported),
            ("17.99.0", Unsupported),
        ] {
            assert_eq!(classify(&v(version)), want, "{version}");
        }
    }
}
