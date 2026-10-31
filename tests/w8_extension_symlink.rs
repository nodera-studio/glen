//! W8 DEBT-2: `glen doctor` and `glen install-extension` judge a symlinked extension path the
//! same way, so doctor never recommends an install that `--force` cannot perform.
mod common;
use common::{OmpBox, data, fake_omp_bin, json};
use std::{fs, os::unix::fs::symlink};

#[test]
fn doctor_and_install_extension_agree_on_a_symlinked_extension_dir() {
    let b = OmpBox::new();
    let outside = b.tmp.path().join("dotfiles/glen");
    fs::create_dir_all(&outside).unwrap();
    let older = glen::extension::render("0.0.9", &b.glen_home);
    fs::write(outside.join("index.ts"), &older).unwrap();
    fs::create_dir_all(b.agent.join("extensions")).unwrap();
    symlink(&outside, b.agent.join("extensions/glen")).unwrap();
    let target = b.agent.join("extensions/glen/index.ts");

    let o = b
        .command(&["doctor", "--json"], &b.alpha())
        .env("GLEN_OMP_BIN", fake_omp_bin())
        .output()
        .unwrap();
    let d = data(&o);
    assert_eq!(d["extension"]["state"], "foreign", "{d}");
    assert_eq!(d["extension"]["current"], false);
    let check = d["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "extension")
        .unwrap();
    assert_eq!(check["status"], "warn");
    assert_eq!(check["value"], "foreign");
    let detail = check["detail"].as_str().unwrap();
    assert_eq!(
        detail,
        format!(
            "refusing symlink in extension path {}; --force cannot override this",
            target.display()
        )
    );
    assert!(!detail.contains("run `glen install-extension`"), "{detail}");

    for flags in [&[][..], &["--force"][..]] {
        let mut args = vec!["install-extension", "--json"];
        args.extend_from_slice(flags);
        let o = b.command(&args, &b.alpha()).output().unwrap();
        assert_eq!(data(&o)["action"], "skipped", "{flags:?}");
        let warning = &json(&o)["warnings"][0];
        assert_eq!(warning["code"], "unsafe_path", "{flags:?}");
        assert_eq!(warning["message"], detail, "{flags:?}");
        assert_eq!(fs::read_to_string(outside.join("index.ts")).unwrap(), older);
    }
}
