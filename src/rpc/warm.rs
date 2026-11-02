//! The opt-in warm worker (v0.2 W4, design §A21): settings, the match key, the on-disk
//! registry and release request, the claim, the idle loop and its one teardown.
use crate::{
    env::Env,
    error::{ErrorCode, GlenError},
    jobs::Job,
};
use std::{path::Path, time::Duration};

/// The longest a worker may stay warm after a job (`--warm`, `GLEN_WARM`).
pub const MAX_WARM: Duration = Duration::from_secs(600);
/// One idle-loop tick: a queued job waits at most this long behind an idle warm worker.
pub const CLAIM_TICK: Duration = Duration::from_millis(100);
/// How often an idle warm worker touches its registry (the heartbeat is the file's mtime).
pub const HEARTBEAT: Duration = Duration::from_secs(1);
/// An idle registry whose mtime is older than this has a stale heartbeat.
pub const STALE: Duration = Duration::from_secs(5);
/// The longest teardown waits for `Client::close` before it frees the session anyway.
pub const TEARDOWN_BOUND: Duration = Duration::from_millis(3500);
/// Variables that differ between otherwise equal environments; `env_digest` skips them, and
/// every `GLEN_` variable.
pub const VOLATILE_ENV: &[&str] = &[
    "_",
    "PWD",
    "OLDPWD",
    "SHLVL",
    "COLUMNS",
    "LINES",
    "TERM_SESSION_ID",
];

/// `--warm`'s value parser: a `parse_duration` value of at most 10m (`0` means off).
pub fn parse_warm(raw: &str) -> Result<Duration, String> {
    let d = crate::cli::parse_duration(raw)?;
    if d > MAX_WARM {
        Err("warm duration must be at most 10m (0 = off)".into())
    } else {
        Ok(d)
    }
}

/// The effective warm duration of a send: the flag if given (even `0`), else `GLEN_WARM`, else
/// off. `None` means off. A bad or over-cap `GLEN_WARM` is `invalid_env`.
pub fn resolve_warm(flag: Option<Duration>, env: &Env) -> Result<Option<Duration>, GlenError> {
    let d = match flag {
        Some(d) => d,
        None => match env.non_empty("GLEN_WARM") {
            None => return Ok(None),
            Some(raw) => parse_warm(raw.trim()).map_err(|e| {
                GlenError::new(ErrorCode::InvalidEnv, format!("GLEN_WARM: {e}"))
                    .with_hint("set GLEN_WARM to a duration such as 5m (at most 10m), or unset it")
                    .with_detail("variable", "GLEN_WARM")
            })?,
        },
    };
    Ok((!d.is_zero()).then_some(d))
}

/// A warm duration as `job.json` records it, from a `send`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setting {
    pub duration: Duration,
    /// Given as `--warm`, not through `GLEN_WARM`: only an explicit flag warns on the live route.
    pub explicit: bool,
}

/// xxh64 (seed 0) of the sorted `KEY=VALUE\n` lines of `env`, without [`VOLATILE_ENV`] and
/// `GLEN_` variables, as 16 lowercase hex digits. No value is ever recorded, only this digest.
pub fn env_digest(env: &Env) -> String {
    let mut bytes = Vec::new();
    for (k, v) in env.iter() {
        if VOLATILE_ENV.contains(&k) || k.starts_with("GLEN_") {
            continue;
        }
        bytes.extend_from_slice(k.as_bytes());
        bytes.push(b'=');
        bytes.extend_from_slice(v.as_bytes());
        bytes.push(b'\n');
    }
    format!("{:016x}", xxhash_rust::xxh64::xxh64(&bytes, 0))
}

/// What a queued job must share with the warm worker's own job to run on its omp.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub omp_bin: Option<std::path::PathBuf>,
    pub profile_flag: Option<String>,
    pub thinking_flag: Option<String>,
    pub cwd: std::path::PathBuf,
    pub env_digest: String,
}
impl Key {
    /// `None` for a job without an `env_digest` (written before v0.2 W4): it never goes warm.
    pub fn of(job: &Job) -> Option<Self> {
        Some(Self {
            omp_bin: job.omp_bin.clone(),
            profile_flag: job.profile_flag.clone(),
            thinking_flag: job.thinking_flag.clone(),
            cwd: job.cwd.clone(),
            env_digest: job.env_digest.clone()?,
        })
    }
    /// Exact on every field but `cwd`, which compares as a canonical path (a `@new --cwd` through
    /// a symlink and the session header's cwd name one directory).
    pub fn matches(&self, job: &Job) -> bool {
        job.env_digest.as_deref() == Some(self.env_digest.as_str())
            && job.omp_bin == self.omp_bin
            && job.profile_flag == self.profile_flag
            && job.thinking_flag == self.thinking_flag
            && same_dir(&job.cwd, &self.cwd)
    }
}
fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;
    fn env(vars: &[(&str, &str)]) -> Env {
        Env::for_test(Some(Path::new("/h")), Path::new("/w"), vars)
    }
    #[test]
    fn parse_warm_table() {
        for (raw, ms) in [
            ("0", 0),
            ("1500ms", 1500),
            ("2s", 2000),
            ("5", 5000),
            ("10m", 600_000),
        ] {
            assert_eq!(parse_warm(raw), Ok(Duration::from_millis(ms)), "{raw}");
        }
        for raw in ["11m", "600001ms", "1h", "abc", "", "-1", "2d"] {
            assert!(parse_warm(raw).is_err(), "{raw}");
        }
        assert!(parse_warm("11m").unwrap_err().contains("10m"));
    }
    #[test]
    fn resolve_warm_table() {
        let s = |n| Some(Duration::from_secs(n));
        for (flag, var, want) in [
            (None, None, None),
            (s(2), None, s(2)),
            (s(1), Some("5s"), s(1)),
            (Some(Duration::ZERO), Some("5s"), None),
            (None, Some("3s"), s(3)),
            (None, Some("0"), None),
            (None, Some(""), None),
            (None, Some("  "), None),
            (None, Some(" 4s "), s(4)),
            (s(600), None, s(600)),
        ] {
            let vars: Vec<_> = var.map(|v| ("GLEN_WARM", v)).into_iter().collect();
            assert_eq!(
                resolve_warm(flag, &env(&vars)).unwrap(),
                want,
                "{flag:?} {var:?}"
            );
        }
        for bad in ["11m", "abc", "1h"] {
            let e = resolve_warm(None, &env(&[("GLEN_WARM", bad)])).unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidEnv, "{bad}");
            assert!(e.message.contains("GLEN_WARM"), "{}", e.message);
            // A valid flag wins over a bad variable.
            assert_eq!(
                resolve_warm(s(2), &env(&[("GLEN_WARM", bad)])).unwrap(),
                s(2)
            );
        }
    }
    #[test]
    fn env_digest_is_stable_and_skips_volatile_and_glen_variables() {
        let base = [
            ("HOME", "/h"),
            ("PATH", "/bin"),
            ("PI_CONFIG_FILES", "a"),
            ("OMP_PROFILE", "p"),
            ("PROVIDER_API_KEY", "k"),
        ];
        let digest = env_digest(&env(&base));
        assert_eq!(digest.len(), 16);
        assert!(
            digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        assert_eq!(env_digest(&env(&base)), digest);
        for (k, v) in [
            ("GLEN_JOB_TIMEOUT", "30s"),
            ("GLEN_WARM", "5m"),
            ("GLEN_HOME", "/g"),
            ("PWD", "/x"),
            ("SHLVL", "3"),
            ("_", "/usr/bin/glen"),
            ("TERM_SESSION_ID", "w0t0p0"),
        ] {
            let mut vars = base.to_vec();
            vars.push((k, v));
            assert_eq!(env_digest(&env(&vars)), digest, "{k}");
        }
        for (i, changed) in [
            ("PI_CONFIG_FILES", "b"),
            ("OMP_PROFILE", "q"),
            ("HOME", "/other"),
            ("PATH", "/usr/bin"),
            ("PROVIDER_API_KEY", "other"),
        ]
        .into_iter()
        .enumerate()
        {
            let mut vars = base.to_vec();
            vars[i] = changed;
            assert_ne!(env_digest(&env(&vars)), digest, "{changed:?}");
        }
        let mut vars = base.to_vec();
        vars.push(("OTHER_API_KEY", "new"));
        assert_ne!(env_digest(&env(&vars)), digest);
    }
    #[test]
    fn key_matches_field_by_field_and_cwd_canonically() {
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("project");
        let other = t.path().join("other");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let link = t.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let mut warm = crate::jobs::tests::job();
        warm.cwd = link.clone();
        warm.env_digest = Some("0123456789abcdef".into());
        let key = Key::of(&warm).unwrap();
        let mut job = warm.clone();
        job.cwd = real.clone();
        assert!(key.matches(&job), "a symlinked and a canonical path match");
        job.cwd = real.join(".");
        assert!(key.matches(&job));
        type Change = fn(&mut Job, &Path);
        let cases: [(&str, Change); 6] = [
            ("omp_bin", |j, _| j.omp_bin = Some("/bin/other_omp".into())),
            ("profile", |j, _| j.profile_flag = Some("other".into())),
            ("thinking", |j, _| j.thinking_flag = Some("high".into())),
            ("cwd", |j, other| j.cwd = other.to_path_buf()),
            ("digest", |j, _| {
                j.env_digest = Some("fedcba9876543210".into())
            }),
            ("no digest", |j, _| j.env_digest = None),
        ];
        for (name, change) in cases {
            let mut job = warm.clone();
            change(&mut job, &other);
            assert!(!key.matches(&job), "{name}");
        }
        // A path that cannot be canonicalised compares lexically.
        let mut missing = warm.clone();
        missing.cwd = "/nonexistent/glen-warm".into();
        let key = Key::of(&missing).unwrap();
        assert!(key.matches(&missing));
        let mut job = missing.clone();
        job.cwd = "/nonexistent/glen-warm-other".into();
        assert!(!key.matches(&job));
        missing.env_digest = None;
        assert!(Key::of(&missing).is_none());
    }
}
