//! The tool catalog (design §A22): its types, the embedded `tools.json`, and the generator that
//! builds it from the `<!-- mcp: … -->` annotations in `docs/json.md` plus clap.
//!
//! Every property's type, enum, default, description and requiredness comes from clap. The
//! generator never reads a `<!-- contract: -->` example, so example edits cannot change a schema.
use clap::{Arg, ArgAction};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    any::TypeId,
    collections::{BTreeMap, BTreeSet},
};

/// The checked-in catalog, embedded at build time; `tests/mcp_contract.rs` keeps it current.
pub const TOOLS_JSON: &str = include_str!("tools.json");

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    pub catalog: u32,
    pub tools: Vec<Tool>,
    pub skipped: Vec<Skip>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    pub exec: Exec,
}
/// How a tool call becomes argv: the subcommand and its arguments in clap declaration order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Exec {
    pub command: String,
    pub args: Vec<ArgSpec>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArgKind {
    Positional,
    Option,
    Flag,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArgSpec {
    /// The schema property name.
    pub name: String,
    pub kind: ArgKind,
    /// The long flag, without `--` (options and flags only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long: Option<String>,
    /// clap's positional index (positionals only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<usize>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Skip {
    pub command: String,
    pub reason: String,
}

/// The embedded catalog, parsed.
pub fn load() -> Result<Catalog, String> {
    serde_json::from_str(TOOLS_JSON)
        .map_err(|e| format!("the embedded MCP tool catalog is invalid: {e}"))
}

const REASONS: [&str; 6] = ["tty", "installer", "shell-setup", "server", "alias", "help"];

#[derive(Debug, PartialEq)]
enum Annotation {
    Tool {
        omit: Vec<String>,
        note: Option<String>,
        output: Option<Value>,
    },
    Skip {
        reason: String,
    },
}

#[derive(Debug, PartialEq)]
enum Raw<'a> {
    Bare(&'a str),
    Double(&'a str),
    Single(&'a str),
}

/// Parses one `<!-- mcp: VERB [key=value …] -->` line (design §A22 grammar).
fn parse_annotation(line: &str) -> Result<Annotation, String> {
    let body = line
        .trim()
        .strip_prefix("<!-- mcp:")
        .and_then(|s| s.strip_suffix("-->"))
        .ok_or("an annotation is one line `<!-- mcp: VERB [key=value …] -->`")?;
    let mut rest = body.trim_start();
    let verb_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let verb = &rest[..verb_end];
    rest = rest[verb_end..].trim_start();
    let mut pairs: BTreeMap<&str, Raw> = BTreeMap::new();
    while !rest.is_empty() {
        let eq = rest
            .find('=')
            .ok_or_else(|| format!("expected key=value at {rest:?}"))?;
        let key = &rest[..eq];
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_lowercase()) {
            return Err(format!("invalid key {key:?}"));
        }
        let after = &rest[eq + 1..];
        let (value, tail) = match after.as_bytes().first() {
            Some(&q @ (b'"' | b'\'')) => {
                let inner = &after[1..];
                let close = inner
                    .find(q as char)
                    .ok_or_else(|| format!("unterminated value for {key}"))?;
                let text = &inner[..close];
                let raw = if q == b'"' {
                    Raw::Double(text)
                } else {
                    Raw::Single(text)
                };
                (raw, &inner[close + 1..])
            }
            _ => {
                let end = after.find(char::is_whitespace).unwrap_or(after.len());
                let text = &after[..end];
                if text.is_empty()
                    || !text
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b',' | b'-'))
                {
                    return Err(format!("invalid bare value {text:?} for {key}"));
                }
                (Raw::Bare(text), &after[end..])
            }
        };
        let text = match value {
            Raw::Bare(t) | Raw::Double(t) | Raw::Single(t) => t,
        };
        if text.contains("--") {
            return Err(format!("the value of {key} contains `--`"));
        }
        if !tail.is_empty() && !tail.starts_with(char::is_whitespace) {
            return Err(format!("expected a space after the value of {key}"));
        }
        if pairs.insert(key, value).is_some() {
            return Err(format!("key {key} repeated"));
        }
        rest = tail.trim_start();
    }
    match verb {
        "tool" => {
            let (mut omit, mut note, mut output) = (vec![], None, None);
            for (key, value) in pairs {
                match (key, value) {
                    ("omit", Raw::Bare(v)) => {
                        omit = v.split(',').map(str::to_owned).collect();
                        if omit.iter().any(String::is_empty) {
                            return Err(format!("omit={v} has an empty property name"));
                        }
                    }
                    ("note", Raw::Double(v)) => {
                        if v.trim().is_empty() {
                            return Err("note is empty".into());
                        }
                        note = Some(v.to_owned());
                    }
                    ("output", Raw::Single(v)) => {
                        let schema: Value = serde_json::from_str(v)
                            .map_err(|e| format!("output is not JSON: {e}"))?;
                        if schema.get("type") != Some(&json!("object")) {
                            return Err(
                                "output must be a JSON object with \"type\":\"object\"".into()
                            );
                        }
                        output = Some(schema);
                    }
                    ("omit" | "note" | "output", _) => {
                        return Err(format!(
                            "{key} takes {}",
                            match key {
                                "omit" => "a bare comma-separated list",
                                "note" => "double-quoted text",
                                _ => "single-quoted JSON",
                            }
                        ));
                    }
                    (other, _) => return Err(format!("unknown key {other} for tool")),
                }
            }
            Ok(Annotation::Tool { omit, note, output })
        }
        "skip" => {
            let mut reason = None;
            for (key, value) in pairs {
                match (key, value) {
                    ("reason", Raw::Bare(v)) if REASONS.contains(&v) => reason = Some(v.to_owned()),
                    ("reason", v) => {
                        return Err(format!(
                            "unknown skip reason {v:?} (one of {})",
                            REASONS.join(", ")
                        ));
                    }
                    (other, _) => return Err(format!("unknown key {other} for skip")),
                }
            }
            let reason = reason.ok_or("skip needs reason=…")?;
            Ok(Annotation::Skip { reason })
        }
        "" => Err("missing verb (tool or skip)".into()),
        other => Err(format!("unknown verb {other:?} (tool or skip)")),
    }
}

/// `(line number, annotation line)` per `## \`glen <name>\`` section; `None` holds annotations
/// found outside every command section.
fn annotations(md: &str) -> BTreeMap<Option<&str>, Vec<(usize, &str)>> {
    let mut out: BTreeMap<Option<&str>, Vec<(usize, &str)>> = BTreeMap::new();
    let mut section = None;
    for (i, line) in md.lines().enumerate() {
        if line.starts_with("## ") {
            section = line
                .trim_end()
                .strip_prefix("## `glen ")
                .and_then(|r| r.strip_suffix('`'));
        } else if line.trim_start().starts_with("<!-- mcp:") {
            out.entry(section).or_default().push((i + 1, line));
        }
    }
    out
}

const UNSIGNED: [fn() -> TypeId; 6] = [
    TypeId::of::<u8>,
    TypeId::of::<u16>,
    TypeId::of::<u32>,
    TypeId::of::<u64>,
    TypeId::of::<u128>,
    TypeId::of::<usize>,
];
const SIGNED: [fn() -> TypeId; 6] = [
    TypeId::of::<i8>,
    TypeId::of::<i16>,
    TypeId::of::<i32>,
    TypeId::of::<i64>,
    TypeId::of::<i128>,
    TypeId::of::<isize>,
];

fn property(arg: &Arg, kind: ArgKind, at: &str) -> Result<Value, String> {
    let mut p = Map::new();
    if kind == ArgKind::Flag {
        p.insert("type".into(), json!("boolean"));
    } else {
        let id = arg.get_value_parser().type_id();
        let integer = if UNSIGNED.iter().any(|t| id == t()) {
            p.insert("type".into(), json!("integer"));
            p.insert("minimum".into(), json!(0));
            true
        } else if SIGNED.iter().any(|t| id == t()) {
            p.insert("type".into(), json!("integer"));
            true
        } else {
            p.insert("type".into(), json!("string"));
            false
        };
        let values: Vec<String> = arg
            .get_possible_values()
            .iter()
            .filter(|v| !v.is_hide_set())
            .map(|v| v.get_name().to_owned())
            .collect();
        if !values.is_empty() {
            p.insert("enum".into(), json!(values));
        }
        match arg.get_default_values() {
            [] => {}
            [one] => {
                let text = one
                    .to_str()
                    .ok_or_else(|| format!("{at}: its default is not UTF-8"))?;
                let value = if integer {
                    text.parse::<u64>()
                        .map(Value::from)
                        .or_else(|_| text.parse::<i64>().map(Value::from))
                        .map_err(|e| format!("{at}: integer default {text:?}: {e}"))?
                } else {
                    json!(text)
                };
                p.insert("default".into(), value);
            }
            _ => return Err(format!("{at}: more than one default value")),
        }
    }
    if let Some(help) = arg.get_help() {
        p.insert("description".into(), json!(help.to_string()));
    }
    Ok(Value::Object(p))
}

fn tool(
    sub: &clap::Command,
    omit: &[String],
    note: Option<&str>,
    output: Option<Value>,
) -> Result<Tool, String> {
    let command = sub.get_name();
    let mut properties = Map::new();
    let (mut required, mut args, mut names) = (vec![], vec![], BTreeSet::new());
    for arg in sub.get_arguments() {
        let id = arg.get_id().as_str();
        if arg.is_global_set() || arg.is_hide_set() || matches!(id, "help" | "version") {
            continue;
        }
        let at = format!("`glen {command}` argument `{id}`");
        let action = arg.get_action();
        let (name, kind, long, position) = if arg.is_positional() {
            if !matches!(action, ArgAction::Set) {
                return Err(format!("{at}: unsupported action {action:?}"));
            }
            let index = arg
                .get_index()
                .ok_or_else(|| format!("{at}: positional without an index"))?;
            (id.to_owned(), ArgKind::Positional, None, Some(index))
        } else {
            let kind = match action {
                ArgAction::SetTrue => ArgKind::Flag,
                ArgAction::Set => ArgKind::Option,
                other => return Err(format!("{at}: unsupported action {other:?}")),
            };
            let long = arg
                .get_long()
                .ok_or_else(|| format!("{at}: has no long flag"))?;
            (long.replace('-', "_"), kind, Some(long.to_owned()), None)
        };
        if !names.insert(name.clone()) {
            return Err(format!("{at}: a second argument maps to property `{name}`"));
        }
        if omit.contains(&name) {
            continue;
        }
        properties.insert(name.clone(), property(arg, kind, &at)?);
        if arg.is_required_set() {
            required.push(name.clone());
        }
        args.push(ArgSpec {
            name,
            kind,
            long,
            position,
        });
    }
    if let Some(missing) = omit.iter().find(|o| !names.contains(*o)) {
        return Err(format!(
            "`glen {command}`: omit={missing} names no property"
        ));
    }
    let mut schema = json!({"type":"object","properties":properties,"additionalProperties":false});
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    let about = sub.get_about().map(|a| a.to_string()).unwrap_or_default();
    Ok(Tool {
        name: format!("glen_{}", command.replace('-', "_")),
        description: match note {
            Some(note) => format!("{about} {note}"),
            None => about,
        },
        input_schema: schema,
        output_schema: output,
        exec: Exec {
            command: command.to_owned(),
            args,
        },
    })
}

/// Renders `tools.json` (pretty JSON, one trailing newline) from `docs/json.md` and clap.
pub fn generate(json_md: &str, cmd: &clap::Command) -> Result<String, String> {
    let mut cmd = cmd.clone();
    cmd.build();
    // Visible names in clap declaration order: (name, the command it aliases).
    let mut visible: Vec<(&str, &clap::Command, Option<&str>)> = vec![];
    for sub in cmd.get_subcommands().filter(|s| !s.is_hide_set()) {
        visible.push((sub.get_name(), sub, None));
        for alias in sub.get_visible_aliases() {
            visible.push((alias, sub, Some(sub.get_name())));
        }
    }
    let mut found: BTreeMap<&str, (usize, &str)> = BTreeMap::new();
    for (section, lines) in annotations(json_md) {
        for (line, text) in lines {
            let Some(name) = section else {
                return Err(format!(
                    "docs/json.md:{line}: an mcp annotation outside a `glen <command>` section"
                ));
            };
            if !visible.iter().any(|(n, ..)| *n == name) {
                return Err(format!(
                    "docs/json.md:{line}: `glen {name}` is not a visible command, so its section takes no mcp annotation"
                ));
            }
            if found.insert(name, (line, text)).is_some() {
                return Err(format!(
                    "docs/json.md:{line}: a second mcp annotation for `glen {name}`"
                ));
            }
        }
    }
    let (mut tools, mut skipped) = (vec![], vec![]);
    for (name, sub, alias_of) in visible {
        let (line, text) = found.get(name).ok_or_else(|| {
            format!("docs/json.md: the `glen {name}` section has no `<!-- mcp: … -->` annotation")
        })?;
        let at = |e: String| format!("docs/json.md:{line}: {e}");
        match parse_annotation(text).map_err(at)? {
            Annotation::Skip { reason } => {
                if reason == "alias" && alias_of.is_none() {
                    return Err(at(format!(
                        "reason=alias needs a visible alias, and `glen {name}` is a command"
                    )));
                }
                skipped.push(Skip {
                    command: name.to_owned(),
                    reason,
                });
            }
            Annotation::Tool { .. } if alias_of.is_some() => {
                return Err(at(format!(
                    "`glen {name}` is an alias of `glen {}`; use skip reason=alias",
                    alias_of.unwrap_or_default()
                )));
            }
            Annotation::Tool { omit, note, output } => {
                tools.push(tool(sub, &omit, note.as_deref(), output).map_err(at)?);
            }
        }
    }
    let mut text = serde_json::to_string_pretty(&Catalog {
        catalog: 1,
        tools,
        skipped,
    })
    .map_err(|e| e.to_string())?;
    text.push('\n');
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Command, builder::PossibleValuesParser, value_parser};

    #[test]
    fn annotation_grammar() {
        let tool = |omit: &[&str], note: Option<&str>, output: Option<Value>| Annotation::Tool {
            omit: omit.iter().map(|s| (*s).to_owned()).collect(),
            note: note.map(str::to_owned),
            output,
        };
        for (line, want) in [
            ("<!-- mcp: tool -->", tool(&[], None, None)),
            ("  <!-- mcp:tool   -->  ", tool(&[], None, None)),
            (
                "<!-- mcp: tool omit=follow -->",
                tool(&["follow"], None, None),
            ),
            (
                "<!-- mcp: tool omit=a,b_c -->",
                tool(&["a", "b_c"], None, None),
            ),
            (
                "<!-- mcp: tool note=\"Waits 90 s; pass a shorter wait.\" -->",
                tool(&[], Some("Waits 90 s; pass a shorter wait."), None),
            ),
            (
                "<!-- mcp: tool output='{\"type\":\"object\"}' omit=x -->",
                tool(&["x"], None, Some(json!({"type":"object"}))),
            ),
        ] {
            assert_eq!(parse_annotation(line).unwrap(), want, "{line}");
        }
        for reason in REASONS {
            assert_eq!(
                parse_annotation(&format!("<!-- mcp: skip reason={reason} -->")).unwrap(),
                Annotation::Skip {
                    reason: reason.to_owned()
                }
            );
        }
        for bad in [
            "<!-- mcp: -->",
            "<!-- mcp: tools -->",
            "<!-- mcp: skip -->",
            "<!-- mcp: skip reason=bogus -->",
            "<!-- mcp: skip reason=\"tty\" -->",
            "<!-- mcp: skip reason=tty note=\"x\" -->",
            "<!-- mcp: tool reason=tty -->",
            "<!-- mcp: tool color=red -->",
            "<!-- mcp: tool omit= -->",
            "<!-- mcp: tool omit=a,,b -->",
            "<!-- mcp: tool omit=\"follow\" -->",
            "<!-- mcp: tool omit=fo$llow -->",
            "<!-- mcp: tool note=bare -->",
            "<!-- mcp: tool note=\"\" -->",
            "<!-- mcp: tool note=\"unterminated -->",
            "<!-- mcp: tool note=\"has -- dashes\" -->",
            "<!-- mcp: tool note=\"a\"b -->",
            "<!-- mcp: tool note=\"a\" note=\"b\" -->",
            "<!-- mcp: tool output='[1]' -->",
            "<!-- mcp: tool output='{\"type\":\"string\"}' -->",
            "<!-- mcp: tool output='{nope' -->",
            "<!-- mcp: tool output=\"{}\" -->",
            "<!-- mcp: tool OMIT=x -->",
            "<!-- mcp: tool omit -->",
            "<!-- mcp: tool -- -->",
            "<!-- mcp: tool",
            "<!-- contract: tool -->",
        ] {
            assert!(parse_annotation(bad).is_err(), "{bad} parsed");
        }
    }

    fn root() -> Command {
        Command::new("glen")
            .arg(
                Arg::new("json")
                    .long("json")
                    .global(true)
                    .action(ArgAction::SetTrue)
                    .help("Print a JSON envelope"),
            )
            .subcommand(
                Command::new("ls")
                    .about("List things")
                    .visible_alias("list")
                    .arg(
                        Arg::new("limit")
                            .short('n')
                            .long("limit")
                            .value_parser(value_parser!(usize))
                            .default_value("20")
                            .help("maximum rows"),
                    )
                    .arg(
                        Arg::new("all")
                            .long("all")
                            .action(ArgAction::SetTrue)
                            .help("every project"),
                    ),
            )
            .subcommand(
                Command::new("send-it")
                    .about("Send")
                    .arg(Arg::new("session").required(true).help("the session"))
                    .arg(Arg::new("message").help("the text"))
                    .arg(
                        Arg::new("deliver")
                            .long("as")
                            .value_parser(PossibleValuesParser::new(["auto", "steer"]))
                            .default_value("auto")
                            .help("how"),
                    )
                    .arg(
                        Arg::new("wait_for")
                            .long("follow-up")
                            .action(ArgAction::SetTrue),
                    ),
            )
            .subcommand(Command::new("secret").hide(true).about("hidden"))
    }
    const DOC: &str = "# json\n\n## Modes\n\nprose\n\n## `glen ls`\n\n<!-- mcp: tool -->\n\n\
        ## `glen list`\n<!-- mcp: skip reason=alias -->\n## `glen send-it`\n\n\
        <!-- mcp: tool note=\"A note.\" -->\n\n## `glen help`\n\n<!-- mcp: skip reason=help -->\n";

    fn catalog(doc: &str, cmd: &Command) -> Result<Catalog, String> {
        generate(doc, cmd).map(|s| serde_json::from_str(&s).unwrap())
    }

    #[test]
    fn schemas_come_from_clap() {
        let c = catalog(DOC, &root()).unwrap();
        assert_eq!(
            c.tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            ["glen_ls", "glen_send_it"]
        );
        assert_eq!(
            c.skipped,
            [
                Skip {
                    command: "list".into(),
                    reason: "alias".into()
                },
                Skip {
                    command: "help".into(),
                    reason: "help".into()
                }
            ]
        );
        let ls = &c.tools[0];
        assert_eq!(ls.description, "List things");
        assert_eq!(
            ls.input_schema,
            json!({"type":"object","additionalProperties":false,"properties":{
                "limit":{"type":"integer","minimum":0,"default":20,"description":"maximum rows"},
                "all":{"type":"boolean","description":"every project"}}})
        );
        let send = &c.tools[1];
        assert_eq!(send.description, "Send A note.");
        assert_eq!(send.input_schema["required"], json!(["session"]));
        assert_eq!(
            send.input_schema["properties"]["as"],
            json!({"type":"string","enum":["auto","steer"],"default":"auto","description":"how"})
        );
        assert_eq!(send.input_schema["properties"]["message"]["type"], "string");
        assert_eq!(
            send.input_schema["properties"]["follow_up"],
            json!({"type":"boolean"})
        );
        assert!(send.input_schema["properties"].get("json").is_none());
        assert_eq!(
            send.exec.args,
            [
                ArgSpec {
                    name: "session".into(),
                    kind: ArgKind::Positional,
                    long: None,
                    position: Some(1)
                },
                ArgSpec {
                    name: "message".into(),
                    kind: ArgKind::Positional,
                    long: None,
                    position: Some(2)
                },
                ArgSpec {
                    name: "as".into(),
                    kind: ArgKind::Option,
                    long: Some("as".into()),
                    position: None
                },
                ArgSpec {
                    name: "follow_up".into(),
                    kind: ArgKind::Flag,
                    long: Some("follow-up".into()),
                    position: None
                },
            ]
        );
        assert_eq!(generate(DOC, &root()), generate(DOC, &root()));
        let omitted = catalog(
            &DOC.replace("<!-- mcp: tool -->", "<!-- mcp: tool omit=all -->"),
            &root(),
        )
        .unwrap();
        assert!(
            omitted.tools[0].input_schema["properties"]
                .get("all")
                .is_none()
        );
        assert_eq!(omitted.tools[0].exec.args.len(), 1);
        let typed = catalog(
            &DOC.replace(
                "<!-- mcp: tool -->",
                "<!-- mcp: tool output='{\"type\":\"object\",\"x\":1}' -->",
            ),
            &root(),
        )
        .unwrap();
        assert_eq!(
            typed.tools[0].output_schema,
            Some(json!({"type":"object","x":1}))
        );
        assert_eq!(typed.tools[1].output_schema, None);
    }

    #[test]
    fn sections_are_checked() {
        let cmd = root();
        for (doc, needle) in [
            (
                DOC.replace("<!-- mcp: skip reason=help -->\n", ""),
                "`glen help` section has no",
            ),
            (
                format!("{DOC}<!-- mcp: tool -->\n"),
                "second mcp annotation for `glen help`",
            ),
            (
                DOC.replace("prose", "<!-- mcp: tool -->"),
                "outside a `glen <command>` section",
            ),
            (
                format!("{DOC}\n## `glen secret`\n<!-- mcp: tool -->\n"),
                "`glen secret` is not a visible command",
            ),
            (
                format!("{DOC}\n## `glen bogus`\n<!-- mcp: tool -->\n"),
                "`glen bogus` is not a visible command",
            ),
            (
                DOC.replace("<!-- mcp: tool -->", "<!-- mcp: tool output='[]' -->"),
                "output must be",
            ),
            (
                DOC.replace("<!-- mcp: tool -->", "<!-- mcp: skip reason=alias -->"),
                "reason=alias needs a visible alias",
            ),
            (
                DOC.replace("<!-- mcp: skip reason=alias -->", "<!-- mcp: tool -->"),
                "is an alias of `glen ls`",
            ),
            (
                DOC.replace("<!-- mcp: tool -->", "<!-- mcp: tool omit=nope -->"),
                "omit=nope names no property",
            ),
        ] {
            let e = generate(&doc, &cmd).unwrap_err();
            assert!(e.contains(needle), "{e}");
        }
        // A section heading that is not `glen <name>` ends the command section.
        let e = generate(
            &DOC.replace("## `glen help`\n\n<!-- mcp", "## Not a command\n\n<!-- mcp"),
            &cmd,
        )
        .unwrap_err();
        assert!(e.contains("outside a `glen <command>` section"), "{e}");
    }

    #[test]
    fn unmapped_argument_shapes_fail_generation() {
        let with = |arg: Arg| root().mut_subcommand("ls", |c| c.arg(arg));
        for (arg, needle) in [
            (
                Arg::new("tag").long("tag").action(ArgAction::Append),
                "`glen ls` argument `tag`",
            ),
            (
                Arg::new("verbose").long("verbose").action(ArgAction::Count),
                "`glen ls` argument `verbose`",
            ),
            (
                Arg::new("short").short('s').action(ArgAction::SetTrue),
                "has no long flag",
            ),
            (
                Arg::new("files").action(ArgAction::Append),
                "`glen ls` argument `files`",
            ),
        ] {
            let e = generate(DOC, &with(arg)).unwrap_err();
            assert!(e.contains(needle), "{e}");
        }
        // `--all-x` and a positional `all_x` would both be property `all_x`.
        let clash = root().mut_subcommand("ls", |c| {
            c.arg(Arg::new("allx").long("all-x").action(ArgAction::SetTrue))
                .arg(Arg::new("all_x"))
        });
        let e = generate(DOC, &clash).unwrap_err();
        assert!(
            e.contains("a second argument maps to property `all_x`"),
            "{e}"
        );
        // A hidden argument and a global argument are never properties.
        let c = catalog(
            DOC,
            &with(
                Arg::new("secret")
                    .long("secret")
                    .hide(true)
                    .action(ArgAction::Count),
            ),
        )
        .unwrap();
        assert!(
            c.tools[0].input_schema["properties"]
                .get("secret")
                .is_none()
        );
    }

    #[test]
    fn the_real_catalog_from_docs_and_clap() {
        let text = generate(
            include_str!("../../docs/json.md"),
            &crate::cli::public_command(),
        )
        .unwrap();
        assert!(!text.contains('\u{1b}'));
        assert!(text.ends_with("}\n") && !text.ends_with("\n\n"));
        let c: Catalog = serde_json::from_str(&text).unwrap();
        assert_eq!(
            c.tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            [
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
                "glen_doctor"
            ]
        );
        let tool = |n: &str| c.tools.iter().find(|t| t.name == n).unwrap();
        for t in &c.tools {
            let props = t.input_schema["properties"].as_object().unwrap();
            for global in ["json", "human", "color", "profile", "help", "version"] {
                assert!(!props.contains_key(global), "{} has {global}", t.name);
            }
        }
        for t in ["glen_tail", "glen_inbox"] {
            assert!(
                tool(t).input_schema["properties"].get("follow").is_none(),
                "{t}"
            );
            assert!(!tool(t).exec.args.iter().any(|a| a.name == "follow"), "{t}");
        }
        let send = &tool("glen_send").input_schema;
        assert_eq!(send["required"], json!(["session", "message"]));
        for p in [
            "as",
            "no_wait",
            "follow_up",
            "steer",
            "wait",
            "cwd",
            "thinking",
            "warm",
        ] {
            assert!(send["properties"].get(p).is_some(), "send lacks {p}");
        }
        assert_eq!(
            send["properties"]["as"]["enum"],
            json!(["auto", "steer", "follow-up"])
        );
        let wait = &tool("glen_wait").input_schema["properties"];
        assert_eq!(wait["timeout"]["type"], "string");
        assert_eq!(wait["timeout"]["default"], "90s");
        assert_eq!(wait["for"]["enum"], json!(["reply"]));
        assert_eq!(
            tool("glen_ls").input_schema["properties"]["limit"]["default"],
            20
        );
        assert!(tool("glen_release").input_schema.get("required").is_none());
        assert!(c.tools.iter().all(|t| t.output_schema.is_none()));
    }
}
