use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Env {
    vars: BTreeMap<String, String>,
    home: Option<PathBuf>,
    home_explicit: bool,
    cwd: PathBuf,
}
impl Env {
    pub fn from_process() -> Self {
        let vars: BTreeMap<String, String> = std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
            .collect();
        let home = std::env::var_os("HOME")
            .filter(|v| !v.is_empty() && v.to_str().is_none_or(|s| !s.trim().is_empty()))
            .map(PathBuf::from);
        Self {
            home_explicit: home.is_some(),
            home: home.or_else(passwd_home),
            vars,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
        }
    }
    pub fn for_test(home: Option<&Path>, cwd: &Path, vars: &[(&str, &str)]) -> Self {
        Self {
            vars: vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            home: home
                .filter(|p| !p.as_os_str().to_string_lossy().trim().is_empty())
                .map(Path::to_path_buf),
            home_explicit: home.is_some_and(|p| !p.as_os_str().to_string_lossy().trim().is_empty()),
            cwd: cwd.to_path_buf(),
        }
    }
    pub fn with_cwd(&self, cwd: &Path) -> Self {
        let mut env = self.clone();
        env.cwd = cwd.to_path_buf();
        env
    }
    pub fn get(&self, k: &str) -> Option<&str> {
        self.vars.get(k).map(String::as_str)
    }
    pub fn non_empty(&self, k: &str) -> Option<&str> {
        self.get(k).filter(|v| !v.trim().is_empty())
    }
    pub fn home_is_explicit(&self) -> bool {
        self.home_explicit
    }
    pub fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }
}
#[cfg(unix)]
fn passwd_home() -> Option<PathBuf> {
    use std::{
        ffi::{CStr, OsString},
        os::unix::ffi::OsStringExt,
    };
    let mut size = 16384;
    loop {
        let mut buffer = vec![0u8; size];
        // SAFETY: getpwuid_r receives valid, writable storage; its returned string
        // remains backed by buffer until copied into an owned OsString below.
        let mut passwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result = std::ptr::null_mut();
        let code = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                &mut passwd,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if code == libc::ERANGE && size < 1024 * 1024 {
            size *= 2;
            continue;
        }
        if code != 0 || result.is_null() || passwd.pw_dir.is_null() {
            return None;
        }
        let bytes = unsafe { CStr::from_ptr(passwd.pw_dir) }.to_bytes();
        return (!bytes.is_empty()).then(|| PathBuf::from(OsString::from_vec(bytes.to_vec())));
    }
}
#[cfg(not(unix))]
fn passwd_home() -> Option<PathBuf> {
    dirs::home_dir().filter(|p| !p.as_os_str().to_string_lossy().trim().is_empty())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empties() {
        let e = Env::for_test(
            None,
            Path::new("/w"),
            &[("a", ""), ("b", "  "), ("c", " x ")],
        );
        assert_eq!(e.get("a"), Some(""));
        assert_eq!(e.non_empty("a"), None);
        assert_eq!(e.non_empty("b"), None);
        assert_eq!(e.non_empty("c"), Some(" x "));
    }
}
