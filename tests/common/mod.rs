use std::{
    path::PathBuf,
    process::{Command, Output},
};
pub fn glen(args: &[&str], vars: &[(&str, &str)]) -> Output {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let path = tmp.path().join("bin");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&path).unwrap();
    Command::new(env!("CARGO_BIN_EXE_glen"))
        .args(args)
        .env_clear()
        .env("HOME", &home)
        .env("GLEN_HOME", tmp.path().join("glen"))
        .env("PATH", path)
        .envs(vars.iter().copied())
        .output()
        .unwrap()
}
pub fn fixture(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(path)
}
pub fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "invalid JSON: {e}; stdout {:?}, stderr {:?}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}
