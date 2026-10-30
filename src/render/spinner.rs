use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
pub struct Spinner(Option<ProgressBar>);
impl Spinner {
    pub fn start(enabled: bool, msg: &str) -> Self {
        Self(if enabled {
            let bar = ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr());
            if let Ok(style) = ProgressStyle::with_template("{spinner} {msg}") {
                bar.set_style(style);
            }
            bar.set_message(msg.to_string());
            bar.enable_steady_tick(std::time::Duration::from_millis(80));
            Some(bar)
        } else {
            None
        })
    }
    pub fn finish(self) {
        if let Some(bar) = self.0 {
            bar.finish_and_clear();
        }
    }
}
