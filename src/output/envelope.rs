use super::{Mode, OutputCtx, stdout_failure_exit};
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
    pub after_output: Option<Box<dyn FnOnce() -> Result<(), GlenError>>>,
}
impl Outcome {
    pub fn new(data: Value, human: impl Into<String>) -> Self {
        Self {
            data,
            human: human.into(),
            warnings: vec![],
            exit: OkExit::Done,
            after_output: None,
        }
    }
}
fn writers(ctx: &OutputCtx) -> (impl Write, impl Write) {
    use anstream::ColorChoice::{Always, Never};
    (
        anstream::AutoStream::new(
            super::stdout_writer(),
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
            super::stderr_writer(),
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
fn write_output(
    ctx: &OutputCtx,
    bytes: &[u8],
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> io::Result<()> {
    match out.write_all(bytes).and_then(|_| out.flush()) {
        Ok(()) => Ok(()),
        Err(e) => {
            if ctx.mode == Mode::Human && e.kind() != io::ErrorKind::BrokenPipe {
                writeln!(
                    err,
                    "error: cannot write output: {}",
                    sanitize(&e.to_string())
                )
                .and_then(|()| err.flush())
                .map_err(io::Error::other)?;
            }
            Err(e)
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
fn home_write_error(
    ctx: &OutputCtx,
    command: &str,
    path: &std::path::Path,
    action: &str,
    e: io::Error,
) -> GlenError {
    if crate::sandbox::restricted(&ctx.env, &e) {
        crate::sandbox::map_home_write(&ctx.env, command, e)
    } else {
        GlenError::new(
            ErrorCode::Io,
            format!("cannot {action} {}: {e}", path.display()),
        )
    }
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
    home.ensure_dir("out")
        .map_err(|e| home_write_error(ctx, command, &dir, "create", e))?;
    // Human output can be explicitly forced to colour on a pipe. Spill files stay plain.
    let plain = anstream::adapter::strip_str(std::str::from_utf8(bytes).map_err(io::Error::other)?)
        .to_string();
    write_private_atomic(&path, plain.as_bytes())
        .map_err(|e| home_write_error(ctx, command, &path, "write", e))?;
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
    let after_output = outcome.after_output;
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
    let bytes = if !(command == "completions"
        || command == "install-skill" && outcome.data["dry_run"] == true)
        && !ctx.stdout_tty
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
    if let Err(e) = write_output(ctx, &bytes, out, err) {
        if e.kind() == io::ErrorKind::BrokenPipe {
            return stdout_failure_exit(&e, ExitCode::from(outcome.exit.exit_code()));
        }
        if after_output.is_some()
            && writeln!(
                err,
                "glen: cannot write inbox output; messages remain unread"
            )
            .and_then(|()| err.flush())
            .is_err()
        {
            return ExitCode::from(1);
        }
        return ExitCode::from(1);
    }
    if ctx.mode != Mode::Json && human_warnings(ctx, &warnings, err).is_err() {
        return ExitCode::from(1);
    }
    if let Some(commit) = after_output
        && let Err(e) = commit()
    {
        if writeln!(err, "glen: {}", sanitize(&e.to_string()))
            .and_then(|()| err.flush())
            .is_err()
        {
            return ExitCode::from(1);
        }
        return ExitCode::from(1);
    }
    ExitCode::from(outcome.exit.exit_code())
}
pub(crate) fn error_value(error: &GlenError, warnings: &[Warning]) -> Value {
    debug_assert!(
        error.code != ErrorCode::NoRoute
            || error
                .details
                .as_deref()
                .and_then(|details| details.get("reason"))
                .is_some_and(Value::is_string),
        "no_route errors require a string details.reason"
    );
    let mut object = json!({"code":error.code,"message":error.message});
    if let Some(hint) = &error.hint {
        object["hint"] = json!(hint);
    }
    if let Some(details) = &error.details {
        object["details"] = details.as_ref().clone();
    }
    let warnings = warnings
        .iter()
        .chain(&error.warnings)
        .cloned()
        .collect::<Vec<_>>();
    with_warnings(json!({"v":1,"ok":false,"error":object}), &warnings)
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
        let rendered = json_bytes(&error_value(&error, &ctx.warnings), ctx.stdout_tty);
        match rendered {
            Ok(b) => {
                if let Err(e) = write_output(ctx, &b, out, err) {
                    return stdout_failure_exit(&e, ExitCode::from(error.code.exit_code()));
                }
            }
            Err(_) => return ExitCode::from(1),
        }
    } else {
        if let Some(report) = error.report
            && let Err(e) = write_output(ctx, format!("{report}\n").as_bytes(), out, err)
        {
            return stdout_failure_exit(&e, ExitCode::from(error.code.exit_code()));
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
            human_warnings(
                ctx,
                &ctx.warnings
                    .iter()
                    .chain(&error.warnings)
                    .cloned()
                    .collect::<Vec<_>>(),
                err,
            )
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
    #[cfg(debug_assertions)]
    #[test]
    fn no_route_envelopes_require_a_string_reason_even_with_a_dynamic_code() {
        let code = ErrorCode::NoRoute;
        for details in [json!({}), json!({"reason":null}), json!({"reason":42})] {
            let error = GlenError::new(code, "invalid no_route fixture").with_details(details);
            assert!(std::panic::catch_unwind(|| error_value(&error, &[])).is_err());
        }
        let error = GlenError::no_route(crate::error::NoRouteReason::LiveOwner, "refused");
        assert_eq!(
            error_value(&error, &[])["error"]["details"]["reason"],
            "live_owner"
        );
    }
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
            if mode == Mode::Raw {
                OutputKind::Raw
            } else {
                OutputKind::Document
            },
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
    fn readonly_spill_failure_maps_for_host_network_disabled_commands() {
        let t = tempfile::tempdir().unwrap();
        let mut c = ctx(t.path(), Mode::Json, false);
        c.env = crate::env::Env::for_test(
            Some(t.path()),
            t.path(),
            &[("CODEX_SANDBOX_NETWORK_DISABLED", "1")],
        );
        let error = home_write_error(
            &c,
            "show",
            &t.path().join("out"),
            "write",
            io::ErrorKind::ReadOnlyFilesystem.into(),
        );
        assert_eq!(error.code, ErrorCode::NoRoute);
        assert_eq!(error.details.unwrap()["glen_home_writable"], false);
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
        kind: io::ErrorKind,
        flush_failure: bool,
    }
    impl Write for Failing {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.calls += 1;
            if self.remaining == 0 {
                return Err(self.kind.into());
            }
            let n = self.remaining.min(b.len());
            self.bytes.extend_from_slice(&b[..n]);
            self.remaining -= n;
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            if self.flush_failure {
                Err(self.kind.into())
            } else {
                Ok(())
            }
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
                kind: io::ErrorKind::BrokenPipe,
                flush_failure: false,
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
                ExitCode::SUCCESS
            );
            assert_eq!(out.bytes.len(), n);
            assert_eq!(out.calls, if n == 0 { 1 } else { 2 });
            assert!(err.is_empty());
        }
    }
    #[test]
    fn output_failures_keep_outcomes_only_for_broken_pipes_and_never_commit() {
        use std::{cell::Cell, rc::Rc};
        let t = tempfile::tempdir().unwrap();
        for mode in [Mode::Json, Mode::Human, Mode::Raw] {
            let c = ctx(t.path(), mode, false);
            for kind in [io::ErrorKind::BrokenPipe, io::ErrorKind::Other] {
                for flush_failure in [false, true] {
                    for exit in [
                        OkExit::Done,
                        OkExit::Running,
                        OkExit::Forked,
                        OkExit::Interrupted,
                    ] {
                        let committed = Rc::new(Cell::new(0));
                        let counter = committed.clone();
                        let mut outcome = Outcome::new(json!({"answer":42}), "answer");
                        outcome.exit = exit;
                        outcome.warnings.push(Warning {
                            code: "test".into(),
                            message: "must not print this warning".into(),
                        });
                        outcome.after_output = Some(Box::new(move || {
                            counter.set(counter.get() + 1);
                            Ok(())
                        }));
                        let mut out = Failing {
                            bytes: Vec::new(),
                            remaining: if flush_failure { usize::MAX } else { 0 },
                            calls: 0,
                            kind,
                            flush_failure,
                        };
                        let mut err = Vec::new();
                        let code = emit_to(&c, outcome, "test", &mut out, &mut err);
                        assert_eq!(
                            code,
                            ExitCode::from(if kind == io::ErrorKind::BrokenPipe {
                                exit.exit_code()
                            } else {
                                1
                            })
                        );
                        assert_eq!(committed.get(), 0);
                        assert_eq!(out.calls, 1);
                        if kind == io::ErrorKind::BrokenPipe {
                            assert!(err.is_empty());
                        }
                        assert!(
                            !String::from_utf8_lossy(&err).contains("must not print this warning")
                        );
                    }
                }
            }
            let committed = Rc::new(Cell::new(0));
            let counter = committed.clone();
            let mut outcome = Outcome::new(json!({}), "ok");
            outcome.after_output = Some(Box::new(move || {
                counter.set(counter.get() + 1);
                Ok(())
            }));
            assert_eq!(
                emit_to(&c, outcome, "test", &mut Vec::new(), &mut Vec::new()),
                ExitCode::SUCCESS
            );
            assert_eq!(committed.get(), 1);
        }
    }
    #[test]
    fn error_output_failures_keep_the_errors_code_only_for_broken_pipes() {
        let t = tempfile::tempdir().unwrap();
        for mode in [Mode::Json, Mode::Human, Mode::Raw] {
            let c = ctx(t.path(), mode, false);
            for code in ErrorCode::ALL {
                for kind in [io::ErrorKind::BrokenPipe, io::ErrorKind::Other] {
                    for flush_failure in [false, true] {
                        let mut out = Failing {
                            bytes: Vec::new(),
                            remaining: if flush_failure { usize::MAX } else { 0 },
                            calls: 0,
                            kind,
                            flush_failure,
                        };
                        let mut err = Vec::new();
                        let error = if code == ErrorCode::NoRoute {
                            GlenError::no_route(crate::error::NoRouteReason::LiveOwner, "failed")
                        } else {
                            GlenError::new(code, "failed")
                        };
                        let exit =
                            emit_error_to(&c, error.with_report("report"), &mut out, &mut err);
                        assert_eq!(
                            exit,
                            ExitCode::from(if kind == io::ErrorKind::BrokenPipe {
                                code.exit_code()
                            } else {
                                1
                            })
                        );
                        assert_eq!(out.calls, 1);
                        if kind == io::ErrorKind::BrokenPipe || mode == Mode::Json {
                            assert!(err.is_empty());
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn failed_stderr_writes_always_exit_one() {
        let t = tempfile::tempdir().unwrap();
        for mode in [Mode::Human, Mode::Raw] {
            let c = ctx(t.path(), mode, false);
            for kind in [io::ErrorKind::BrokenPipe, io::ErrorKind::Other] {
                for flush_failure in [false, true] {
                    let mut err = Failing {
                        bytes: Vec::new(),
                        remaining: if flush_failure { usize::MAX } else { 0 },
                        calls: 0,
                        kind,
                        flush_failure,
                    };
                    let mut out = Vec::new();
                    assert_eq!(
                        emit_error_to(
                            &c,
                            GlenError::no_route(
                                crate::error::NoRouteReason::LiveOwner,
                                "unavailable"
                            ),
                            &mut out,
                            &mut err
                        ),
                        ExitCode::from(1)
                    );
                    assert!(out.is_empty());
                }
            }
        }
        // A broken stderr must not turn a different stdout failure into EPIPE success.
        let c = ctx(t.path(), Mode::Human, false);
        let mut out = Failing {
            bytes: Vec::new(),
            remaining: 0,
            calls: 0,
            kind: io::ErrorKind::Other,
            flush_failure: false,
        };
        let mut err = Failing {
            bytes: Vec::new(),
            remaining: 0,
            calls: 0,
            kind: io::ErrorKind::BrokenPipe,
            flush_failure: false,
        };
        assert_eq!(
            emit_error_to(
                &c,
                GlenError::no_route(crate::error::NoRouteReason::LiveOwner, "unavailable")
                    .with_report("report"),
                &mut out,
                &mut err
            ),
            ExitCode::from(1)
        );
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
