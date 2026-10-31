use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
pub struct Spinner(Option<ProgressBar>);
impl Spinner {
    pub fn start(enabled: bool, msg: &str) -> Self {
        Self(if enabled {
            let bar = ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr());
            if let Ok(style) = ProgressStyle::with_template("{spinner} {msg}") {
                bar.set_style(style);
            }
            // Messages can carry session handles or model names from transcripts.
            bar.set_message(super::sanitize(msg).into_owned());
            bar.enable_steady_tick(std::time::Duration::from_millis(80));
            Some(bar)
        } else {
            None
        })
    }
    pub fn set_message(&self, msg: &str) {
        if let Some(bar) = &self.0 {
            bar.set_message(super::sanitize(msg).into_owned());
        }
    }
    pub fn finish(self) {
        if let Some(bar) = &self.0 {
            bar.finish_and_clear();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        if let Some(bar) = &self.0 {
            bar.finish_and_clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drop_finishes_and_clears_even_on_error() {
        let bar = ProgressBar::hidden();
        let observer = bar.clone();
        fn failing(bar: ProgressBar) -> Result<(), ()> {
            let _spinner = Spinner(Some(bar));
            Err(())
        }
        let error = failing(bar);
        assert!(error.is_err());
        assert!(observer.is_finished());
    }
    #[test]
    fn messages_escape_terminal_controls() {
        let spinner = Spinner::start(true, "stopping \x1bc\x1b[31mzz");
        let bar = spinner.0.as_ref().unwrap();
        assert!(!bar.message().contains('\x1b'));
        assert!(bar.message().contains("\\x1b"));
        spinner.set_message("next \u{9b}2J");
        assert!(!bar.message().contains('\u{9b}'));
        spinner.finish();
    }
}

#[cfg(test)]
mod clear_tests {
    use super::*;
    use std::{
        io,
        sync::{Arc, Mutex},
    };
    #[derive(Clone, Debug, Default)]
    struct Terminal(Arc<Mutex<String>>);
    impl indicatif::TermLike for Terminal {
        fn width(&self) -> u16 {
            80
        }
        fn move_cursor_up(&self, _: usize) -> io::Result<()> {
            Ok(())
        }
        fn move_cursor_down(&self, _: usize) -> io::Result<()> {
            Ok(())
        }
        fn move_cursor_right(&self, _: usize) -> io::Result<()> {
            Ok(())
        }
        fn move_cursor_left(&self, _: usize) -> io::Result<()> {
            Ok(())
        }
        fn write_line(&self, s: &str) -> io::Result<()> {
            self.write_str(s)
        }
        fn write_str(&self, s: &str) -> io::Result<()> {
            self.0.lock().unwrap().push_str(s);
            Ok(())
        }
        fn clear_line(&self) -> io::Result<()> {
            self.0.lock().unwrap().clear();
            Ok(())
        }
        fn flush(&self) -> io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn drop_erases_the_rendered_spinner_frame() {
        let terminal = Terminal::default();
        let bar = ProgressBar::with_draw_target(
            None,
            ProgressDrawTarget::term_like(Box::new(terminal.clone())),
        );
        bar.set_style(ProgressStyle::with_template("{spinner} {msg}").unwrap());
        bar.set_message("reading session");
        bar.tick();
        assert!(terminal.0.lock().unwrap().contains("reading session"));
        drop(Spinner(Some(bar)));
        assert!(terminal.0.lock().unwrap().trim().is_empty());
    }
}
