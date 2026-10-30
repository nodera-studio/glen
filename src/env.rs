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
        Self {
            vars: std::env::vars_os()
                .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
                .collect(),
            home: dirs::home_dir(),
            home_explicit: std::env::var_os("HOME").is_some_and(|v| !v.is_empty()),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
        }
    }
    pub fn for_test(home: Option<&Path>, cwd: &Path, vars: &[(&str, &str)]) -> Self {
        Self {
            vars: vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            home: home.map(Path::to_path_buf),
            home_explicit: home.is_some(),
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
