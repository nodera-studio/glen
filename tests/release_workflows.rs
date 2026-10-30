#![cfg(unix)]
use glen::error::GlenError;
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

type Result<T = ()> = std::result::Result<T, GlenError>;
const CI: &str = include_str!("../.github/workflows/ci.yml");
const RELEASE: &str = include_str!("../.github/workflows/release.yml");
const MAIN_PUSH: &str = "github.event_name == 'push' && github.ref == 'refs/heads/main'";

#[test]
fn deploy_checks_main_immediately_before_wrangler() -> Result {
    // Exercise the actual workflow shell block with local peers: no network or
    // Cloudflare credentials, and no second copy of the ordering logic.
    let block = CI
        .split("run: |\n")
        .find(|block| block.contains("git ls-remote"))
        .ok_or_else(|| std::io::Error::other("missing deploy script"))?;
    let script = block
        .lines()
        .map_while(|line| line.strip_prefix("          "))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(script.contains("npx --yes wrangler@4.124.0 deploy"));
    for (remote, git_code, deploys, succeeds) in [
        ("current", 0, true, true),
        ("newer", 0, false, true),
        ("", 128, false, false),
    ] {
        let tmp = tempfile::tempdir()?;
        for (name, body) in [
            (
                "git",
                "#!/bin/sh\n[ \"$*\" = 'ls-remote --exit-code origin refs/heads/main' ] || exit 99\nprintf '%s\\trefs/heads/main\\n' \"$REMOTE_SHA\"\nexit \"$GIT_CODE\"\n",
            ),
            (
                "npx",
                "#!/bin/sh\n[ \"$*\" = '--yes wrangler@4.124.0 deploy' ] || exit 99\nprintf deployed > \"$DEPLOY_MARKER\"\n",
            ),
        ] {
            let path = tmp.path().join(name);
            fs::write(&path, body)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
        }
        let marker = tmp.path().join("deployed");
        let o = Command::new("/bin/bash")
            .args(["-e", "-c", &script])
            .env_clear()
            .env("PATH", tmp.path())
            .env("GITHUB_SHA", "current")
            .env("REMOTE_SHA", remote)
            .env("GIT_CODE", git_code.to_string())
            .env("DEPLOY_MARKER", &marker)
            .output()?;
        assert_eq!(o.status.success(), succeeds, "{o:?}");
        assert_eq!(marker.exists(), deploys, "{o:?}");
        if remote == "newer" {
            assert!(
                String::from_utf8_lossy(&o.stdout)
                    .contains("::notice title=Site deploy skipped::newer commit on main; skipping"),
                "{o:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn deploy_token_is_main_push_only_and_account_is_explicit() -> Result {
    let token = CI
        .split("      - id: token\n")
        .nth(1)
        .ok_or_else(|| std::io::Error::other("missing token presence step"))?;
    assert!(token.starts_with(&format!("        if: {MAIN_PUSH}\n")));
    assert!(CI.contains(&format!(
        "      - if: {MAIN_PUSH} && steps.token.outputs.present == 'true'"
    )));
    assert!(CI.contains("group: site-deploy-${{ github.ref }}\n      cancel-in-progress: true"));
    assert!(!CI.contains("CLOUDFLARE_ACCOUNT_ID"));
    let config = include_str!("../site/wrangler.jsonc")
        .lines()
        .skip_while(|line| line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let config: serde_json::Value = serde_json::from_str(&config).map_err(std::io::Error::other)?;
    assert_eq!(config["account_id"], "859b4a03642ac2a45e7157bfd9024c21");
    Ok(())
}

#[test]
fn release_reuses_main_git_cache_and_manifest_changes_trigger_it() {
    let paths = RELEASE
        .lines()
        .find(|line| line.trim_start().starts_with("paths:"));
    assert!(
        paths.is_some_and(|line| line.contains("'Cargo.toml'") && line.contains("'Cargo.lock'"))
    );
    let key = "key: omp-git-v1-${{ hashFiles('Cargo.lock') }}";
    assert!(CI.contains(key) && RELEASE.contains(key));
    assert!(CI.contains(&format!(
        "      - if: {MAIN_PUSH} && runner.os == 'Linux'\n        uses: actions/cache@v4\n        with:\n          path: ~/.cargo/git\n          {key}"
    )));
    assert!(RELEASE.contains("uses: actions/cache/restore@v4"));
    assert!(RELEASE.contains("path: ~/.cargo/git"));
    assert!(RELEASE.contains("restore-keys: |\n            omp-git-v1-"));
    assert!(!RELEASE.contains("uses: actions/cache/save@"));
    assert!(!RELEASE.contains("uses: Swatinem/rust-cache@"));
    assert!(!RELEASE.contains("path: target"));
}
