#![cfg(unix)]
use glen::error::GlenError;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

type Result<T = ()> = std::result::Result<T, GlenError>;
const TARGETS: [&str; 4] = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
];
fn target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => TARGETS[0],
        ("macos", "x86_64") => TARGETS[1],
        ("linux", "x86_64") => TARGETS[2],
        ("linux", "aarch64") => TARGETS[3],
        pair => panic!("unsupported test host: {pair:?}"),
    }
}
fn executable(path: &Path, content: &str) -> Result {
    fs::write(path, content)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}
fn check(output: &Output, code: i32) {
    assert_eq!(output.status.code(), Some(code), "{output:?}");
}
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

struct Installer {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    shim: PathBuf,
    rel: PathBuf,
    binary: PathBuf,
}
impl Installer {
    fn new() -> Result<Self> {
        let tmp = tempfile::tempdir()?;
        let root = fs::canonicalize(tmp.path())?;
        let home = root.join("home");
        let shim = root.join("shim");
        let rel = root.join("rel");
        for path in [&home, &shim, &rel] {
            fs::create_dir(path)?;
        }
        let binary = root.join("fake-glen");
        executable(&binary, "#!/bin/sh\necho \"glen 9.9.9\"\n")?;
        Ok(Self {
            _tmp: tmp,
            root,
            home,
            shim,
            rel,
            binary,
        })
    }
    fn command(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin:/usr/sbin", self.shim.display()),
            )
            .env("HOME", &self.home)
            .env(
                "GLEN_INSTALL_BASE_URL",
                format!("file://{}", self.rel.display()),
            )
            .current_dir(env!("CARGO_MANIFEST_DIR"));
        c
    }
    fn install(&self, args: &[&str]) -> Result<Output> {
        Ok(self.command("sh").arg("install.sh").args(args).output()?)
    }
    fn package(&self, out: &Path) -> Result<Output> {
        Ok(self
            .command("sh")
            .arg("scripts/package-release.sh")
            .arg(target())
            .arg(&self.binary)
            .arg(out)
            .output()?)
    }
    fn release(&self) -> Result {
        let latest = self.rel.join("latest/download");
        let version = self.rel.join("download/v9.9.9");
        check(&self.package(&latest)?, 0);
        fs::create_dir_all(&version)?;
        for entry in fs::read_dir(latest)? {
            let entry = entry?;
            fs::copy(entry.path(), version.join(entry.file_name()))?;
        }
        Ok(())
    }
    fn platform(&self, os: &str, arch: &str, translated: &str) -> Result {
        executable(
            &self.shim.join("uname"),
            &format!(
                "#!/bin/sh\ncase \"$1\" in -s) echo '{os}';; -m) echo '{arch}';; *) exit 1;; esac\n"
            ),
        )?;
        executable(
            &self.shim.join("sysctl"),
            &format!("#!/bin/sh\necho '{translated}'\n"),
        )
    }
    fn asset(&self) -> PathBuf {
        self.rel
            .join("latest/download")
            .join(format!("glen-{}.tar.gz", target()))
    }
    fn sentinel(&self) -> Result<PathBuf> {
        let bin = self.home.join(".local/bin/glen");
        fs::create_dir_all(self.home.join(".local/bin"))?;
        fs::write(&bin, b"existing sentinel\0binary")?;
        Ok(bin)
    }
    fn installed(&self, bin: &Path, output: &Output) -> Result {
        check(output, 0);
        assert_eq!(fs::metadata(bin)?.permissions().mode() & 0o777, 0o755);
        let version = self
            .command(bin.to_string_lossy().as_ref())
            .arg("--version")
            .output()?;
        check(&version, 0);
        assert_eq!(stdout(&version), "glen 9.9.9\n");
        assert!(
            stdout(output).contains(&format!("installed glen 9.9.9 to {}", bin.display())),
            "{output:?}"
        );
        Ok(())
    }
}

#[test]
fn native_latest_install() -> Result {
    let b = Installer::new()?;
    b.release()?;
    b.installed(&b.home.join(".local/bin/glen"), &b.install(&[])?)
}

#[test]
fn versioned_install_and_invalid_versions() -> Result {
    let b = Installer::new()?;
    b.release()?;
    fs::remove_dir_all(b.rel.join("latest"))?;
    for version in ["9.9.9", "v9.9.9"] {
        b.installed(
            &b.home.join(".local/bin/glen"),
            &b.install(&["--version", version])?,
        )?;
    }
    for version in ["9.9", "1.0.0;rm", "v", "1.0.0/path", "1.0.0\n9.9.9"] {
        let o = b.install(&["--version", version])?;
        check(&o, 2);
        assert!(stderr(&o).contains("Usage:"));
        assert!(!stderr(&o).contains("downloading"));
    }
    let o = b.install(&["--version=9.9.9"])?;
    b.installed(&b.home.join(".local/bin/glen"), &o)
}

#[test]
fn prefix_creates_bin() -> Result {
    let b = Installer::new()?;
    b.release()?;
    let prefix = b.root.join("p");
    let o = b
        .command("sh")
        .args(["install.sh", "--prefix"])
        .arg(&prefix)
        .output()?;
    b.installed(&prefix.join("bin/glen"), &o)
}

#[test]
fn unsupported_platforms_write_nothing() -> Result {
    let b = Installer::new()?;
    // Prove no attempt to download or allocate a staging directory occurs.
    for tool in ["curl", "mktemp", "mkdir"] {
        executable(
            &b.shim.join(tool),
            "#!/bin/sh\necho unexpected tool invocation >&2\nexit 99\n",
        )?;
    }
    for (os, arch) in [
        ("FreeBSD", "amd64"),
        ("Linux", "armv7l"),
        ("Linux", "i686"),
        ("MINGW64_NT", "x86_64"),
    ] {
        b.platform(os, arch, "0")?;
        let o = b.install(&[])?;
        check(&o, 1);
        let err = stderr(&o);
        assert!(err.contains(&format!("{os} {arch}")), "{err}");
        for target in TARGETS {
            assert!(err.contains(target), "{err}");
        }
        assert!(!err.contains("unexpected tool"));
        assert!(!b.home.join(".local").exists());
    }
    Ok(())
}

#[test]
fn checksum_mismatch_preserves_existing_binary() -> Result {
    let b = Installer::new()?;
    b.release()?;
    let bin = b.sentinel()?;
    let before = fs::read(&bin)?;
    fs::write(
        b.asset().with_extension("gz.sha256"),
        format!("{}  asset\n", "0".repeat(64)),
    )?;
    let o = b.install(&[])?;
    check(&o, 1);
    assert!(stderr(&o).contains("checksum mismatch"), "{o:?}");
    assert_eq!(fs::read(bin)?, before);
    Ok(())
}

#[test]
fn unexpected_archive_entry_preserves_existing_binary() -> Result {
    let b = Installer::new()?;
    b.release()?;
    let bin = b.sentinel()?;
    let before = fs::read(&bin)?;
    let stage = b.root.join("stage");
    let name = format!("glen-{}", target());
    fs::create_dir_all(stage.join(&name))?;
    fs::copy(&b.binary, stage.join(&name).join("glen"))?;
    fs::write(stage.join(&name).join("LICENSE"), "synthetic license")?;
    fs::write(stage.join(&name).join("extra"), "unexpected")?;
    check(
        &b.command("tar")
            .env("COPYFILE_DISABLE", "1")
            .arg("-czf")
            .arg(b.asset())
            .arg("-C")
            .arg(&stage)
            .arg(&name)
            .output()?,
        0,
    );
    let hash = b.command("sh").args(["-c", "if command -v sha256sum >/dev/null; then sha256sum \"$1\"; elif command -v shasum >/dev/null; then shasum -a 256 \"$1\"; else openssl dgst -sha256 -r \"$1\"; fi", "hash"]).arg(b.asset()).output()?;
    check(&hash, 0);
    fs::write(b.asset().with_extension("gz.sha256"), &hash.stdout)?;
    let o = b.install(&[])?;
    check(&o, 1);
    assert!(stderr(&o).contains("unexpected archive entry"), "{o:?}");
    assert_eq!(fs::read(bin)?, before);
    assert!(!b.home.join(".local/bin/extra").exists());
    assert!(!b.root.join("evil").exists());
    Ok(())
}

#[test]
fn paths_with_spaces() -> Result {
    let mut b = Installer::new()?;
    b.release()?;
    let prefix = b.root.join("my prefix");
    let o = b
        .command("sh")
        .args(["install.sh", "--prefix"])
        .arg(&prefix)
        .output()?;
    b.installed(&prefix.join("bin/glen"), &o)?;
    b.home = b.root.join("my home");
    fs::create_dir(&b.home)?;
    b.installed(&b.home.join(".local/bin/glen"), &b.install(&[])?)
}

#[test]
fn missing_archive_and_checksum_name_url_and_source_fallback() -> Result {
    let b = Installer::new()?;
    for checksum_missing in [false, true] {
        if checksum_missing {
            b.release()?;
            fs::remove_file(b.asset().with_extension("gz.sha256"))?;
        }
        let o = b.install(&[])?;
        check(&o, 1);
        let url = format!(
            "file://{}{}",
            b.asset().display(),
            if checksum_missing { ".sha256" } else { "" }
        );
        assert!(stderr(&o).contains(&url), "{o:?}");
        assert!(
            stderr(&o)
                .contains("cargo install --git https://github.com/nodera-studio/glen --locked"),
            "{o:?}"
        );
        assert!(!b.home.join(".local").exists());
    }
    Ok(())
}

#[test]
fn dry_run_is_a_write_free_plan() -> Result {
    let b = Installer::new()?;
    b.platform("Linux", "aarch64", "0")?;
    for tool in ["curl", "mktemp", "mkdir"] {
        executable(
            &b.shim.join(tool),
            "#!/bin/sh\necho unexpected tool invocation >&2\nexit 99\n",
        )?;
    }
    let o = b.install(&["--dry-run"])?;
    check(&o, 0);
    assert!(o.stderr.is_empty());
    let archive = format!(
        "file://{}/latest/download/glen-aarch64-unknown-linux-musl.tar.gz",
        b.rel.display()
    );
    assert_eq!(
        stdout(&o),
        format!(
            "target: aarch64-unknown-linux-musl\narchive: {archive}\nchecksum: {archive}.sha256\ninstall: {}/.local/bin/glen\n",
            b.home.display()
        )
    );
    assert_eq!(fs::read_dir(&b.home)?.count(), 0);
    assert_eq!(fs::read_dir(&b.rel)?.count(), 0);
    Ok(())
}

#[test]
fn rosetta_selects_native_apple_silicon() -> Result {
    let b = Installer::new()?;
    for (translated, target) in [("1", TARGETS[0]), ("0", TARGETS[1])] {
        b.platform("Darwin", "x86_64", translated)?;
        let o = b.install(&["--dry-run"])?;
        check(&o, 0);
        assert!(
            stdout(&o).starts_with(&format!("target: {target}\n")),
            "{o:?}"
        );
    }
    Ok(())
}

#[test]
fn unwritable_directory_suggests_prefix() -> Result {
    let b = Installer::new()?;
    b.release()?;
    let prefix = b.root.join("ro");
    let dir = prefix.join("bin");
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555))?;
    if fs::write(dir.join("probe"), "probe").is_ok() {
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755))?;
        return Ok(());
    }
    let o = b
        .command("sh")
        .args(["install.sh", "--prefix"])
        .arg(&prefix)
        .output();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755))?;
    let o = o?;
    check(&o, 1);
    assert!(stderr(&o).contains(&dir.display().to_string()), "{o:?}");
    assert!(stderr(&o).contains("--prefix"), "{o:?}");
    Ok(())
}

#[test]
fn architecture_aliases() -> Result {
    let b = Installer::new()?;
    for (os, arch, target) in [
        ("Linux", "amd64", TARGETS[2]),
        ("Darwin", "arm64", TARGETS[0]),
        ("Linux", "arm64", TARGETS[3]),
    ] {
        b.platform(os, arch, "0")?;
        let o = b.install(&["--dry-run"])?;
        check(&o, 0);
        assert!(
            stdout(&o).starts_with(&format!("target: {target}\n")),
            "{o:?}"
        );
    }
    Ok(())
}

#[test]
fn package_layout_checksum_and_executable_mode() -> Result {
    let b = Installer::new()?;
    b.release()?;
    let o = b.command("tar").arg("-tzf").arg(b.asset()).output()?;
    check(&o, 0);
    let name = format!("glen-{}", target());
    let mut entries = stdout(&o).lines().map(str::to_owned).collect::<Vec<_>>();
    entries.sort();
    let mut expected = [
        format!("{name}/"),
        format!("{name}/glen"),
        format!("{name}/LICENSE"),
    ];
    expected.sort();
    assert_eq!(entries, expected);
    let sum = fs::read_to_string(b.asset().with_extension("gz.sha256"))?;
    assert!(sum.len() >= 64);
    assert!(
        sum[..64]
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    );
    assert_eq!(&sum[64..], format!("  {name}.tar.gz\n"));
    // Installer verifies the checksum and archive mode, coupling both scripts end to end.
    b.installed(&b.home.join(".local/bin/glen"), &b.install(&[])?)
}

#[test]
fn argument_errors_help_and_home_override() -> Result {
    let b = Installer::new()?;
    for args in [
        vec!["--unknown"],
        vec!["--version"],
        vec!["--prefix"],
        vec!["--prefix="],
        vec!["--version="],
        vec!["--version", "--dry-run"],
    ] {
        let o = b.install(&args)?;
        check(&o, 2);
        assert!(stderr(&o).contains("Usage:"), "{o:?}");
    }
    for args in [
        vec![],
        vec!["bad/target", "missing", "out"],
        vec![target(), "missing", "out"],
        vec![target(), "a", "b", "c"],
    ] {
        let o = b
            .command("sh")
            .arg("scripts/package-release.sh")
            .args(&args)
            .output()?;
        check(
            &o,
            if args.len() == 3 && args[0] == target() {
                1
            } else {
                2
            },
        );
    }
    let o = b.install(&["--help"])?;
    check(&o, 0);
    for word in [
        "--version",
        "--prefix",
        "--dry-run",
        "GLEN_INSTALL_BASE_URL",
        "Uninstall:",
    ] {
        assert!(stdout(&o).contains(word));
    }
    let o = b
        .command("sh")
        .args(["install.sh", "--dry-run"])
        .env_remove("HOME")
        .output()?;
    check(&o, 1);
    assert!(stderr(&o).contains("HOME"));
    let prefix = b.root.join("p");
    let o = b
        .command("sh")
        .args(["install.sh", "--dry-run"])
        .arg(format!("--prefix={}", prefix.display()))
        .env_remove("HOME")
        .output()?;
    check(&o, 0);
    assert!(!prefix.exists());
    Ok(())
}
