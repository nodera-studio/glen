use crate::{env::Env, omp::paths::absolute};
use std::{fs, path::Path};
pub fn bucket_for(cwd: &Path, env: &Env) -> String {
    let resolved = absolute(env, &cwd.to_string_lossy());
    let canonical = fs::canonicalize(&resolved).unwrap_or(resolved);
    let temp = env
        .non_empty("TMPDIR")
        .or_else(|| env.non_empty("TMP"))
        .or_else(|| env.non_empty("TEMP"))
        .unwrap_or("/tmp");
    let temp = absolute(env, temp);
    let temp = fs::canonicalize(&temp).unwrap_or(temp);
    if let Ok(rel) = canonical.strip_prefix(temp)
        && within(rel)
    {
        return encode_relative("-tmp", rel);
    }
    if let Some(home) = env.home() {
        let home = fs::canonicalize(home).unwrap_or_else(|_| home.into());
        if let Ok(rel) = canonical.strip_prefix(home)
            && within(rel)
        {
            return encode_relative("-", rel);
        }
    }
    format!(
        "--{}--",
        encode(canonical.to_string_lossy().trim_start_matches('/'))
    )
}
fn within(relative: &Path) -> bool {
    !relative
        .components()
        .next()
        .is_some_and(|c| c.as_os_str().to_string_lossy().starts_with(".."))
}
fn encode(s: &str) -> String {
    s.replace(['/', '\\', ':'], "-")
}
fn encode_relative(prefix: &str, rel: &Path) -> String {
    let rel = encode(&rel.to_string_lossy());
    if rel.is_empty() {
        prefix.into()
    } else if prefix.ends_with('-') {
        format!("{prefix}{rel}")
    } else {
        format!("{prefix}-{rel}")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn buckets() {
        let t = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(t.path()).unwrap();
        let h = root.join("home");
        let tmp = root.join("tmp");
        std::fs::create_dir_all(h.join("work/alpha")).unwrap();
        std::fs::create_dir_all(tmp.join("x")).unwrap();
        let e = Env::for_test(Some(&h), &h, &[("TMPDIR", tmp.to_str().unwrap())]);
        assert_eq!(bucket_for(&h.join("work/alpha"), &e), "-work-alpha");
        assert_eq!(bucket_for(&h, &e), "-");
        assert_eq!(bucket_for(&tmp.join("x"), &e), "-tmp-x");
        assert_eq!(bucket_for(&h.join("missing"), &e), "-missing");
        assert_eq!(
            bucket_for(Path::new("/outside/path"), &e),
            "--outside-path--"
        );
        let e = Env::for_test(Some(t.path()), &h, &[("TMPDIR", tmp.to_str().unwrap())]);
        assert_eq!(bucket_for(&tmp.join("x"), &e), "-tmp-x");
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    #[test]
    fn dotdot_prefixed_components_are_outside_home_and_temp() {
        let tree = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tree.path()).unwrap();
        let home = root.join("home");
        let temp = root.join("tmp");
        fs::create_dir_all(home.join("..cfg/x")).unwrap();
        fs::create_dir_all(temp.join("..cache/x")).unwrap();
        let env = Env::for_test(Some(&home), &home, &[("TMPDIR", temp.to_str().unwrap())]);
        for p in [home.join("..cfg/x"), temp.join("..cache/x")] {
            assert_eq!(
                bucket_for(&p, &env),
                format!(
                    "--{}--",
                    encode(p.to_str().unwrap().trim_start_matches('/'))
                )
            );
        }
        assert_eq!(
            bucket_for(&home.join("normal/..cfg"), &env),
            "-normal-..cfg"
        );
        assert_eq!(bucket_for(&home, &env), "-");
    }
}
