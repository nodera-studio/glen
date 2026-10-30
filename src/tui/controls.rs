//! W3b supplies the real controls route; this base deliberately has no controls I/O.
use super::state::{ControlData, ControlOp, Target};
use crate::{
    cli::Cli,
    env::Env,
    error::{ErrorCode, GlenError},
};
use std::sync::atomic::AtomicBool;

/// Switch to a cfg/check for `src/ctl::route` once W3b lands.
/// Whoever lands W3b second must flip availability and fill in `run`'s real dispatch.
pub const AVAILABLE: bool = false;

pub fn run(
    _cli: &Cli,
    _env: &Env,
    _target: &Target,
    _op: &ControlOp,
    _stop: &AtomicBool,
) -> Result<ControlData, GlenError> {
    Err(GlenError::new(
        ErrorCode::NotImplemented,
        "session controls are not in this build",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn every_control_is_unavailable_without_io_or_state_creation() {
        let tree = tempfile::tempdir().unwrap();
        let missing = tree.path().join("absent-home");
        let env = Env::for_test(
            Some(tree.path()),
            tree.path(),
            &[("GLEN_HOME", missing.to_str().unwrap())],
        );
        let cli = Cli::try_parse_from(["glen", "watch"]).unwrap();
        let target = Target {
            id: "absent-session".into(),
            path: Some(tree.path().join("absent.jsonl")),
        };
        assert!(!std::hint::black_box(AVAILABLE));
        let stop = AtomicBool::new(false);
        for op in [
            ControlOp::Abort,
            ControlOp::ListModels,
            ControlOp::SetModel {
                provider: "p".into(),
                id: "m".into(),
            },
            ControlOp::SetThinking("high".into()),
            ControlOp::Compact,
        ] {
            let error = run(&cli, &env, &target, &op, &stop).unwrap_err();
            assert_eq!(error.code, ErrorCode::NotImplemented);
            assert_eq!(error.message, "session controls are not in this build");
            assert!(!missing.exists());
        }
        let app = super::super::state::App::new(
            super::super::theme::TuiTheme {
                depth: crate::render::palette::ColorDepth::None,
                theme: crate::render::palette::Theme::Dark,
            },
            super::super::state::Scope::All,
            (80, 24),
            std::time::Instant::now(),
            std::time::SystemTime::now(),
        );
        assert_eq!(app.caps.controls, AVAILABLE);
    }
}
