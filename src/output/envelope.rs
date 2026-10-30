use super::{Mode, OutputCtx};
use crate::{
    error::{ErrorCode, GlenError, OkExit, Warning},
    glen_home::{self, write_private_atomic},
    render::{palette::Token, sanitize},
};
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Write},
    process::ExitCode,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
pub struct Outcome {
    pub data: Value,
    pub human: String,
    pub warnings: Vec<Warning>,
    pub exit: OkExit,
}
impl Outcome {
    pub fn new(data: Value, human: impl Into<String>) -> Self {
        Self {
            data,
            human: human.into(),
            warnings: vec![],
            exit: OkExit::Done,
        }
    }
}
fn writers(
    ctx: &OutputCtx,
) -> (
    anstream::AutoStream<io::StdoutLock<'static>>,
    anstream::AutoStream<io::StderrLock<'static>>,
) {
    use anstream::ColorChoice::{Always, Never};
    (
        anstream::AutoStream::new(
            io::stdout().lock(),
            // JSON and raw script output never carry ANSI (serde escapes ESC; completion
            // scripts are static) and must round-trip untrusted bytes such as DEL exactly, so
            // they bypass stripping rather than going through `Never`.
            if ctx.stdout_color || matches!(ctx.mode, Mode::Json | Mode::Raw) {
                Always
            } else {
                Never
            },
        ),
        anstream::AutoStream::new(
            io::stderr().lock(),
            if ctx.stderr_color { Always } else { Never },
        ),
    )
}
pub fn emit(ctx: &OutputCtx, outcome: Outcome, command: &str) -> ExitCode {
    let (mut out, mut err) = writers(ctx);
    emit_to(ctx, outcome, command, &mut out, &mut err)
}
pub fn emit_error(ctx: &OutputCtx, error: GlenError) -> ExitCode {
    let (mut out, mut err) = writers(ctx);
    emit_error_to(ctx, error, &mut out, &mut err)
}
fn json_bytes(value: &Value, pretty: bool) -> io::Result<Vec<u8>> {
    let mut bytes = if pretty {
        serde_json::to_vec_pretty(value)
    } else {
        serde_json::to_vec(value)
    }
    .map_err(io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
}
fn with_warnings(mut value: Value, warnings: &[Warning]) -> Value {
    if !warnings.is_empty() {
        value["warnings"] = json!(warnings);
    }
    value
}
fn write_output(ctx: &OutputCtx, bytes: &[u8], out: &mut dyn Write, err: &mut dyn Write) -> bool {
    match out.write_all(bytes).and_then(|_| out.flush()) {
        Ok(()) => true,
        Err(e) => {
            if ctx.mode == Mode::Human && e.kind() != io::ErrorKind::BrokenPipe {
                let _ = writeln!(
                    err,
                    "error: cannot write output: {}",
                    sanitize(&e.to_string())
                );
            }
            false
        }
    }
}
fn human_warnings(ctx: &OutputCtx, warnings: &[Warning], err: &mut dyn Write) -> io::Result<()> {
    for warning in warnings {
        writeln!(
            err,
            "{} {}",
            ctx.err_painter
                .paint(Token::Warning, &ctx.err_painter.bold("warning:")),
            sanitize(&warning.message)
        )?;
    }
    err.flush()
}
fn spill(
    ctx: &OutputCtx,
    bytes: &[u8],
    command: &str,
    warnings: &[Warning],
) -> Result<Vec<u8>, GlenError> {
    let home = glen_home::resolve(&ctx.env)?;
    let dir = home.root.join("out");
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_millis();
    let is_json = ctx.mode == Mode::Json;
    let path = dir.join(format!(
        "{ms}-{command}-{}.{}",
        std::process::id(),
        if is_json { "json" } else { "txt" }
    ));
    let json_path = if is_json {
        serde_json::to_value(&path).map_err(|e| {
            GlenError::new(
                ErrorCode::InvalidEnv,
                format!("cannot serialize spill path {}: {e}", path.display()),
            )
        })?
    } else {
        Value::Null
    };
    home.ensure_dir("out").map_err(|e| {
        GlenError::new(
            ErrorCode::Io,
            format!("cannot create {}: {e}", dir.display()),
        )
    })?;
    // Human output can be explicitly forced to colour on a pipe. Spill files stay plain.
    let plain = anstream::adapter::strip_str(std::str::from_utf8(bytes).map_err(io::Error::other)?)
        .to_string();
    write_private_atomic(&path, plain.as_bytes()).map_err(|e| {
        GlenError::new(
            ErrorCode::Io,
            format!("cannot write {}: {e}", path.display()),
        )
    })?;
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if entry.path() != path
                && entry.file_type().is_ok_and(|t| t.is_file())
                && entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| SystemTime::now().duration_since(t).ok())
                    .is_some_and(|age| age > Duration::from_secs(7 * 24 * 3600))
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    if is_json {
        json_bytes(
            &with_warnings(json!({"v":1,"ok":true,"spilled":{"path":json_path,"bytes":plain.len(),"format":"json"}}), warnings),
            false,
        ).map_err(GlenError::from)
    } else {
        Ok(format!(
            "output is {} KB; full output in {}\n",
            plain.len() / 1024,
            sanitize(&path.display().to_string())
        )
        .into_bytes())
    }
}
pub fn emit_to(
    ctx: &OutputCtx,
    outcome: Outcome,
    command: &str,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> ExitCode {
    let warnings = ctx
        .warnings
        .iter()
        .chain(&outcome.warnings)
        .cloned()
        .collect::<Vec<_>>();
    let rendered = match ctx.mode {
        Mode::Json => json_bytes(
            &with_warnings(json!({"v":1,"ok":true,"data":outcome.data}), &warnings),
            ctx.stdout_tty,
        ),
        Mode::Human => Ok(format!("{}\n", outcome.human).into_bytes()),
        Mode::Raw => Ok(outcome.human.into_bytes()),
    };
    let bytes = match rendered {
        Ok(bytes) => bytes,
        Err(e) => return emit_error_to(ctx, e.into(), out, err),
    };
    let bytes = if !ctx.stdout_tty
        && ctx.mode != Mode::Raw
        && ctx.max_output.is_some_and(|max| {
            anstream::adapter::strip_str(&String::from_utf8_lossy(&bytes))
                .to_string()
                .len()
                > max
        }) {
        match spill(ctx, &bytes, command, &warnings) {
            Ok(b) => b,
            Err(e) => return emit_error_to(ctx, e, out, err),
        }
    } else {
        bytes
    };
    if !write_output(ctx, &bytes, out, err) {
        return ExitCode::from(1);
    }
    if ctx.mode != Mode::Json && human_warnings(ctx, &warnings, err).is_err() {
        return ExitCode::from(1);
    }
    ExitCode::from(outcome.exit.exit_code())
}
pub fn emit_error_to(
    ctx: &OutputCtx,
    error: GlenError,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> ExitCode {
    // `Mode::Raw` only governs successful script output (§A10): stdout must stay pure script
    // bytes, so an error (env errors included, not just clap usage errors) always renders as
    // human text on stderr, never a JSON envelope on stdout, for a raw command without
    // `--json`. `decide_mode` only yields `Raw` when `--json` was not requested.
    let mode = if ctx.mode == Mode::Raw {
        Mode::Human
    } else {
        ctx.mode
    };
    if mode == Mode::Json {
        let mut object = json!({"code":error.code,"message":error.message});
        if let Some(hint) = error.hint {
            object["hint"] = json!(hint);
        }
        if let Some(details) = error.details {
            object["details"] = details;
        }
        let rendered = json_bytes(
            &with_warnings(json!({"v":1,"ok":false,"error":object}), &ctx.warnings),
            ctx.stdout_tty,
        );
        match rendered {
            Ok(b) => {
                if !write_output(ctx, &b, out, err) {
                    return ExitCode::from(1);
                }
            }
            Err(_) => return ExitCode::from(1),
        }
    } else {
        if let Some(report) = error.report
            && !write_output(ctx, format!("{report}\n").as_bytes(), out, err)
        {
            return ExitCode::from(1);
        }
        let p = &ctx.err_painter;
        let result = (|| {
            writeln!(
                err,
                "{} {}",
                p.paint(Token::Danger, &p.bold("error:")),
                sanitize(&error.message)
            )?;
            if let Some(hint) = error.hint {
                writeln!(
                    err,
                    "{}",
                    p.paint(Token::Muted, &format!("  hint: {}", sanitize(&hint)))
                )?;
            }
            human_warnings(ctx, &ctx.warnings, err)
        })();
        if result.is_err() {
            return ExitCode::from(1);
        }
    }
    ExitCode::from(error.code.exit_code())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cli::GlobalArgs,
        env::Env,
        error::ErrorCode,
        output::{OutputKind, Terminal},
    };
    use std::path::Path;
    fn ctx(home: &Path, mode: Mode, tty: bool) -> OutputCtx {
        let env = Env::for_test(
            Some(home),
            home,
            &[("GLEN_MAX_OUTPUT", "0"), ("CLICOLOR_FORCE", "1")],
        );
        OutputCtx::new(
            &GlobalArgs {
                json: mode == Mode::Json,
                human: mode == Mode::Human,
                ..Default::default()
            },
            OutputKind::Document,
            &env,
            &Terminal {
                stdout_tty: tty,
                stderr_tty: false,
                stdout_cols: None,
            },
        )
        .unwrap()
    }
    #[test]
    fn success_contract_and_warnings() {
        let t = tempfile::tempdir().unwrap();
        let mut c = ctx(t.path(), Mode::Json, false);
        let mut out = Vec::new();
        let mut err = Vec::new();
        let outcome = || Outcome::new(json!({"text":"a"}), "unused");
        assert_eq!(
            emit_to(&c, outcome(), "test", &mut out, &mut err),
            ExitCode::SUCCESS
        );
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v.as_object().unwrap().len(), 3);
        assert_eq!(v["v"], 1);
        assert_eq!(v["ok"], true);
        assert!(v.get("warnings").is_none());
        assert!(err.is_empty());
        assert!(!out.contains(&27));
        assert!(out.ends_with(b"\n"));
        c.warnings.push(Warning {
            code: "one".into(),
            message: "first".into(),
        });
        let mut o = outcome();
        o.warnings.push(Warning {
            code: "two".into(),
            message: "second".into(),
        });
        out.clear();
        emit_to(&c, o, "test", &mut out, &mut err);
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["warnings"].as_array().unwrap().len(), 2);
    }
    #[test]
    fn error_contract() {
        let t = tempfile::tempdir().unwrap();
        let c = ctx(t.path(), Mode::Json, false);
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(
            emit_error_to(
                &c,
                GlenError::new(ErrorCode::NotFound, "missing"),
                &mut out,
                &mut err
            ),
            ExitCode::from(3)
        );
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["error"]["code"], "not_found");
        assert_eq!(v["error"]["message"], "missing");
        assert!(v["error"].get("hint").is_none());
        assert!(err.is_empty());
    }
    #[test]
    fn human_errors() {
        let t = tempfile::tempdir().unwrap();
        let mut c = ctx(t.path(), Mode::Human, false);
        c.stderr_color = false;
        c.err_painter = crate::render::style::Painter::new(
            crate::render::palette::ColorDepth::None,
            crate::render::palette::Theme::Dark,
        );
        let mut out = Vec::new();
        let mut err = Vec::new();
        emit_error_to(
            &c,
            GlenError::new(ErrorCode::Io, "oops\x1b").with_hint("try again"),
            &mut out,
            &mut err,
        );
        let text = String::from_utf8(err).unwrap();
        assert!(text.starts_with("error: oops\\x1b\n"));
        assert!(text.contains("  hint: try again"));
    }
    #[test]
    fn spills_full_document_and_keeps_tty_unbounded() {
        let t = tempfile::tempdir().unwrap();
        let mut c = ctx(t.path(), Mode::Json, false);
        c.max_output = Some(100);
        let data = json!({"text":"x".repeat(450)});
        let full = json_bytes(&json!({"v":1,"ok":true,"data":data}), false).unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        emit_to(
            &c,
            Outcome::new(data.clone(), "unused"),
            "test",
            &mut out,
            &mut err,
        );
        let stub: Value = serde_json::from_slice(&out).unwrap();
        let path = Path::new(stub["spilled"]["path"].as_str().unwrap());
        assert_eq!(fs::read(path).unwrap(), full);
        assert_eq!(stub["spilled"]["bytes"], full.len());
        assert_eq!(crate::glen_home::mode_is_private(path), Some(true));
        assert!(path.starts_with(t.path().join(".glen/out")));
        for tty in [false, true] {
            c.stdout_tty = tty;
            c.max_output = if tty { Some(100) } else { None };
            out.clear();
            emit_to(
                &c,
                Outcome::new(data.clone(), "unused"),
                "test",
                &mut out,
                &mut err,
            );
            let v: Value = serde_json::from_slice(&out).unwrap();
            assert!(v.get("spilled").is_none());
            assert_eq!(v["data"], data);
        }
    }
    #[test]
    fn human_spill_has_exact_plain_bytes() {
        let t = tempfile::tempdir().unwrap();
        let mut c = ctx(t.path(), Mode::Human, false);
        c.max_output = Some(100);
        let full = format!("{}\n", "a".repeat(499));
        let mut out = Vec::new();
        let mut err = Vec::new();
        emit_to(
            &c,
            Outcome::new(json!(null), full.trim_end()),
            "test",
            &mut out,
            &mut err,
        );
        let paths = fs::read_dir(t.path().join(".glen/out"))
            .unwrap()
            .map(|p| p.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(paths.len(), 1);
        let bytes = fs::read(&paths[0]).unwrap();
        assert_eq!(bytes.len(), 500);
        assert_eq!(bytes, full.as_bytes());
        assert!(String::from_utf8(out).unwrap().contains("full output in"));
    }
    struct Failing {
        bytes: Vec<u8>,
        remaining: usize,
        calls: usize,
    }
    impl Write for Failing {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.calls += 1;
            if self.remaining == 0 {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            let n = self.remaining.min(b.len());
            self.bytes.extend_from_slice(&b[..n]);
            self.remaining -= n;
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn broken_and_partial_pipe_no_second_document() {
        let t = tempfile::tempdir().unwrap();
        let c = ctx(t.path(), Mode::Json, false);
        for n in [0, 10] {
            let mut out = Failing {
                bytes: vec![],
                remaining: n,
                calls: 0,
            };
            let mut err = Vec::new();
            assert_eq!(
                emit_to(
                    &c,
                    Outcome::new(json!({"long":"x".repeat(100)}), "unused"),
                    "test",
                    &mut out,
                    &mut err
                ),
                ExitCode::from(1)
            );
            assert_eq!(out.bytes.len(), n);
            assert_eq!(out.calls, if n == 0 { 1 } else { 2 });
            assert!(err.is_empty());
        }
    }
    #[test]
    fn raw_unmodified_and_errors_never_spill() {
        let t = tempfile::tempdir().unwrap();
        let mut c = ctx(t.path(), Mode::Json, false);
        c.mode = Mode::Raw;
        c.max_output = Some(1);
        let mut out = Vec::new();
        let mut err = Vec::new();
        emit_to(
            &c,
            Outcome::new(json!(null), "script\n"),
            "test",
            &mut out,
            &mut err,
        );
        assert_eq!(out, b"script\n");
        c.mode = Mode::Json;
        out.clear();
        emit_error_to(
            &c,
            GlenError::new(ErrorCode::Usage, "x".repeat(100)),
            &mut out,
            &mut err,
        );
        assert!(
            serde_json::from_slice::<Value>(&out)
                .unwrap()
                .get("error")
                .is_some()
        );
        assert!(!t.path().join(".glen").exists());
    }
    #[test]
    fn spill_stub_keeps_context_and_outcome_warnings() {
        let t = tempfile::tempdir().unwrap();
        let mut c = ctx(t.path(), Mode::Json, false);
        c.max_output = Some(1);
        c.warnings.push(Warning {
            code: "invalid_env".into(),
            message: "context".into(),
        });
        let mut outcome = Outcome::new(json!({"long": "x".repeat(100)}), "unused");
        outcome.warnings.push(Warning {
            code: "omp_version_untested".into(),
            message: "outcome".into(),
        });
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(
            emit_to(&c, outcome, "test", &mut out, &mut err),
            ExitCode::SUCCESS
        );
        let stub: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(stub["warnings"].as_array().unwrap().len(), 2);
        let full: Value =
            serde_json::from_slice(&fs::read(stub["spilled"]["path"].as_str().unwrap()).unwrap())
                .unwrap();
        assert_eq!(stub["warnings"], full["warnings"]);
        assert!(err.is_empty());
    }
    #[test]
    fn human_spill_threshold_ignores_ansi_bytes() {
        let t = tempfile::tempdir().unwrap();
        let mut c = ctx(t.path(), Mode::Human, false);
        c.max_output = Some(5);
        let text = "\x1b[31mabcd\x1b[0m";
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(
            emit_to(
                &c,
                Outcome::new(json!(null), text),
                "test",
                &mut out,
                &mut err
            ),
            ExitCode::SUCCESS
        );
        assert_eq!(out, format!("{text}\n").as_bytes());
        assert!(!t.path().join(".glen").exists());
        c.max_output = Some(4);
        out.clear();
        assert_eq!(
            emit_to(
                &c,
                Outcome::new(json!(null), text),
                "test",
                &mut out,
                &mut err
            ),
            ExitCode::SUCCESS
        );
        let path = fs::read_dir(t.path().join(".glen/out"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(fs::read(path).unwrap(), b"abcd\n");
    }
    #[test]
    fn home_resolution_is_deferred_until_spilling_and_errors_name_the_path() {
        let t = tempfile::tempdir().unwrap();
        let mut c = ctx(t.path(), Mode::Json, false);
        c.env = Env::for_test(Some(t.path()), t.path(), &[("GLEN_HOME", "relative")]);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = emit_to(
            &c,
            Outcome::new(json!(1), "unused"),
            "test",
            &mut out,
            &mut err,
        );
        assert_eq!(code, ExitCode::SUCCESS, "{}", String::from_utf8_lossy(&out));
        c.max_output = Some(1);
        out.clear();
        assert_eq!(
            emit_to(
                &c,
                Outcome::new(json!(1), "unused"),
                "test",
                &mut out,
                &mut err
            ),
            ExitCode::from(2)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap()["error"]["code"],
            "invalid_env"
        );
        let root = t.path().join("blocked");
        fs::write(&root, b"not a directory").unwrap();
        c.env = Env::for_test(
            Some(t.path()),
            t.path(),
            &[("GLEN_HOME", root.to_str().unwrap())],
        );
        out.clear();
        assert_eq!(
            emit_to(
                &c,
                Outcome::new(json!(1), "unused"),
                "test",
                &mut out,
                &mut err
            ),
            ExitCode::from(1)
        );
        let value: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["error"]["code"], "io");
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .contains(root.join("out").to_str().unwrap())
        );
        assert!(err.is_empty());
        let out_dir = t.path().join(".glen/out");
        fs::create_dir_all(&out_dir).unwrap();
        let e = spill(
            &ctx(t.path(), Mode::Json, false),
            b"{}",
            "missing/child",
            &[],
        )
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::Io);
        assert!(e.message.starts_with("cannot write "));
        assert!(e.message.contains(out_dir.to_str().unwrap()));
        assert!(e.message.contains("missing/child"));
    }
    #[cfg(unix)]
    #[test]
    fn non_utf8_spill_path_does_not_panic() {
        use std::os::unix::ffi::OsStringExt;
        let t = tempfile::tempdir().unwrap();
        let home = t
            .path()
            .join(std::ffi::OsString::from_vec(b"home\xff".to_vec()));
        let mut c = ctx(&home, Mode::Json, false);
        c.max_output = Some(1);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        assert_eq!(
            emit_to(
                &c,
                Outcome::new(json!(1), "unused"),
                "test",
                &mut out,
                &mut err
            ),
            ExitCode::from(2)
        );
        let value: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["error"]["code"], "invalid_env");
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .contains("cannot serialize spill path")
        );
        assert!(err.is_empty());
        assert!(!home.exists());
    }
}
