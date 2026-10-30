#![allow(dead_code)]
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

pub struct OmpBox {
    pub tmp: tempfile::TempDir,
    pub home: PathBuf,
    pub agent: PathBuf,
    pub tmpdir: PathBuf,
    pub glen_home: PathBuf,
}
impl OmpBox {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let home = root.join("home");
        let agent = root.join("agent");
        let tmpdir = root.join("tmpdir");
        let glen_home = root.join("glen");
        for p in [
            home.join("work/alpha"),
            home.join("work/beta"),
            agent.clone(),
            tmpdir.clone(),
        ] {
            std::fs::create_dir_all(p).unwrap();
        }
        copy_tree(&fixture("omp/agent"), &agent);
        let sb = Self {
            tmp,
            home,
            agent,
            tmpdir,
            glen_home,
        };
        let now = std::time::SystemTime::now();
        for (k, seconds) in [
            ('G', 60),
            ('E', 120),
            ('D', 180),
            ('C', 240),
            ('B', 300),
            ('A', 360),
            ('F', 30 * 86400),
        ] {
            std::fs::File::open(sb.session(k))
                .unwrap()
                .set_modified(now - std::time::Duration::from_secs(seconds))
                .unwrap();
        }
        sb
    }
    pub fn alpha(&self) -> PathBuf {
        self.home.join("work/alpha")
    }
    pub fn beta(&self) -> PathBuf {
        self.home.join("work/beta")
    }
    pub fn session(&self, key: char) -> PathBuf {
        let tail = match key {
            'A' => "e27c0de",
            'B' => "f13a5b6",
            'C' => "b7e2d81",
            'D' => "c4f9a72",
            'E' => "0a3f417",
            'F' => "a81c0de",
            'G' => "b8e4a07",
            _ => panic!("unknown fixture"),
        };
        for bucket in ["-work-alpha", "-work-beta"] {
            for e in std::fs::read_dir(self.agent.join("sessions").join(bucket)).unwrap() {
                let e = e.unwrap();
                if e.file_name()
                    .to_string_lossy()
                    .ends_with(&format!("{tail}.jsonl"))
                {
                    return e.path();
                }
            }
        }
        panic!("fixture missing")
    }
    pub fn command(&self, args: &[&str], cwd: &std::path::Path) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_glen"));
        c.args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("GLEN_HOME", &self.glen_home)
            .env("PI_CODING_AGENT_DIR", &self.agent)
            .env("TMPDIR", &self.tmpdir)
            .env("TZ", "UTC")
            .env("PATH", "")
            .current_dir(cwd);
        c
    }
    pub fn glen(&self, args: &[&str], cwd: &std::path::Path) -> Output {
        self.command(args, cwd).output().unwrap()
    }
    pub fn clean(&self) {
        assert!(!self.home.join(".omp").exists());
        assert!(!self.glen_home.exists());
    }
}
fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            std::fs::create_dir(&target).unwrap();
            copy_tree(&e.path(), &target)
        } else {
            std::fs::copy(e.path(), target).unwrap();
        }
    }
}
pub fn data(out: &Output) -> serde_json::Value {
    assert!(out.status.success(), "{out:?}");
    assert!(out.stderr.is_empty(), "{out:?}");
    assert!(!out.stdout.contains(&27));
    let v = json(out);
    assert_eq!(v["ok"], true);
    v["data"].clone()
}
