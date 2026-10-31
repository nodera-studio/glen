use super::{Mode, OutputCtx, envelope::error_value, stdout_failure_exit};
use crate::{
    error::{GlenError, Warning},
    render::{palette::Token, sanitize, style::Painter},
};
use serde_json::{Map, Value, json};
use std::{
    io::{self, Write},
    process::ExitCode,
};
pub struct EventStream {
    json: bool,
    failure: Option<io::ErrorKind>,
    out: Box<dyn Write>,
    err: Box<dyn Write>,
    err_painter: Painter,
}
impl EventStream {
    pub fn new(ctx: &OutputCtx) -> Self {
        Self::for_mode(ctx, ctx.mode == Mode::Json)
    }
    /// Always NDJSON, whatever the mode. The stdout writer must then bypass anstream's
    /// stripping too, or DEL in message text is lost under `--human` (W8-ERR-6).
    pub fn ndjson(ctx: &OutputCtx) -> Self {
        Self::for_mode(ctx, true)
    }
    fn for_mode(ctx: &OutputCtx, json: bool) -> Self {
        use anstream::ColorChoice::{Always, Never};
        Self::with_writers(
            json,
            Box::new(anstream::AutoStream::new(
                super::stdout_writer(),
                if ctx.stdout_color || json {
                    Always
                } else {
                    Never
                },
            )),
            Box::new(anstream::AutoStream::new(
                super::stderr_writer(),
                if ctx.stderr_color { Always } else { Never },
            )),
            ctx.err_painter.clone(),
        )
    }
    pub fn with_writers(
        json: bool,
        out: Box<dyn Write>,
        err: Box<dyn Write>,
        painter: Painter,
    ) -> Self {
        Self {
            json,
            failure: None,
            out,
            err,
            err_painter: painter,
        }
    }
    fn writable(&self) -> io::Result<()> {
        match self.failure {
            Some(kind) => Err(kind.into()),
            None => Ok(()),
        }
    }
    fn failure_exit(&self, outcome: ExitCode) -> ExitCode {
        self.failure
            .map_or(outcome, |kind| stdout_failure_exit(&kind.into(), outcome))
    }
    pub fn write_exit(&self) -> ExitCode {
        match self.failure {
            Some(kind) => stdout_failure_exit(&kind.into(), ExitCode::SUCCESS),
            None => ExitCode::from(1),
        }
    }
    fn line(&mut self, v: &Value) -> io::Result<()> {
        self.writable()?;
        let mut b = super::envelope::to_json_vec(v, false)?;
        b.push(b'\n');
        let result = self.out.write_all(&b).and_then(|_| self.out.flush());
        if let Err(e) = &result {
            self.failure.get_or_insert(e.kind());
        }
        result
    }
    pub fn event(&mut self, kind: &str, mut fields: Map<String, Value>) -> io::Result<()> {
        if !self.json {
            return Ok(());
        }
        fields.insert("v".into(), json!(1));
        fields.insert("event".into(), json!(kind));
        self.line(&Value::Object(fields))
    }
    pub fn text(&mut self, s: &str) -> io::Result<()> {
        self.writable()?;
        if self.json {
            return Ok(());
        }
        let result = writeln!(self.out, "{s}").and_then(|_| self.out.flush());
        if let Err(e) = &result {
            self.failure.get_or_insert(e.kind());
        }
        result
    }
    pub fn diagnostic(&mut self, s: &str) -> io::Result<()> {
        self.writable()?;
        let result = writeln!(
            self.err,
            "{}",
            self.err_painter.paint(Token::Muted, &sanitize(s))
        )
        .and_then(|()| self.err.flush());
        if result.is_err() {
            // A broken stderr is always a failure, unlike a closed stdout reader.
            self.failure.get_or_insert(io::ErrorKind::Other);
        }
        result
    }
    pub fn warnings(&mut self, ws: &[Warning]) -> io::Result<()> {
        self.writable()?;
        for w in ws {
            if self.json {
                self.event(
                    "warning",
                    Map::from_iter([
                        ("code".into(), json!(w.code)),
                        ("message".into(), json!(w.message)),
                    ]),
                )?
            } else {
                let result = writeln!(
                    self.err,
                    "{} {}",
                    self.err_painter
                        .paint(Token::Warning, &self.err_painter.bold("warning:")),
                    sanitize(&w.message)
                )
                .and_then(|()| self.err.flush());
                if result.is_err() {
                    self.failure.get_or_insert(io::ErrorKind::Other);
                }
                result?;
            }
        }
        Ok(())
    }
    pub fn end(mut self, code: u8) -> ExitCode {
        let outcome = ExitCode::from(code);
        if self.failure.is_some() {
            return self.failure_exit(outcome);
        }
        if self
            .event("end", Map::from_iter([("ok".into(), json!(true))]))
            .is_err()
        {
            self.failure_exit(outcome)
        } else {
            ExitCode::from(code)
        }
    }
    pub fn fail(mut self, e: GlenError) -> ExitCode {
        let outcome = ExitCode::from(e.code.exit_code());
        if self.failure.is_some() {
            return self.failure_exit(outcome);
        }
        let result = if self.json {
            self.line(&error_value(&e, &[]))
        } else {
            (locationless_error(&mut self.err, &self.err_painter, &e))
                .and_then(|_| self.err.flush())
        };
        match result {
            Ok(()) => outcome,
            Err(error) if self.json => stdout_failure_exit(&error, outcome),
            Err(_) => ExitCode::from(1),
        }
    }
}
fn locationless_error(err: &mut dyn Write, p: &Painter, e: &GlenError) -> io::Result<()> {
    writeln!(
        err,
        "{} {}",
        p.paint(Token::Danger, &p.bold("error:")),
        sanitize(&e.message)
    )?;
    if let Some(h) = &e.hint {
        writeln!(
            err,
            "{}",
            p.paint(Token::Muted, &format!("  hint: {}", sanitize(h)))
        )?
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::palette::{ColorDepth, Theme};
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };
    #[derive(Clone, Default)]
    struct Sink(Rc<RefCell<Vec<u8>>>);
    impl Write for Sink {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    fn stream(out: Sink) -> EventStream {
        EventStream::with_writers(
            true,
            Box::new(out),
            Box::new(Vec::<u8>::new()),
            Painter::new(ColorDepth::None, Theme::Dark),
        )
    }
    #[test]
    fn trailers() {
        for code in [0, 130] {
            let out = Sink::default();
            let mut s = stream(out.clone());
            s.event("session", Map::new()).unwrap();
            s.event(
                "item",
                Map::from_iter([("item".into(), json!({"text":"\x1b"}))]),
            )
            .unwrap();
            assert_eq!(s.end(code), ExitCode::from(code));
            let bytes = out.0.borrow();
            assert!(!bytes.contains(&27));
            let lines = std::str::from_utf8(&bytes)
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str::<Value>(l).unwrap())
                .collect::<Vec<_>>();
            assert!(lines.iter().all(|v| v["v"] == 1));
            assert_eq!(
                lines.last().unwrap(),
                &json!({"v":1,"event":"end","ok":true})
            );
        }
    }
    /// W8-SEC-5: NDJSON lines escape C1 and bidi controls and decode unchanged.
    #[test]
    fn ndjson_escapes_c1_and_bidi_controls() {
        let out = Sink::default();
        let mut s = stream(out.clone());
        let text = "A\u{7f}B\u{85}C\u{9b}D\u{202e}E\u{2066}F";
        s.event("message", Map::from_iter([("text".into(), json!(text))]))
            .unwrap();
        let bytes = out.0.borrow();
        let line = std::str::from_utf8(&bytes).unwrap();
        for raw in ['\u{7f}', '\u{85}', '\u{9b}', '\u{202e}', '\u{2066}'] {
            assert!(!line.contains(raw), "{line}");
        }
        assert!(line.contains("\\u009b"), "{line}");
        assert_eq!(serde_json::from_str::<Value>(line).unwrap()["text"], text);
    }
    #[test]
    fn failure() {
        let out = Sink::default();
        assert_eq!(
            stream(out.clone()).fail(GlenError::new(crate::error::ErrorCode::NotFound, "gone")),
            ExitCode::from(3)
        );
        let v: Value = serde_json::from_slice(&out.0.borrow()).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["code"], "not_found");
    }
    struct Failing {
        kind: io::ErrorKind,
        flush_failure: bool,
        calls: Rc<Cell<usize>>,
    }
    impl Write for Failing {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.calls.set(self.calls.get() + 1);
            if self.flush_failure {
                Ok(bytes.len())
            } else {
                Err(self.kind.into())
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            if self.flush_failure {
                Err(self.kind.into())
            } else {
                Ok(())
            }
        }
    }
    fn failing_stream(
        kind: io::ErrorKind,
        flush_failure: bool,
    ) -> (EventStream, Rc<Cell<usize>>, Sink) {
        let calls = Rc::new(Cell::new(0));
        let err = Sink::default();
        let stream = EventStream::with_writers(
            true,
            Box::new(Failing {
                kind,
                flush_failure,
                calls: calls.clone(),
            }),
            Box::new(err.clone()),
            Painter::new(ColorDepth::None, Theme::Dark),
        );
        (stream, calls, err)
    }
    #[test]
    fn broken_pipe_and_other_errors_keep_only_the_broken_pipes_outcome() {
        for kind in [io::ErrorKind::BrokenPipe, io::ErrorKind::Other] {
            for flush in [false, true] {
                for code in [0, 10, 11, 130] {
                    let (s, calls, err) = failing_stream(kind, flush);
                    assert_eq!(
                        s.end(code),
                        ExitCode::from(if kind == io::ErrorKind::BrokenPipe {
                            code
                        } else {
                            1
                        })
                    );
                    assert_eq!(calls.get(), 1);
                    assert!(err.0.borrow().is_empty());
                }
                let (s, calls, err) = failing_stream(kind, flush);
                assert_eq!(
                    s.fail(GlenError::new(crate::error::ErrorCode::NotFound, "gone")),
                    ExitCode::from(if kind == io::ErrorKind::BrokenPipe {
                        3
                    } else {
                        1
                    })
                );
                assert_eq!(calls.get(), 1);
                assert!(err.0.borrow().is_empty());
            }
        }
    }
    #[test]
    fn first_failure_stops_all_output_and_suppresses_the_end_event() {
        for kind in [io::ErrorKind::BrokenPipe, io::ErrorKind::Other] {
            for flush in [false, true] {
                let (mut s, calls, err) = failing_stream(kind, flush);
                assert!(s.event("message", Map::new()).is_err());
                assert_eq!(
                    s.write_exit(),
                    ExitCode::from(if kind == io::ErrorKind::BrokenPipe {
                        0
                    } else {
                        1
                    })
                );
                assert!(s.event("message", Map::new()).is_err());
                assert!(s.text("later").is_err());
                assert!(s.diagnostic("must not print").is_err());
                assert!(
                    s.warnings(&[Warning {
                        code: "test".into(),
                        message: "must not print".into()
                    }])
                    .is_err()
                );
                assert_eq!(
                    s.end(0),
                    ExitCode::from(if kind == io::ErrorKind::BrokenPipe {
                        0
                    } else {
                        1
                    })
                );
                assert_eq!(calls.get(), 1);
                assert!(err.0.borrow().is_empty());

                let (mut s, calls, err) = failing_stream(kind, flush);
                assert!(s.event("item", Map::new()).is_err());
                assert_eq!(
                    s.fail(GlenError::new(crate::error::ErrorCode::NotFound, "gone")),
                    ExitCode::from(if kind == io::ErrorKind::BrokenPipe {
                        3
                    } else {
                        1
                    })
                );
                assert_eq!(calls.get(), 1);
                assert!(err.0.borrow().is_empty());
            }
        }
    }
    #[test]
    fn failed_stderr_diagnostics_and_warnings_exit_one_even_for_broken_pipe() {
        for warning in [false, true] {
            let out = Sink::default();
            let mut s = EventStream::with_writers(
                false,
                Box::new(out.clone()),
                Box::new(Failing {
                    kind: io::ErrorKind::BrokenPipe,
                    flush_failure: false,
                    calls: Rc::new(Cell::new(0)),
                }),
                Painter::new(ColorDepth::None, Theme::Dark),
            );
            let result = if warning {
                s.warnings(&[Warning {
                    code: "test".into(),
                    message: "warning".into(),
                }])
            } else {
                s.diagnostic("following")
            };
            assert!(result.is_err());
            assert_eq!(s.write_exit(), ExitCode::from(1));
            assert_eq!(s.end(0), ExitCode::from(1));
            assert!(out.0.borrow().is_empty());
        }
    }
    #[test]
    fn human_text_failures_also_stop_diagnostics_and_keep_the_outcome() {
        for kind in [io::ErrorKind::BrokenPipe, io::ErrorKind::Other] {
            for flush in [false, true] {
                let (mut s, calls, err) = failing_stream(kind, flush);
                s.json = false;
                assert!(s.text("message").is_err());
                let failed_calls = calls.get();
                assert!(s.diagnostic("following").is_err());
                assert_eq!(
                    s.end(11),
                    ExitCode::from(if kind == io::ErrorKind::BrokenPipe {
                        11
                    } else {
                        1
                    })
                );
                assert_eq!(calls.get(), failed_calls);
                assert!(err.0.borrow().is_empty());
            }
        }
    }
}
