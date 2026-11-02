//! The MCP tool catalog stays generated (v0.2 W5, AC-v2-501 and AC-v2-506; design §A22):
//! `src/mcp/tools.json` equals `catalog::generate` over `docs/json.md` and clap, byte for byte,
//! and every schema comes from clap, never from a `<!-- contract: -->` example.
use clap::{Arg, ArgAction, CommandFactory};
use glen::{
    cli::{Cli, public_command},
    mcp::catalog::{self, Catalog},
};
use serde_json::{Value, json};
use std::path::PathBuf;

const REGENERATE: &str = "cargo test --locked --test mcp_contract regenerate_catalog -- --ignored";
const JSON_MD: &str = include_str!("../docs/json.md");
const TOOLS: [&str; 14] = [
    "glen_ls",
    "glen_show",
    "glen_tail",
    "glen_send",
    "glen_wait",
    "glen_jobs",
    "glen_abort",
    "glen_models",
    "glen_model",
    "glen_thinking",
    "glen_compact",
    "glen_inbox",
    "glen_release",
    "glen_doctor",
];

fn catalog_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/mcp/tools.json")
}
fn generated() -> String {
    catalog::generate(JSON_MD, &public_command())
        .unwrap_or_else(|e| panic!("cannot generate the MCP catalog: {e}"))
}
fn checked_in() -> Catalog {
    serde_json::from_str(catalog::TOOLS_JSON).unwrap()
}

#[test]
fn catalog_is_current() {
    let want = generated();
    let on_disk = std::fs::read_to_string(catalog_path()).unwrap();
    assert!(
        on_disk == want,
        "src/mcp/tools.json is stale against docs/json.md and clap; run `{REGENERATE}`"
    );
    assert!(
        catalog::TOOLS_JSON == want,
        "the embedded catalog is stale (rebuild after `{REGENERATE}`)"
    );
    assert_eq!(generated(), want, "generation is deterministic");
}

/// Rewrites `src/mcp/tools.json`; run it with `-- --ignored` after an annotation or clap edit.
#[test]
#[ignore = "writes src/mcp/tools.json"]
fn regenerate_catalog() {
    std::fs::write(catalog_path(), generated()).unwrap();
}

#[test]
fn drift_in_either_source_is_caught() {
    let current = generated();
    let doc = |from: &str, to: &str| {
        assert!(JSON_MD.contains(from), "{from}");
        catalog::generate(&JSON_MD.replacen(from, to, 1), &public_command()).unwrap()
    };
    for mutated in [
        doc("pass a shorter timeout", "pass a smaller timeout"),
        doc(
            "## `glen jobs`\n\n<!-- mcp: tool -->",
            "## `glen jobs`\n\n<!-- mcp: skip reason=tty -->",
        ),
        doc(
            "## `glen watch`\n\n<!-- mcp: skip reason=tty -->",
            "## `glen watch`\n\n<!-- mcp: tool -->",
        ),
        doc("<!-- mcp: tool omit=follow -->", "<!-- mcp: tool -->"),
        doc(
            "<!-- mcp: skip reason=shell-setup -->",
            "<!-- mcp: skip reason=installer -->",
        ),
    ] {
        assert_ne!(
            mutated, current,
            "an annotation edit must change tools.json"
        );
    }
    // A whitespace-only edit leaves the parsed annotation, and so the file, unchanged.
    assert_eq!(
        catalog::generate(
            &JSON_MD.replacen(
                "<!-- mcp: tool omit=follow -->",
                "<!-- mcp:  tool  omit=follow  -->",
                1
            ),
            &public_command()
        )
        .unwrap(),
        current
    );
    let clap = |name: &str, f: fn(clap::Command) -> clap::Command| {
        catalog::generate(JSON_MD, &public_command().mut_subcommand(name, f)).unwrap()
    };
    for mutated in [
        clap("ls", |c| {
            c.arg(
                Arg::new("extra")
                    .long("extra")
                    .action(ArgAction::SetTrue)
                    .help("x"),
            )
        }),
        clap("ls", |c| c.mut_arg("limit", |a| a.default_value("21"))),
        clap("ls", |c| c.mut_arg("limit", |a| a.help("other help"))),
        clap("ls", |c| {
            c.mut_arg("limit", |a| a.value_parser(clap::value_parser!(String)))
        }),
        clap("send", |c| {
            c.mut_arg("deliver_as", |a| a.value_parser(["auto", "steer"]))
        }),
        clap("show", |c| c.mut_arg("session", |a| a.required(false))),
        clap("jobs", |c| c.mut_arg("active", |a| a.long("only-active"))),
        clap("doctor", |c| c.about("Other about")),
    ] {
        assert_ne!(mutated, current, "a clap edit must change tools.json");
    }
}

#[test]
fn unmapped_clap_shapes_fail_generation_naming_the_argument() {
    for (cmd, needle) in [
        (
            public_command().mut_subcommand("ls", |c| {
                c.arg(
                    Arg::new("tag")
                        .long("tag")
                        .action(ArgAction::Append)
                        .help("t"),
                )
            }),
            "`glen ls` argument `tag`",
        ),
        (
            public_command().mut_subcommand("jobs", |c| {
                c.arg(
                    Arg::new("verbose")
                        .long("verbose")
                        .action(ArgAction::Count)
                        .help("v"),
                )
            }),
            "`glen jobs` argument `verbose`",
        ),
        (
            public_command().mut_subcommand("show", |c| {
                c.arg(
                    Arg::new("x")
                        .long("x-y")
                        .action(ArgAction::SetTrue)
                        .help("n"),
                )
                .arg(Arg::new("x_y").help("p"))
            }),
            "a second argument maps to property `x_y`",
        ),
    ] {
        let e = catalog::generate(JSON_MD, &cmd).unwrap_err();
        assert!(e.contains(needle), "{e}");
    }
}

#[test]
fn every_visible_command_alias_and_help_has_exactly_one_annotation() {
    let mut cmd = public_command();
    cmd.build();
    let mut want = vec![];
    for sub in cmd.get_subcommands().filter(|s| !s.is_hide_set()) {
        want.push(sub.get_name().to_owned());
        want.extend(sub.get_visible_aliases().map(str::to_owned));
    }
    assert!(want.contains(&"help".to_owned()) && want.contains(&"stop".to_owned()));
    let mut section = None;
    let mut found: Vec<String> = vec![];
    for line in JSON_MD.lines() {
        if line.starts_with("## ") {
            section = line
                .strip_prefix("## `glen ")
                .and_then(|r| r.strip_suffix('`'))
                .map(str::to_owned);
        } else if line.trim_start().starts_with("<!-- mcp:") {
            found.push(
                section
                    .clone()
                    .expect("an annotation outside a command section"),
            );
        }
    }
    let mut sorted_found = found.clone();
    sorted_found.sort();
    sorted_found.dedup();
    assert_eq!(
        found.len(),
        sorted_found.len(),
        "a section has two annotations"
    );
    let mut sorted_want = want.clone();
    sorted_want.sort();
    assert_eq!(sorted_found, sorted_want);
}

#[test]
fn the_catalog_holds_the_fourteen_tools_and_the_skips() {
    let c = checked_in();
    assert_eq!(c.catalog, 1);
    assert_eq!(
        c.tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        TOOLS
    );
    let skipped: Vec<(&str, &str)> = c
        .skipped
        .iter()
        .map(|s| (s.command.as_str(), s.reason.as_str()))
        .collect();
    let mut want = vec![
        ("stop", "alias"),
        ("watch", "tty"),
        ("install-extension", "installer"),
        ("install-skill", "installer"),
        ("completions", "shell-setup"),
        ("help", "help"),
    ];
    if skipped.iter().any(|(c, _)| *c == "mcp") {
        want.insert(2, ("mcp", "server"));
    }
    assert_eq!(skipped, want);
    for hidden in ["__acp", "__worker"] {
        assert!(!catalog::TOOLS_JSON.contains(hidden), "{hidden}");
        assert!(
            !JSON_MD.contains(&format!("## `glen {hidden}`")),
            "{hidden}"
        );
    }
    for t in &c.tools {
        let name_ok = t.name.len() <= 64
            && t.name.starts_with(|ch: char| ch.is_ascii_lowercase())
            && t.name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        assert!(name_ok, "{}", t.name);
        assert!(!t.description.trim().is_empty() && !t.description.contains('\u{1b}'));
        assert_eq!(t.name, format!("glen_{}", t.exec.command.replace('-', "_")));
        assert!(
            t.output_schema.is_none(),
            "{} declares an outputSchema",
            t.name
        );
        let props = t.input_schema["properties"].as_object().unwrap();
        for global in ["json", "human", "color", "profile", "help", "version"] {
            assert!(!props.contains_key(global), "{} has {global}", t.name);
        }
        assert_eq!(t.input_schema["additionalProperties"], false);
        assert_eq!(t.input_schema["type"], "object");
        let names: Vec<&str> = t.exec.args.iter().map(|a| a.name.as_str()).collect();
        let mut keys: Vec<&str> = props.keys().map(String::as_str).collect();
        let mut sorted = names.clone();
        sorted.sort();
        keys.sort();
        assert_eq!(sorted, keys, "{}: exec.args and properties agree", t.name);
    }
    for t in ["glen_tail", "glen_inbox"] {
        let tool = c.tools.iter().find(|x| x.name == t).unwrap();
        assert!(
            tool.input_schema["properties"].get("follow").is_none(),
            "{t}"
        );
    }
    assert!(!catalog::TOOLS_JSON.contains('\u{1b}'));
    let notes: Vec<&str> = JSON_MD
        .lines()
        .filter_map(|l| l.split_once("note=\"").map(|(_, rest)| rest))
        .map(|rest| rest.split('"').next().unwrap())
        .collect();
    assert_eq!(notes.len(), 2, "send and wait carry notes");
    for note in notes {
        assert!(note.len() <= 300 && note.is_ascii(), "{note}");
        assert!(note.contains("90 s"), "{note}");
    }
}

/// Applies `f` to the body of every `<!-- contract: -->` example block.
fn map_examples(md: &str, f: impl Fn(&str) -> String) -> String {
    let mut out = String::new();
    let (mut armed, mut inside, mut body) = (false, false, String::new());
    for line in md.split_inclusive('\n') {
        if inside {
            if line.trim_start().starts_with("```") {
                out.push_str(&f(&body));
                out.push_str(line);
                (inside, body) = (false, String::new());
            } else {
                body.push_str(line);
            }
            continue;
        }
        if armed && line.trim_start().starts_with("```") {
            (armed, inside) = (false, true);
        } else if line.starts_with("<!-- contract:") {
            armed = true;
        }
        out.push_str(line);
    }
    out
}

#[test]
fn examples_never_shape_the_catalog() {
    let current = generated();
    let nulls = map_examples(JSON_MD, |b| {
        b.replacen('{', "{\"zz\":null,", 1).replace("\":", "\": ")
    });
    let values = map_examples(JSON_MD, |b| {
        b.replace("true", "false")
            .replace("e27c0de", "fffffff")
            .replace('0', "7")
    });
    let empty = map_examples(JSON_MD, |_| String::new());
    for (what, doc) in [
        ("null fields", nulls),
        ("new values", values),
        ("no examples", empty),
    ] {
        assert_ne!(doc, JSON_MD, "{what}: the mutation changed nothing");
        assert_eq!(
            catalog::generate(&doc, &public_command()).unwrap(),
            current,
            "{what} changed tools.json"
        );
    }
    let first = JSON_MD.find("<!-- contract:").unwrap();
    let end = JSON_MD[first..].find("```\n\n").unwrap() + first + 4;
    let deleted = format!("{}{}", &JSON_MD[..first], &JSON_MD[end..]);
    assert_eq!(
        catalog::generate(&deleted, &public_command()).unwrap(),
        current
    );
}

#[test]
fn an_output_annotation_adds_exactly_that_output_schema() {
    let doc = JSON_MD.replacen(
        "## `glen jobs`\n\n<!-- mcp: tool -->",
        "## `glen jobs`\n\n<!-- mcp: tool output='{\"type\":\"object\",\"required\":[\"ok\"]}' -->",
        1,
    );
    let c: Catalog =
        serde_json::from_str(&catalog::generate(&doc, &public_command()).unwrap()).unwrap();
    let declared: Vec<_> = c
        .tools
        .iter()
        .filter(|t| t.output_schema.is_some())
        .collect();
    assert_eq!(declared.len(), 1);
    assert_eq!(declared[0].name, "glen_jobs");
    assert_eq!(
        declared[0].output_schema,
        Some(json!({"type":"object","required":["ok"]}))
    );
}

/// The schema of `glen <command>` property by property, straight from clap.
#[test]
fn schemas_equal_their_clap_definitions() {
    let c = checked_in();
    let full = Cli::command();
    for command in ["send", "wait", "thinking", "release", "ls", "inbox"] {
        let tool = c.tools.iter().find(|t| t.exec.command == command).unwrap();
        let sub = full.find_subcommand(command).unwrap();
        let schema = &tool.input_schema;
        let required: Vec<Value> = schema["required"].as_array().cloned().unwrap_or_default();
        for arg in sub
            .get_arguments()
            .filter(|a| !a.is_hide_set() && !a.is_global_set())
        {
            let name = match arg.get_long() {
                Some(long) if !arg.is_positional() => long.replace('-', "_"),
                _ => arg.get_id().to_string(),
            };
            let Some(p) = schema["properties"].get(&name) else {
                assert!(
                    (command == "tail" || command == "inbox") && name == "follow",
                    "{command} lacks {name}"
                );
                continue;
            };
            assert_eq!(
                required.contains(&json!(name)),
                arg.is_required_set(),
                "{command} {name} requiredness"
            );
            let flag = matches!(arg.get_action(), ArgAction::SetTrue);
            assert_eq!(p["type"] == "boolean", flag, "{command} {name} type");
            if !flag {
                let defaults: Vec<String> = arg
                    .get_default_values()
                    .iter()
                    .map(|v| v.to_string_lossy().into_owned())
                    .collect();
                let documented = match &p["default"] {
                    Value::Null => vec![],
                    Value::String(s) => vec![s.clone()],
                    other => vec![other.to_string()],
                };
                assert_eq!(documented, defaults, "{command} {name} default");
                let values: Vec<Value> = arg
                    .get_possible_values()
                    .iter()
                    .filter(|v| !v.is_hide_set())
                    .map(|v| json!(v.get_name()))
                    .collect();
                let listed = p["enum"].as_array().cloned().unwrap_or_default();
                assert_eq!(listed, values, "{command} {name} enum");
            }
            let help = arg.get_help().map(|h| h.to_string());
            assert_eq!(
                p["description"].as_str().map(str::to_owned),
                help,
                "{command} {name} help"
            );
        }
    }
    let thinking = c.tools.iter().find(|t| t.name == "glen_thinking").unwrap();
    assert_eq!(
        thinking.input_schema["properties"]["level"]["enum"],
        json!(["off", "minimal", "low", "medium", "high", "xhigh", "max"])
    );
    assert_eq!(
        thinking.input_schema["required"],
        json!(["session", "level"])
    );
    let release = c.tools.iter().find(|t| t.name == "glen_release").unwrap();
    assert!(
        release.input_schema.get("required").is_none(),
        "target is required unless --all"
    );
    assert_eq!(
        release.input_schema["properties"]["timeout"]["default"],
        "10s"
    );
}
