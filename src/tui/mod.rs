pub mod ansi;
pub mod controls;
pub mod keymap;
pub mod layout;
pub mod prompt;
pub mod state;
pub mod theme;
pub mod update;
pub mod view;

pub(crate) mod sources;

pub(crate) mod runtime;

#[cfg(test)]
mod testkit;

#[cfg(test)]
mod purity {
    #[test]
    fn core_modules_have_no_io_or_effect_execution() {
        let modules = [
            ("state", include_str!("state.rs")),
            ("update", include_str!("update.rs")),
            ("prompt", include_str!("prompt.rs")),
            ("keymap", include_str!("keymap.rs")),
            ("layout", include_str!("layout.rs")),
            ("view", include_str!("view.rs")),
            ("theme", include_str!("theme.rs")),
            ("ansi", include_str!("ansi.rs")),
        ];
        for (name, source) in modules {
            for forbidden in [
                "std::fs",
                "std::process",
                "std::net",
                "UnixStream",
                "File::",
                "thread::spawn",
                "lease::probe",
                "JobStore",
                "registry::",
                "TimeZone::system",
                "TimeZone::try_system",
                "text::age(",
                "text::local_hm(",
                "transcript::render_item(",
                "renderer::render_item(",
            ] {
                assert!(
                    !source.contains(forbidden),
                    "effect {forbidden} in tui::{name}"
                );
            }
        }
    }
    #[test]
    fn all_tui_and_shared_readers_have_no_direct_terminal_output() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut dirs = vec![root.join("src/tui")];
        let mut files = vec![
            root.join("src/commands/follow.rs"),
            root.join("src/inbox.rs"),
        ];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    dirs.push(path);
                } else if entry.file_type().unwrap().is_file() {
                    files.push(path);
                }
            }
        }
        files.sort();
        for path in files {
            let source = std::fs::read_to_string(&path).unwrap();
            for forbidden in [
                concat!("e", "print", "ln", "!("),
                concat!("print", "ln", "!("),
                concat!("pri", "nt", "!("),
            ] {
                assert!(
                    !source.contains(forbidden),
                    "direct terminal output in {}",
                    path.display()
                );
            }
        }
    }
}
