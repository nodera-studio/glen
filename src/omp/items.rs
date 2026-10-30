use super::session::{Entry, display_name, string, text_of};
use crate::cli::Tools;
use serde::Serialize;
use serde_json::Value;
pub struct ItemOpts {
    pub thinking: bool,
    pub tools: Tools,
}
#[derive(Debug, Serialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub summary: String,
    #[serde(skip)]
    pub reply_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    pub result: Option<ToolResultView>,
}
#[derive(Debug, Serialize)]
pub struct ToolResultView {
    pub is_error: bool,
    pub text: String,
    pub chars: usize,
    pub truncated: bool,
    pub images: usize,
}
#[derive(Debug, Serialize)]
pub struct Mention {
    pub path: String,
    pub line_count: Option<u64>,
}
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Item {
    User {
        id: String,
        timestamp: Option<String>,
        text: String,
        images: usize,
    },
    Assistant {
        id: String,
        timestamp: Option<String>,
        text: String,
        model: Option<String>,
        stop_reason: Option<String>,
        error: Option<String>,
        has_thinking: bool,
        thinking_words: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    ToolResult {
        id: String,
        timestamp: Option<String>,
        tool_call_id: String,
        tool_name: String,
        is_error: bool,
        text: String,
        chars: usize,
        truncated: bool,
        images: usize,
    },
    Bash {
        id: String,
        timestamp: Option<String>,
        command: String,
        exit_code: Option<i64>,
        cancelled: bool,
        output: String,
        chars: usize,
        truncated: bool,
    },
    Python {
        id: String,
        timestamp: Option<String>,
        code: String,
        exit_code: Option<i64>,
        cancelled: bool,
        output: String,
        chars: usize,
        truncated: bool,
    },
    Developer {
        id: String,
        timestamp: Option<String>,
        text: String,
    },
    Custom {
        id: String,
        timestamp: Option<String>,
        custom_type: String,
        text: String,
    },
    FileMention {
        id: String,
        timestamp: Option<String>,
        files: Vec<Mention>,
    },
    Compaction {
        id: String,
        timestamp: Option<String>,
        tokens_before: Option<u64>,
        tokens_after: Option<u64>,
        summary: Option<String>,
    },
    Reset {
        id: String,
        timestamp: Option<String>,
    },
    BranchSummary {
        id: String,
        timestamp: Option<String>,
        summary: String,
    },
    ModelChange {
        id: String,
        timestamp: Option<String>,
        model: String,
    },
    ThinkingLevel {
        id: String,
        timestamp: Option<String>,
        level: Option<String>,
    },
}
fn images(c: &Value) -> usize {
    c.as_array()
        .into_iter()
        .flatten()
        .filter(|v| v["type"] == "image")
        .count()
}
fn clipped(s: String, tools: Tools) -> (String, usize, bool) {
    let chars = s.chars().count();
    let trunc = !matches!(tools, Tools::Full) && chars > 500;
    (
        if trunc {
            s.chars().take(500).collect()
        } else {
            s
        },
        chars,
        trunc,
    )
}
fn result(m: &Value, tools: Tools) -> ToolResultView {
    let (text, chars, truncated) = clipped(text_of(&m["content"], "\n"), tools);
    ToolResultView {
        is_error: m["isError"].as_bool().unwrap_or(false),
        text,
        chars,
        truncated,
        images: images(&m["content"]),
    }
}
fn summary(args: &Value) -> String {
    let text = [
        "command",
        "cmd",
        "path",
        "file_path",
        "filePath",
        "pattern",
        "query",
        "url",
        "description",
        "prompt",
    ]
    .iter()
    .find_map(|k| args[*k].as_str())
    .map(str::to_owned)
    .unwrap_or_else(|| args.to_string());
    display_name(&text)
        .unwrap_or_default()
        .chars()
        .take(120)
        .collect()
}
pub fn build_raw(entries: &[&Entry]) -> Vec<Value> {
    entries.iter().map(|e| e.raw.clone()).collect()
}
pub fn build(entries: &[&Entry], opts: &ItemOpts) -> Vec<Item> {
    let mut out = vec![];
    for e in entries {
        let id = e.id.clone();
        let timestamp = e
            .timestamp
            .as_deref()
            .and_then(|s| s.parse::<jiff::Timestamp>().ok())
            .map(|t| t.to_string());
        let v = &e.raw;
        let m = &v["message"];
        let text = || text_of(&m["content"], "\n");
        let field = |key: &str| string(m, key).unwrap_or_default();
        let item = match e.kind.as_str() {
            "model_change" => Item::ModelChange {
                id,
                timestamp,
                model: string(v, "model").unwrap_or_default(),
            },
            "thinking_level_change" => Item::ThinkingLevel {
                id,
                timestamp,
                level: string(v, "thinkingLevel"),
            },
            "compaction" => Item::Compaction {
                id,
                timestamp,
                tokens_before: v["tokensBefore"].as_u64(),
                tokens_after: v["tokensAfter"].as_u64(),
                summary: string(v, "shortSummary"),
            },
            "reset_boundary" => Item::Reset { id, timestamp },
            "branch_summary" => Item::BranchSummary {
                id,
                timestamp,
                summary: string(v, "summary").unwrap_or_default(),
            },
            "custom_message" if v["display"] == true => Item::Custom {
                id,
                timestamp,
                custom_type: string(v, "customType").unwrap_or_default(),
                text: text_of(&v["content"], "\n"),
            },
            "message" => match m["role"].as_str() {
                Some("user") => Item::User {
                    id,
                    timestamp,
                    text: text(),
                    images: images(&m["content"]),
                },
                Some("developer") => Item::Developer {
                    id,
                    timestamp,
                    text: text(),
                },
                Some("assistant") => {
                    let mut prose = vec![];
                    let mut thoughts = vec![];
                    let mut tool_calls = vec![];
                    if let Some(s) = m["content"].as_str() {
                        prose.push(s.to_owned())
                    }
                    for block in m["content"].as_array().into_iter().flatten() {
                        match block["type"].as_str() {
                            Some("text") => prose.push(string(block, "text").unwrap_or_default()),
                            Some("thinking") => {
                                thoughts.push(string(block, "thinking").unwrap_or_default())
                            }
                            Some("redactedThinking") => thoughts.push("(redacted)".into()),
                            Some("toolCall") => {
                                if !matches!(opts.tools, Tools::None) {
                                    tool_calls.push(ToolCall {
                                        id: string(block, "id").unwrap_or_default(),
                                        name: string(block, "name").unwrap_or_default(),
                                        summary: summary(&block["arguments"]),
                                        reply_text: (block["name"] == "glen_reply")
                                            .then(|| {
                                                block["arguments"]["text"]
                                                    .as_str()
                                                    .map(str::to_owned)
                                            })
                                            .flatten(),
                                        arguments: matches!(opts.tools, Tools::Full)
                                            .then(|| block["arguments"].clone()),
                                        result: None,
                                    })
                                }
                            }
                            Some("image" | "fallback" | "anthropicServerTool") => {}
                            Some(t) => prose.push(format!("[{t}]")),
                            None => {}
                        }
                    }
                    let thinking_words =
                        thoughts.iter().map(|s| s.split_whitespace().count()).sum();
                    Item::Assistant {
                        id,
                        timestamp,
                        text: prose.join("\n\n"),
                        model: super::listing::model_of(m),
                        stop_reason: string(m, "stopReason"),
                        error: string(m, "errorMessage"),
                        has_thinking: !thoughts.is_empty(),
                        thinking_words,
                        thinking: opts.thinking.then(|| thoughts.join("\n\n")),
                        tool_calls,
                    }
                }
                Some("toolResult") if !matches!(opts.tools, Tools::None) => {
                    let tool_call_id = field("toolCallId");
                    let r = result(m, opts.tools);
                    let call = out.iter_mut().rev().find_map(|i| {
                        if let Item::Assistant { tool_calls, .. } = i {
                            tool_calls.iter_mut().find(|c| c.id == tool_call_id)
                        } else {
                            None
                        }
                    });
                    if let Some(call) = call {
                        call.result = Some(r);
                        continue;
                    }
                    Item::ToolResult {
                        id,
                        timestamp,
                        tool_call_id,
                        tool_name: field("toolName"),
                        is_error: r.is_error,
                        text: r.text,
                        chars: r.chars,
                        truncated: r.truncated,
                        images: r.images,
                    }
                }
                Some(role @ ("bashExecution" | "pythonExecution"))
                    if !matches!(opts.tools, Tools::None) =>
                {
                    let (output, chars, cut) = clipped(field("output"), opts.tools);
                    let truncated = cut || m["truncated"].as_bool().unwrap_or(false);
                    let exit_code = m["exitCode"].as_i64();
                    let cancelled = m["cancelled"].as_bool().unwrap_or(false);
                    if role == "bashExecution" {
                        Item::Bash {
                            id,
                            timestamp,
                            command: field("command"),
                            exit_code,
                            cancelled,
                            output,
                            chars,
                            truncated,
                        }
                    } else {
                        Item::Python {
                            id,
                            timestamp,
                            code: field("code"),
                            exit_code,
                            cancelled,
                            output,
                            chars,
                            truncated,
                        }
                    }
                }
                Some("custom" | "hookMessage") if m["display"] == true => Item::Custom {
                    id,
                    timestamp,
                    custom_type: field("customType"),
                    text: text(),
                },
                Some("fileMention") => Item::FileMention {
                    id,
                    timestamp,
                    files: m["files"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|f| Mention {
                            path: string(f, "path").unwrap_or_default(),
                            line_count: f["lineCount"].as_u64(),
                        })
                        .collect(),
                },
                _ => continue,
            },
            _ => continue,
        };
        out.push(item);
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::omp::{fixtures, session::Session};
    use std::path::Path;
    fn data(k: char, thinking: bool, tools: Tools) -> Value {
        let s = Session::parse(Path::new("fixture"), fixtures::bytes(k)).unwrap();
        serde_json::to_value(build(&s.active_branch(), &ItemOpts { thinking, tools })).unwrap()
    }
    #[test]
    fn fixture_items() {
        let a = data('A', false, Tools::Collapsed);
        assert_eq!(
            a.as_array()
                .unwrap()
                .iter()
                .map(|v| v["kind"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "thinking_level",
                "model_change",
                "user",
                "assistant",
                "assistant"
            ]
        );
        assert_eq!(a[3]["tool_calls"][0]["name"], "read");
        assert_eq!(a[3]["tool_calls"][0]["summary"], "tests/parser_test.rs");
        assert_eq!(a[3]["has_thinking"], true);
        assert!(a[3].get("thinking").is_none());
        assert!(a[3]["tool_calls"][0].get("arguments").is_none());
        assert_eq!(
            a[3]["tool_calls"][0]["result"]["text"],
            "fn parser_test() { … }"
        );
        assert_eq!(
            data('A', true, Tools::Full)[3]["thinking"],
            "Check the fixture loader."
        );
        assert_eq!(
            data('A', true, Tools::Full)[3]["tool_calls"][0]["arguments"]["path"],
            "tests/parser_test.rs"
        );
        assert!(data('A', false, Tools::None)[3].get("tool_calls").is_none());
        let d = data('D', false, Tools::Collapsed);
        assert_eq!(
            d.as_array()
                .unwrap()
                .iter()
                .map(|v| v["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["40000001", "40000002", "40000005", "40000006", "40000007"]
        );
        let e = data('E', false, Tools::Collapsed);
        assert_eq!(e.as_array().unwrap().len(), 10);
        assert!(e[9]["text"].as_str().unwrap().contains("[future_block]"));
        assert_eq!(e[8]["exit_code"], 0);
    }
    #[test]
    fn unicode_result_limit() {
        let r = result(
            &serde_json::json!({"content":[{"type":"text","text":"界".repeat(600)}]}),
            Tools::Collapsed,
        );
        assert_eq!(r.text.chars().count(), 500);
        assert_eq!(r.chars, 600);
        assert!(r.truncated);
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use crate::omp::session::Session;
    use serde_json::json;
    use std::path::Path;
    #[test]
    fn unknown_blocks_survive_all_text_message_paths() {
        let mut bytes = b"{\"type\":\"session\",\"version\":3,\"id\":\"x\"}\n".to_vec();
        let mut previous = Value::Null;
        for (i, role) in ["user", "developer", "custom", "toolResult", "assistant"]
            .iter()
            .enumerate()
        {
            let id = i.to_string();
            let e = json!({"type":"message", "id":id,"parentId":previous,"message":{"role":role,"display":true,
                "content":[{"type":"text","text":"visible"},{"type":"future_block"},{"type":"image"},
                    {"type":"thinking"},{"type":"redactedThinking"},{"type":"toolCall"},
                    {"type":"fallback"},{"type":"anthropicServerTool"}]}});
            bytes.extend(format!("{e}\n").bytes());
            previous = id.into();
        }
        bytes.extend(format!("{}\n",json!({"type":"custom_message","id":"custom-entry","parentId":"4","display":true,"content":[{"type":"future_block"}]})).bytes());
        let s = Session::parse(Path::new("blocks"), &bytes).unwrap();
        let items = serde_json::to_value(build(
            &s.active_branch(),
            &ItemOpts {
                thinking: false,
                tools: Tools::Full,
            },
        ))
        .unwrap();
        for item in items.as_array().unwrap() {
            assert!(
                item["text"].as_str().unwrap().contains("[future_block]"),
                "{item}"
            );
        }
        for item in &items.as_array().unwrap()[..5] {
            let sep = if item["kind"] == "assistant" {
                "\n\n"
            } else {
                "\n"
            };
            assert_eq!(item["text"], format!("visible{sep}[future_block]"));
        }
    }
}
