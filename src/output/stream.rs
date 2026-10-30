use super::{Mode, OutputCtx, envelope::error_value};
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
    failed: bool,
    out: Box<dyn Write>,
    err: Box<dyn Write>,
    err_painter: Painter,
}
impl EventStream {
    pub fn new(ctx: &OutputCtx) -> Self {
        use anstream::ColorChoice::{Always, Never};
        Self::with_writers(
            ctx.mode == Mode::Json,
            Box::new(anstream::AutoStream::new(
                io::stdout().lock(),
                if ctx.stdout_color || ctx.mode == Mode::Json {
                    Always
                } else {
                    Never
                },
            )),
            Box::new(anstream::AutoStream::new(
                io::stderr().lock(),
                if ctx.stderr_color { Always } else { Never },
            )),
            ctx.err_painter.clone(),
        )
    }
    pub fn ndjson(ctx: &OutputCtx) -> Self {
        let mut s = Self::new(ctx);
        s.json = true;
        s
    }
    pub fn with_writers(
        json: bool,
        out: Box<dyn Write>,
        err: Box<dyn Write>,
        painter: Painter,
    ) -> Self {
        Self {
            json,
            failed: false,
            out,
            err,
            err_painter: painter,
        }
    }
    fn line(&mut self, v: &Value) -> io::Result<()> {
        let mut b = serde_json::to_vec(v).map_err(io::Error::other)?;
        b.push(b'\n');
        let result = self.out.write_all(&b).and_then(|_| self.out.flush());
        self.failed |= result.is_err();
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
        if self.json {
            return Ok(());
        }
        let result = writeln!(self.out, "{s}").and_then(|_| self.out.flush());
        self.failed |= result.is_err();
        result
    }
    pub fn diagnostic(&mut self, s: &str) -> io::Result<()> {
        writeln!(
            self.err,
            "{}",
            self.err_painter.paint(Token::Muted, &sanitize(s))
        )?;
        self.err.flush()
    }
    pub fn warnings(&mut self, ws: &[Warning]) -> io::Result<()> {
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
                writeln!(
                    self.err,
                    "{} {}",
                    self.err_painter
                        .paint(Token::Warning, &self.err_painter.bold("warning:")),
                    sanitize(&w.message)
                )?;
                self.err.flush()?
            }
        }
        Ok(())
    }
    pub fn end(mut self, code: u8) -> ExitCode {
        if self.failed {
            return ExitCode::from(1);
        }
        if self
            .event("end", Map::from_iter([("ok".into(), json!(true))]))
            .is_err()
        {
            ExitCode::from(1)
        } else {
            ExitCode::from(code)
        }
    }
    pub fn fail(mut self, e: GlenError) -> ExitCode {
        if self.failed {
            return ExitCode::from(1);
        }
        let result = if self.json {
            self.line(&error_value(&e, &[]))
        } else {
            (locationless_error(&mut self.err, &self.err_painter, &e))
                .and_then(|_| self.err.flush())
        };
        ExitCode::from(if result.is_ok() {
            e.code.exit_code()
        } else {
            1
        })
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
    use std::{cell::RefCell, rc::Rc};
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
    struct Failing;
    impl Write for Failing {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn broken_pipe() {
        let s = EventStream::with_writers(
            true,
            Box::new(Failing),
            Box::new(Vec::<u8>::new()),
            Painter::new(ColorDepth::None, Theme::Dark),
        );
        assert_eq!(s.end(0), ExitCode::from(1));
    }
}
