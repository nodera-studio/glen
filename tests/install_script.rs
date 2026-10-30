#![cfg(unix)]
use glen::error::GlenError;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
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

// Write exact ustar headers so malicious paths and link types survive both GNU
// and BSD tar's creation-time sanitization. No new test dependency is needed.
fn tar_entry(archive: &mut Vec<u8>, name: &str, kind: u8, link: &str, data: &[u8]) {
    assert!(name.len() <= 100 && link.len() <= 100);
    let mut header = [0; 512];
    header[..name.len()].copy_from_slice(name.as_bytes());
    header[100..108].copy_from_slice(b"0000755\0");
    header[108..116].copy_from_slice(b"0000000\0");
    header[116..124].copy_from_slice(b"0000000\0");
    header[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
    header[136..148].copy_from_slice(b"00000000000\0");
    header[148..156].fill(b' ');
    header[156] = kind;
    header[157..157 + link.len()].copy_from_slice(link.as_bytes());
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum: usize = header.iter().map(|b| usize::from(*b)).sum();
    header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
    archive.extend_from_slice(&header);
    archive.extend_from_slice(data);
    archive.resize(archive.len().next_multiple_of(512), 0);
}

struct Installer {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    shim: PathBuf,
    rel: PathBuf,
    binary: PathBuf,
    scratch: PathBuf,
}
impl Installer {
    fn new() -> Result<Self> {
        let tmp = tempfile::tempdir()?;
        let root = fs::canonicalize(tmp.path())?;
        let home = root.join("home");
        let shim = root.join("shim");
        let rel = root.join("rel");
        let scratch = root.join("scratch");
        for path in [&home, &shim, &rel, &scratch] {
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
            scratch,
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
            .env("TMPDIR", &self.scratch)
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
    fn checksum(&self) -> Result {
        let hash = self.command("sh").args(["-c", "if command -v sha256sum >/dev/null; then sha256sum \"$1\"; elif command -v shasum >/dev/null; then shasum -a 256 \"$1\"; else openssl dgst -sha256 -r \"$1\"; fi", "hash"]).arg(self.asset()).output()?;
        check(&hash, 0);
        fs::write(self.asset().with_extension("gz.sha256"), &hash.stdout)?;
        Ok(())
    }
    fn attack_archive(&self, entry: &str, kind: u8, link: &str) -> Result {
        let name = format!("glen-{}", target());
        let mut archive = Vec::new();
        tar_entry(&mut archive, &format!("{name}/"), b'5', "", b"");
        tar_entry(
            &mut archive,
            &format!("{name}/glen"),
            b'0',
            "",
            &fs::read(&self.binary)?,
        );
        tar_entry(
            &mut archive,
            entry,
            kind,
            link,
            if kind == b'0' { b"unexpected" } else { b"" },
        );
        archive.extend_from_slice(&[0; 1024]);
        let raw = self.root.join("attack.tar");
        fs::write(&raw, archive)?;
        let gzip = self.command("gzip").arg("-c").arg(raw).output()?;
        check(&gzip, 0);
        fs::write(self.asset(), &gzip.stdout)?;
        self.checksum()
    }
    fn cleaned(&self) -> Result {
        assert_eq!(fs::read_dir(&self.scratch)?.count(), 0);
        let bin = self.home.join(".local/bin");
        if bin.is_dir() {
            for entry in fs::read_dir(bin)? {
                assert!(
                    !entry?
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".glen.tmp.")
                );
            }
        }
        Ok(())
    }
    fn sentinel(&self) -> Result<PathBuf> {
        let bin = self.home.join(".local/bin/glen");
        fs::create_dir_all(self.home.join(".local/bin"))?;
        fs::write(&bin, b"existing sentinel\0binary")?;
        Ok(bin)
    }
    fn installed(&self, bin: &Path, output: &Output) -> Result {
        check(output, 0);
        assert!(fs::symlink_metadata(bin)?.file_type().is_file());
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
    // Extraction is under root/scratch/tmp.XXXX; ../../evil targets root/evil.
    b.attack_archive("../../evil", b'0', "")?;
    let o = b.install(&[])?;
    check(&o, 1);
    assert!(stderr(&o).contains("unexpected archive entry"), "{o:?}");
    assert_eq!(fs::read(bin)?, before);
    assert!(!b.home.join(".local/bin/extra").exists());
    assert!(!b.root.join("evil").exists());
    b.cleaned()
}

#[test]
fn absolute_archive_path_is_rejected_and_cleaned() -> Result {
    let b = Installer::new()?;
    b.release()?;
    let bin = b.sentinel()?;
    let before = fs::read(&bin)?;
    b.attack_archive(&b.root.join("evil").to_string_lossy(), b'0', "")?;
    let o = b.install(&[])?;
    check(&o, 1);
    assert!(stderr(&o).contains("unexpected archive entry"), "{o:?}");
    assert_eq!(fs::read(bin)?, before);
    assert!(!b.root.join("evil").exists());
    b.cleaned()
}

#[test]
fn archive_links_and_special_files_are_rejected_and_cleaned() -> Result {
    for kind in *b"216" {
        let b = Installer::new()?;
        b.release()?;
        let bin = b.sentinel()?;
        let before = fs::read(&bin)?;
        let name = format!("glen-{}", target());
        // LICENSE is allow-listed, and a hardlink can resolve to the earlier
        // regular glen entry: the type check itself must reject the attack.
        b.attack_archive(&format!("{name}/LICENSE"), kind, &format!("{name}/glen"))?;
        let o = b.install(&[])?;
        check(&o, 1);
        assert!(
            stderr(&o).contains("unexpected archive entry type"),
            "{o:?}"
        );
        assert_eq!(fs::read(bin)?, before);
        b.cleaned()?;
    }
    Ok(())
}

#[test]
fn destination_directory_preserves_contents() -> Result {
    let b = Installer::new()?;
    b.release()?;
    let destination = b.home.join(".local/bin/glen");
    fs::create_dir_all(&destination)?;
    let old = destination.join("glen");
    fs::write(&old, "existing binary")?;
    let o = b.install(&[])?;
    check(&o, 1);
    assert!(
        stderr(&o).contains("destination is a symlink or directory"),
        "{o:?}"
    );
    assert!(destination.is_dir());
    assert_eq!(fs::read_to_string(old)?, "existing binary");
    assert_eq!(fs::read_dir(destination)?.count(), 1);
    b.cleaned()
}

#[test]
fn destination_symlinks_preserve_targets() -> Result {
    for directory in [true, false] {
        let b = Installer::new()?;
        b.release()?;
        let destination = b.home.join(".local/bin/glen");
        fs::create_dir_all(b.home.join(".local/bin"))?;
        let target = b.root.join("original");
        let old = if directory {
            fs::create_dir(&target)?;
            target.join("glen")
        } else {
            target.clone()
        };
        fs::write(&old, "existing binary")?;
        symlink(&target, &destination)?;
        let o = b.install(&[])?;
        check(&o, 1);
        assert!(
            stderr(&o).contains("destination is a symlink or directory"),
            "{o:?}"
        );
        assert_eq!(fs::read_link(destination)?, target);
        assert_eq!(fs::read_to_string(old)?, "existing binary");
        b.cleaned()?;
    }
    Ok(())
}

#[test]
fn staged_version_failure_preserves_existing_binary_and_cleans_up() -> Result {
    for existing in [true, false] {
        let b = Installer::new()?;
        executable(&b.binary, "#!/bin/sh\nexit 42\n")?;
        b.release()?;
        let bin = b.home.join(".local/bin/glen");
        if existing {
            b.sentinel()?;
        }
        let o = b.install(&[])?;
        check(&o, 1);
        assert!(stderr(&o).contains("staged glen --version failed"), "{o:?}");
        assert!(!stdout(&o).contains("installed"));
        if existing {
            assert_eq!(fs::read(bin)?, b"existing sentinel\0binary");
        } else {
            assert!(!bin.exists());
        }
        b.cleaned()?;
    }
    Ok(())
}

#[test]
fn downloads_have_transfer_and_connection_timeouts_and_retries() -> Result {
    let b = Installer::new()?;
    b.release()?;
    executable(
        &b.shim.join("curl"),
        "#!/bin/sh\nset -eu\nargs=\" $* \"\nfor option in '--max-time 300' '--connect-timeout 15' '--retry 3'; do\ncase \"$args\" in *\" $option \"*) ;; *) echo \"missing $option\" >&2; exit 99;; esac\ndone\nexec /usr/bin/curl \"$@\"\n",
    )?;
    b.installed(&b.home.join(".local/bin/glen"), &b.install(&[])?)
}

#[test]
fn staging_file_is_reserved_before_copying() -> Result {
    let b = Installer::new()?;
    b.release()?;
    let victim = b.root.join("victim");
    fs::write(&victim, "do not overwrite")?;
    // An unreserved staging name lets a competing writer plant a symlink.
    executable(
        &b.shim.join("cp"),
        "#!/bin/sh\nset -eu\nif [ ! -e \"$2\" ]; then ln -s \"$VICTIM\" \"$2\"; fi\nexec /bin/cp \"$@\"\n",
    )?;
    let o = b
        .command("sh")
        .arg("install.sh")
        .env("VICTIM", &victim)
        .output()?;
    b.installed(&b.home.join(".local/bin/glen"), &o)?;
    assert_eq!(fs::read_to_string(victim)?, "do not overwrite");
    b.cleaned()
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
