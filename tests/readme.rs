//! The root README stays true to the binary and the site (W7 AC-740, AC-741, AC-742).

use std::{collections::BTreeSet, fs, path::Path};

fn read(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The text between a wave's `<!-- W*:start -->` and `<!-- W*:end -->` markers.
fn marker_block(readme: &str, wave: &str) -> String {
    let (start, end) = (
        format!("<!-- {wave}:start -->"),
        format!("<!-- {wave}:end -->"),
    );
    let a = readme
        .find(&start)
        .unwrap_or_else(|| panic!("README has no {start}"))
        + start.len();
    let b = readme[a..]
        .find(&end)
        .unwrap_or_else(|| panic!("README has no {end} after {start}"))
        + a;
    readme[a..b].to_string()
}

/// W8-DOC-1: v0.1.0 ships the live route and the skill, so their sections are filled in.
#[test]
fn readme_live_and_skill_sections_are_filled() {
    let readme = read("README.md");
    for (wave, needles) in [
        ("W3", ["glen install-extension", "glen_reply", "glen inbox"]),
        ("W4", ["glen install-skill", "--claude", "--codex"]),
    ] {
        let block = marker_block(&readme, wave);
        assert!(
            !block.contains("Coming in a later release"),
            "README {wave} block still says it ships later"
        );
        for needle in needles {
            assert!(block.contains(needle), "README {wave} block names {needle}");
        }
    }
    assert!(marker_block(&readme, "W4").contains("--project"));
    assert!(
        !readme.contains("Until the live route ships"),
        "README still says the live route is unshipped"
    );
}

/// Lines inside ``` fences.
fn code_lines(md: &str) -> Vec<&str> {
    let mut inside = false;
    md.lines()
        .filter(|line| {
            if line.trim_start().starts_with("```") {
                inside = !inside;
                return false;
            }
            inside
        })
        .collect()
}

/// `glen <subcommand>` invocations: the token `glen` followed by a word, comments dropped.
fn subcommands(lines: &[&str]) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for line in lines {
        let code = line.split(" #").next().unwrap_or_default();
        let tokens: Vec<&str> = code.split_whitespace().collect();
        for pair in tokens.windows(2) {
            if pair[0] == "glen" && pair[1].starts_with(|c: char| c.is_ascii_lowercase()) {
                found.insert(pair[1].to_string());
            }
        }
    }
    found
}

#[test]
fn readme_subcommands_exist_in_the_public_cli() {
    let public: BTreeSet<String> = glen::cli::public_command()
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .collect();
    // W8-DOC-1: the W3/W4 blocks are filled now, so their commands are checked too.
    let readme = read("README.md");
    let used = subcommands(&code_lines(&readme));
    for sub in [
        "ls",
        "show",
        "send",
        "wait",
        "install-extension",
        "install-skill",
        "inbox",
    ] {
        assert!(used.contains(sub), "the quick start must show glen {sub}");
    }
    let unknown: Vec<_> = used.difference(&public).collect();
    assert!(
        unknown.is_empty(),
        "README code uses glen subcommands that are hidden or missing: {unknown:?}"
    );
}

#[test]
fn subcommand_scan_skips_comments_paths_and_flags() {
    let lines = [
        "glen ls --all   # the paths glen uses",
        "cargo install --git https://github.com/nodera-studio/glen --locked",
        "xattr -d com.apple.quarantine ./glen",
        "echo hi | glen send @latest -",
    ];
    let found: Vec<_> = subcommands(&lines).into_iter().collect();
    assert_eq!(found, ["ls", "send"]);
}

/// A `const name = '…';` string from site.ts.
fn const_str(src: &str, name: &str) -> String {
    let head = format!("const {name} = '");
    let at = src
        .find(&head)
        .unwrap_or_else(|| panic!("site.ts has no {head}"))
        + head.len();
    src[at..at + src[at..].find('\'').expect("closing quote")].to_string()
}

/// A `key: `…`` template literal from site.ts, with its two constants substituted.
fn template(src: &str, key: &str) -> String {
    let head = format!("{key}: `");
    let at = src
        .find(&head)
        .unwrap_or_else(|| panic!("site.ts has no {head}"))
        + head.len();
    let raw = &src[at..at + src[at..].find('`').expect("closing backtick")];
    let out = raw
        .replace("${repoUrl}", &const_str(src, "repoUrl"))
        .replace("${installScriptUrl}", &const_str(src, "installScriptUrl"));
    assert!(!out.contains("${"), "unexpanded template in {key}: {out}");
    out
}

#[test]
fn readme_install_commands_match_the_site() {
    let site = read("site/src/data/site.ts");
    let readme = read("README.md");
    let code = code_lines(&readme);
    for key in ["script", "cargo"] {
        let command = template(&site, key);
        assert!(
            code.contains(&command.as_str()),
            "README needs the site's {key} command on its own line: {command}"
        );
    }
    for target in [
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "x86_64-unknown-linux-musl",
        "aarch64-unknown-linux-musl",
    ] {
        assert!(
            readme.contains(&format!("`glen-{target}.tar.gz`")),
            "README names the {target} archive"
        );
    }
}

#[test]
fn readme_has_screenshot_and_license() {
    let readme = read("README.md");
    assert!(
        readme.contains("](docs/demo.png)"),
        "README shows docs/demo.png"
    );
    assert!(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("docs/demo.png")
            .is_file(),
        "docs/demo.png exists (npm --prefix site run readme:shot)"
    );
    assert!(readme.contains("MIT") && readme.contains("](LICENSE)"));
}

/// The site URL is recorded in README and as astro.config.mjs's default `site` together. Before the
/// first deploy both still hold their placeholders: the README's `SITE_URL` line and no default.
#[test]
fn readme_site_url_matches_astro_config() {
    let config = read("site/astro.config.mjs");
    let readme = read("README.md");
    let head = "process.env.SITE_URL || '";
    let baked = config.find(head).map(|at| {
        let rest = &config[at + head.len()..];
        rest[..rest.find('\'').expect("closing quote")].to_string()
    });
    let placeholder = readme.lines().any(|line| line.trim() == "SITE_URL");
    match baked {
        Some(url) => {
            let host = url.trim_end_matches('/');
            assert!(
                host.starts_with("https://glen.") && host.ends_with(".workers.dev"),
                "astro.config.mjs default site is not a glen workers.dev URL: {url}"
            );
            assert!(readme.contains(host), "README must link {host}");
            assert!(!placeholder, "README still has the SITE_URL placeholder");
        }
        None => {
            assert!(
                config.contains("process.env.SITE_URL || undefined"),
                "astro.config.mjs: expected `process.env.SITE_URL || '<url>'` or the undeployed `|| undefined`"
            );
            assert!(
                placeholder,
                "README needs its SITE_URL placeholder line until the URL is baked in"
            );
            assert!(
                !readme.contains(".workers.dev"),
                "README names a workers.dev URL that astro.config.mjs does not"
            );
        }
    }
}
