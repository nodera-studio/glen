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
    // W8 SEC-6: the locked devDependency, never a registry fetch next to the token.
    assert!(script.contains("npx --no wrangler deploy"));
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
                "#!/bin/sh\n[ \"$*\" = '--no wrangler deploy' ] || exit 99\nprintf deployed > \"$DEPLOY_MARKER\"\n",
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
    // W8 SEC-7: the restore action is pinned to the v4 commit, not the moving tag.
    assert!(
        RELEASE
            .contains("uses: actions/cache/restore@0057852bfaa89a56745cba8c7296529d2fc39830 # v4.")
    );
    assert!(RELEASE.contains("path: ~/.cargo/git"));
    assert!(RELEASE.contains("restore-keys: |\n            omp-git-v1-"));
    assert!(!RELEASE.contains("uses: actions/cache/save@"));
    assert!(!RELEASE.contains("uses: Swatinem/rust-cache@"));
    assert!(!RELEASE.contains("path: target"));
}

const FLAKE: &str = include_str!("../.github/workflows/flake-proof.yml");
const OMP_LATEST: &str = include_str!("../.github/workflows/omp-latest.yml");

/// `uses:` targets of a workflow, with the trailing `# version` comment.
fn uses(workflow: &str) -> Vec<&str> {
    workflow
        .lines()
        .filter_map(|line| {
            line.trim_start()
                .trim_start_matches("- ")
                .strip_prefix("uses: ")
        })
        .collect()
}

/// `owner/repo[/path]@<40 hex> # <version>`.
fn sha_pinned(action: &str) -> bool {
    let Some((_, rest)) = action.split_once('@') else {
        return false;
    };
    let Some((sha, comment)) = rest.split_once(" # ") else {
        return false;
    };
    sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()) && !comment.trim().is_empty()
}

#[test]
fn third_party_actions_are_pinned_to_commit_shas() {
    // W8 SEC-7: every action in the release build, and every non-`actions/*`
    // action anywhere, is a full commit SHA with a version comment.
    let release = uses(RELEASE);
    assert!(release.len() >= 8, "{release:?}");
    for action in release {
        assert!(sha_pinned(action), "release.yml: {action}");
    }
    for (name, workflow) in [("ci.yml", CI), ("flake-proof.yml", FLAKE)] {
        let third_party = uses(workflow)
            .into_iter()
            .filter(|action| !action.starts_with("actions/"))
            .collect::<Vec<_>>();
        assert!(!third_party.is_empty(), "{name}");
        for action in third_party {
            assert!(sha_pinned(action), "{name}: {action}");
        }
    }
}

#[test]
fn toolchain_comes_from_rust_toolchain_toml_and_msrv_is_checked() {
    // W8 DEBT-7.
    let pin = include_str!("../rust-toolchain.toml");
    let channel = pin
        .lines()
        .find_map(|line| line.strip_prefix("channel = \""))
        .and_then(|rest| rest.strip_suffix('"'))
        .expect("rust-toolchain.toml channel");
    assert_eq!(
        channel.split('.').count(),
        3,
        "pin an exact stable: {channel}"
    );
    assert!(channel.split('.').all(|part| part.parse::<u32>().is_ok()));
    assert!(pin.contains("components = [\"clippy\", \"rustfmt\"]"));
    assert!(include_str!("../Cargo.toml").contains("\nrust-version = \"1.89\"\n"));
    let read_pin = "run: echo \"channel=$(sed -n 's/^channel = \"\\(.*\\)\"$/\\1/p' rust-toolchain.toml)\" >> \"$GITHUB_OUTPUT\"";
    for (name, workflow, jobs) in [
        ("ci.yml", CI, 2),
        ("release.yml", RELEASE, 1),
        ("flake-proof.yml", FLAKE, 1),
        ("omp-latest.yml", OMP_LATEST, 1),
    ] {
        assert!(!workflow.contains("rust-toolchain@stable"), "{name}");
        assert_eq!(workflow.matches(read_pin).count(), jobs, "{name}");
        assert_eq!(
            workflow
                .matches("toolchain: ${{ steps.toolchain.outputs.channel }}")
                .count(),
            jobs,
            "{name}"
        );
    }
    let msrv = CI.split("\n  msrv:\n").nth(1).expect("msrv job");
    let msrv = msrv.split("\n\n").next().unwrap_or(msrv);
    assert!(msrv.contains("echo \"RUSTUP_TOOLCHAIN=$version\" >> \"$GITHUB_ENV\""));
    assert!(msrv.contains("toolchain: ${{ steps.msrv.outputs.version }}"));
    assert!(msrv.contains("- run: cargo check --locked --all-targets"));
}

#[test]
fn release_archives_carry_generated_third_party_licenses() {
    // W8 DEBT-3: the build job generates the notices per target with a pinned
    // cargo-about and hands them to the packager; musl targets get musl's notice.
    assert!(RELEASE.contains("cargo install --locked cargo-about --version 0.9.2 --features cli"));
    assert!(RELEASE.contains(
        "cargo about generate --locked --fail -c licenses/about.toml --target \"$t\" -o \"$notices\" licenses/about.hbs"
    ));
    assert!(RELEASE.contains("*-linux-musl) cat licenses/musl.txt >> \"$notices\" ;;"));
    assert!(RELEASE.contains("sh scripts/package-release.sh \"$t\" \"$bin\" \"$notices\" dist"));
    assert!(RELEASE.contains("'rust-toolchain.toml', 'licenses/**']"));
    // The template's MPL-2.0 source line names the option-ext the lockfile builds.
    let lock = include_str!("../Cargo.lock");
    let option_ext = lock
        .split("[[package]]\nname = \"option-ext\"\nversion = \"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("option-ext in Cargo.lock");
    let line = format!("https://crates.io/crates/option-ext/{option_ext}");
    assert!(
        include_str!("../licenses/about.hbs").contains(&line),
        "{line}"
    );
    assert!(RELEASE.contains(&format!("option-ext {option_ext}: {line}$")));
    assert!(
        include_str!("../licenses/musl.txt").contains("Copyright © 2005-2020 Rich Felker, et al.")
    );
}

#[test]
fn site_deploy_uses_the_locked_wrangler() -> Result {
    // W8 SEC-6: wrangler is an exact devDependency with an integrity hash, and
    // `npm ci` installs it in a step that never sees the Cloudflare token.
    let package: serde_json::Value = serde_json::from_str(include_str!("../site/package.json"))
        .map_err(std::io::Error::other)?;
    assert_eq!(package["devDependencies"]["wrangler"], "4.124.0");
    for script in ["deploy", "deploy:dry"] {
        let script = package["scripts"][script].as_str().unwrap_or_default();
        assert!(script.contains("npx --no wrangler deploy"), "{script}");
    }
    let lock: serde_json::Value = serde_json::from_str(include_str!("../site/package-lock.json"))
        .map_err(std::io::Error::other)?;
    let wrangler = &lock["packages"]["node_modules/wrangler"];
    assert_eq!(wrangler["version"], "4.124.0");
    assert!(
        wrangler["integrity"]
            .as_str()
            .is_some_and(|i| i.starts_with("sha512-"))
    );
    for text in [CI, include_str!("../site/package.json")] {
        assert!(!text.contains("wrangler@") && !text.contains("npx --yes"));
    }
    let deploy = CI
        .split("\n  site-deploy:\n")
        .nth(1)
        .expect("site-deploy job");
    let npm_ci = deploy.find("      - run: npm ci\n").expect("npm ci step");
    assert!(npm_ci < deploy.find("--dry-run").expect("dry run"));
    assert!(npm_ci < deploy.find("CLOUDFLARE_API_TOKEN").expect("token"));
    Ok(())
}

#[test]
fn omp_latest_pins_every_action_to_a_commit_sha() {
    // AC-v2-106: `actions/*` included, unlike the ci.yml rule above.
    let actions = uses(OMP_LATEST);
    assert!(actions.len() >= 3, "{actions:?}");
    for action in actions {
        assert!(sha_pinned(action), "omp-latest.yml: {action}");
    }
}

#[test]
fn omp_latest_triggers_permissions_and_isolation() {
    // AC-v2-106: a schedule at an off-the-hour minute, dispatch, and PRs touching only itself.
    assert!(OMP_LATEST.contains(
        "\non:\n  schedule:\n    - cron: '41 4 * * *'\n  workflow_dispatch:\n  pull_request:\n    paths: ['.github/workflows/omp-latest.yml']\n\npermissions:\n  contents: read\n"
    ));
    let cron = OMP_LATEST
        .lines()
        .find_map(|l| l.trim().strip_prefix("- cron: '"))
        .expect("cron line");
    assert_ne!(cron.split_whitespace().next(), Some("0"), "{cron}");
    for trigger in ["push:", "pull_request_target:", "workflow_run:"] {
        assert!(!OMP_LATEST.contains(trigger), "{trigger}");
    }
    assert!(!OMP_LATEST.contains("secrets."));
    assert!(OMP_LATEST.contains("persist-credentials: false"));
    assert!(OMP_LATEST.contains("https://omp.sh/install"));
    assert!(OMP_LATEST.contains("scratch=\"$(mktemp -d)\""));
    // Every command that runs omp (installer, --version, doctor --probe) clears the
    // environment and uses the scratch HOME; the surface test sets omp's HOME itself.
    for needle in ["omp.sh/install", "--version |", "doctor --probe"] {
        let line = OMP_LATEST
            .lines()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("no line runs {needle}"));
        assert!(
            line.contains("env -i HOME=\"$scratch\"") || line.contains("env -i HOME=\"$OMP_HOME\""),
            "{line}"
        );
    }
    for step in ["surface", "facts", "doctor"] {
        assert!(
            OMP_LATEST.contains(&format!(
                "      - id: {step}\n        if: ${{{{ !cancelled() && steps.omp.outcome == 'success' }}}}\n"
            )),
            "{step}"
        );
    }
}

#[test]
fn release_and_ci_never_read_omp_latest() {
    // AC-v2-106: the nightly never gates a PR check or the release preflight.
    for (name, workflow) in [("release.yml", RELEASE), ("ci.yml", CI)] {
        assert!(!workflow.contains("omp-latest"), "{name}");
    }
}

#[test]
fn omp_latest_summary_names_version_outcomes_and_bump_candidate() -> Result {
    // Execute the real summary block, as GitHub's bash does, from the repo root.
    let block = OMP_LATEST
        .split("run: |\n")
        .find(|block| block.contains("GITHUB_STEP_SUMMARY"))
        .ok_or_else(|| std::io::Error::other("missing summary block"))?;
    let script = block
        .lines()
        .map_while(|line| line.strip_prefix("          "))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(script.contains("Not covered"), "{script}");
    let untested = r#"{"ok":true,"warnings":[{"code":"omp_version_untested","message":"m"}]}"#;
    let in_range = r#"{"ok":true}"#;
    let ok = ["success"; 4];
    for (version, outcomes, doctor, want, absent) in [
        (
            "18.15.0",
            ok,
            Some(untested),
            vec![
                "omp version: 18.15.0",
                "bump candidate: omp 18.15.0",
                "max_tested 18.14.0",
            ],
            vec!["FAILED", "no bump needed"],
        ),
        (
            "18.14.0",
            ok,
            Some(in_range),
            vec!["omp version: 18.14.0", "in range, no bump needed"],
            vec!["FAILED", "bump candidate"],
        ),
        (
            "18.15.0",
            ["success", "failure", "success", "success"],
            Some(untested),
            vec!["- surface: failure", "FAILED: surface (failure)"],
            vec!["bump candidate", "no bump needed"],
        ),
        (
            "",
            ["failure", "skipped", "skipped", "skipped"],
            None,
            vec![
                "omp not installed",
                "FAILED: install (failure) surface (skipped)",
            ],
            vec!["bump candidate", "omp version:"],
        ),
    ] {
        let tmp = tempfile::tempdir()?;
        let summary = tmp.path().join("summary.md");
        let doctor_json = tmp.path().join("doctor.json");
        if let Some(text) = doctor {
            fs::write(&doctor_json, text)?;
        }
        let o = Command::new("/bin/bash")
            .args([
                "--noprofile",
                "--norc",
                "-e",
                "-o",
                "pipefail",
                "-c",
                &script,
            ])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("GITHUB_EVENT_NAME", "schedule")
            .env("GITHUB_STEP_SUMMARY", &summary)
            .env("DOCTOR_JSON", &doctor_json)
            .env("OMP_VERSION", version)
            .env("OUTCOME_OMP", outcomes[0])
            .env("OUTCOME_SURFACE", outcomes[1])
            .env("OUTCOME_FACTS", outcomes[2])
            .env("OUTCOME_DOCTOR", outcomes[3])
            .output()?;
        assert!(o.status.success(), "{o:?}");
        let text = fs::read_to_string(&summary)?;
        assert!(
            text.contains(
                "Not covered: model turns, and lease and storage behaviour under a real turn."
            ),
            "{text}"
        );
        for line in want {
            assert!(text.contains(line), "{line:?} missing:\n{text}");
        }
        for line in absent {
            assert!(!text.contains(line), "{line:?} present:\n{text}");
        }
    }
    Ok(())
}
