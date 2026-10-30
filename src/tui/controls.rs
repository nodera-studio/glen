//! Session controls use the CLI's routing, response validation and error mapping.
use super::state::{ControlData, ControlOp, ModelChoice, Target};
use crate::{
    cli::Cli,
    commands::{
        ctl::{self as command, Control},
        open::{self, Opened},
    },
    ctl::{
        self,
        route::{self, ModelsSource},
    },
    env::Env,
    error::{ErrorCode, GlenError, Warning},
    ops::{self, ModelRow, Op, OpError},
    render::text,
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// W3b's controls route is included unconditionally in this build.
pub const AVAILABLE: bool = true;

pub fn run(
    cli: &Cli,
    env: &Env,
    target: &Target,
    op: &ControlOp,
    stop: &Arc<AtomicBool>,
) -> Result<ControlData, GlenError> {
    let deadline = Instant::now() + Duration::from_secs(90);
    let arg = target.path.as_ref().map(|p| p.to_string_lossy());
    let opened = open::open_unprobed(cli, env, arg.as_deref().unwrap_or(&target.id))?;
    let control = match op {
        ControlOp::Abort => Some(Control::Abort),
        ControlOp::SetModel { provider, id } => Some(Control::Model(format!("{provider}/{id}"))),
        ControlOp::SetThinking(level) => Some(Control::Thinking(level.clone())),
        ControlOp::Compact => Some(Control::Compact(None)),
        ControlOp::ListModels => None,
    };
    for attempt in 0..2 {
        let (mut transport, mut warnings, source) = if let Some(control) = &control {
            let (transport, warnings) =
                route::route(cli, env, &opened, control.name(), deadline, stop.clone())?;
            (transport, warnings, None)
        } else {
            let (transport, warnings, source) =
                route::route_models(cli, env, Some(&opened), deadline, stop.clone())?;
            (transport, warnings, Some(source))
        };
        let job = transport.job().map(str::to_owned);
        let version = transport.ext_version().map(str::to_owned);
        let context = ctl::ErrCtx {
            handle: &opened.handle,
            session_id: &opened.session.header.id,
            route: transport.name(),
            job: job.as_deref(),
            ext_version: version.as_deref(),
        };
        let result = if let Some(control) = &control {
            command::execute(&mut transport, control, &context)
        } else {
            command::call(&mut transport, &Op::ListModels, &context).map(|data| (data, vec![]))
        };
        // Finish even after an op failure, before retrying a job that ended during routing.
        let finished = transport.finish();
        if stop.load(Ordering::SeqCst) {
            return Err(command::op_error(
                OpError::new("interrupted", "session control was interrupted"),
                &context,
            ));
        }
        let (mut data, op_warnings) = match result {
            Err(error)
                if attempt == 0
                    && error
                        .details
                        .as_ref()
                        .is_some_and(|d| d["reason"] == ops::JOB_ENDED) =>
            {
                continue;
            }
            result => result?,
        };
        let (fork, finish_warnings) = finished.map_err(|e| command::op_error(e, &context))?;
        warnings.extend(op_warnings);
        warnings.extend(finish_warnings);
        if let Some(source) = source {
            return model_choices(&mut data, source, &context);
        }
        if opened.not_yet_persisted
            && matches!(op, ControlOp::SetModel { .. } | ControlOp::SetThinking(_))
        {
            warnings.push(route::not_persisted());
        }
        let to = fork.map(|f| f.to).or_else(|| {
            data["session_id"]
                .as_str()
                .filter(|id| *id != opened.session.header.id)
                .map(str::to_owned)
        });
        let summary = if let Some(to) = to {
            format!("✓ forked → {}", crate::rpc::session_handle(&to))
        } else {
            summary(&opened, op, &data)
        };
        return Ok(ControlData::Done(with_warnings(summary, warnings)));
    }
    Err(GlenError::new(
        ErrorCode::ControlFailed,
        "the glen job ended again while applying a control",
    )
    .with_details(json!({"reason":"job_ended"})))
}

fn model_choices(
    data: &mut Value,
    source: ModelsSource,
    context: &ctl::ErrCtx<'_>,
) -> Result<ControlData, GlenError> {
    data["thinking_configured"] = Value::Null;
    match source {
        ModelsSource::Session => data["current_source"] = json!("session"),
        ModelsSource::Disk(selection) => {
            data["current_source"] = json!("disk");
            data["current"] = json!(selection.model.and_then(|m| {
                m.split_once('/')
                    .map(|(provider, id)| json!({"provider":provider,"id":id}))
            }));
            data["thinking_level"] = json!(selection.thinking_level);
            data["thinking_configured"] = json!(selection.thinking_configured);
        }
        ModelsSource::Profile | ModelsSource::Unpersisted => {
            data["current_source"] = Value::Null;
            data["current"] = Value::Null;
            data["thinking_level"] = Value::Null;
        }
    }
    let mut rows: Vec<ModelRow> = serde_json::from_value(data["models"].clone()).map_err(|e| {
        command::op_error(
            OpError::new(ops::INTERNAL, format!("invalid model rows: {e}")),
            context,
        )
    })?;
    rows.sort_by(|a, b| (&a.provider, &a.id).cmp(&(&b.provider, &b.id)));
    let current = |row: &ModelRow| {
        data["current"]["provider"] == row.provider && data["current"]["id"] == row.id
    };
    let thinking_levels = rows
        .iter()
        .find(|row| current(row))
        .map(|row| row.thinking_levels.clone())
        .unwrap_or_default();
    let models = rows
        .iter()
        .map(|row| ModelChoice {
            provider: row.provider.clone(),
            id: row.id.clone(),
            label: text::single_line(&format!(
                "{}/{} — {}{}",
                row.provider,
                row.id,
                row.name,
                if current(row) { " (current)" } else { "" }
            )),
        })
        .collect();
    Ok(ControlData::Models {
        models,
        thinking_levels,
    })
}
fn pair(data: &Value) -> String {
    match (data["provider"].as_str(), data["id"].as_str()) {
        (Some(provider), Some(id)) => text::single_line(&format!("{provider}/{id}")),
        _ => "unknown model".into(),
    }
}
fn summary(opened: &Opened, op: &ControlOp, data: &Value) -> String {
    let handle = text::single_line(&opened.handle);
    let level = text::single_line(data["thinking_level"].as_str().unwrap_or("unknown"));
    match op {
        ControlOp::Abort if data["aborted"] == false => format!("✓ {handle} idle"),
        ControlOp::Abort => format!("✓ {handle} aborted"),
        ControlOp::SetModel { .. } => {
            format!("✓ {handle} → {} · thinking {level}", pair(&data["model"]))
        }
        ControlOp::SetThinking(_) => {
            format!("✓ {handle} thinking {level} · {}", pair(&data["model"]))
        }
        ControlOp::Compact => format!("✓ {handle} compacted"),
        ControlOp::ListModels => unreachable!("model listings return choices"),
    }
}
fn with_warnings(mut summary: String, warnings: Vec<Warning>) -> String {
    for warning in warnings {
        summary.push_str(" · ");
        summary.push_str(&text::single_line(&warning.message));
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::palette::{ColorDepth, Theme};
    use crate::tui::{
        sources::tests::Rig,
        state::{App, Scope},
        theme::TuiTheme,
    };
    use clap::Parser;
    use std::{path::PathBuf, process::Command, sync::OnceLock, time::SystemTime};

    fn fake_omp() -> &'static PathBuf {
        static BIN: OnceLock<PathBuf> = OnceLock::new();
        BIN.get_or_init(|| {
            let executable = std::env::current_exe().unwrap();
            let profile = executable.parent().unwrap().parent().unwrap();
            let output = Command::new(env!("CARGO"))
                .args([
                    "build",
                    "--quiet",
                    "--locked",
                    "--offline",
                    "--example",
                    "fake_omp",
                ])
                .arg("--manifest-path")
                .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
                .arg("--target-dir")
                .arg(profile.parent().unwrap())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            profile.join("examples/fake_omp")
        })
    }
    fn env(rig: &Rig, bin: &str) -> Env {
        Env::for_test(
            Some(rig.temp.path()),
            rig.env.cwd(),
            &[
                ("PI_CODING_AGENT_DIR", rig.agent.to_str().unwrap()),
                ("GLEN_HOME", rig.home.root.to_str().unwrap()),
                ("GLEN_OMP_BIN", bin),
            ],
        )
    }
    fn target(rig: &Rig) -> Target {
        let (ctx, _) = rig.ctx();
        let opened = open::open_watch(&ctx.cli, &rig.env, "e27c0de").unwrap();
        Target {
            id: opened.session.header.id,
            path: Some(opened.session.path),
        }
    }
    #[test]
    fn controls_available_and_idle_abort_creates_no_state() {
        assert!(std::hint::black_box(AVAILABLE));
        let rig = Rig::new(true);
        let cli = Cli::try_parse_from(["glen", "watch"]).unwrap();
        let target = target(&rig);
        let before = std::fs::read(target.path.as_ref().unwrap()).unwrap();
        let result = run(
            &cli,
            &env(&rig, "/missing/omp"),
            &target,
            &ControlOp::Abort,
            &Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert!(matches!(result, ControlData::Done(s) if s == "✓ e27c0de idle"));
        assert!(!rig.home.root.exists());
        assert_eq!(
            before,
            std::fs::read(target.path.as_ref().unwrap()).unwrap()
        );
        let app = App::new(
            TuiTheme {
                depth: ColorDepth::None,
                theme: Theme::Dark,
            },
            Scope::All,
            (80, 24),
            Instant::now(),
            SystemTime::UNIX_EPOCH,
        );
        assert!(app.caps.controls);
    }
    #[test]
    fn listing_uses_disk_selection_and_never_resumes_or_creates_state() {
        let rig = Rig::new(true);
        let cli = Cli::try_parse_from(["glen", "watch"]).unwrap();
        let target = target(&rig);
        // A copied fixture's latest selection matches this fake peer model.
        let path = target.path.as_ref().unwrap();
        let mut session = String::from_utf8(crate::omp::fixtures::bytes('A').to_vec()).unwrap();
        session.push_str("\n{\"type\":\"model_change\",\"id\":\"watch-model\",\"parentId\":\"10000009\",\"model\":\"fake/fake-1\"}\n");
        std::fs::write(path, session).unwrap();
        let log = rig.temp.path().join("omp.log");
        // The real RPC launcher inherits environment; use the existing fake through
        // a test-owned wrapper rather than mutating this test process's environment.
        let wrapper = rig.temp.path().join("omp");
        let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nFAKE_OMP_LOG={} exec {} \"$@\"\n",
                quote(log.to_str().unwrap()),
                quote(fake_omp().to_str().unwrap())
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let env = env(&rig, wrapper.to_str().unwrap());
        let result = run(
            &cli,
            &env,
            &target,
            &ControlOp::ListModels,
            &Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        let ControlData::Models {
            models,
            thinking_levels,
        } = result
        else {
            panic!("model choices")
        };
        assert!(!models.is_empty());
        assert!(
            models.iter().any(|m| m.provider == "fake"
                && m.id == "fake-1"
                && m.label.ends_with(" (current)"))
        );
        assert_eq!(thinking_levels, ["off", "low", "medium", "high"]);
        assert!(!rig.home.root.exists());
        let frames = std::fs::read_to_string(log).unwrap();
        assert!(frames.contains("--no-session"));
        assert!(!frames.contains("--session\""));
    }
    #[test]
    fn unavailable_omp_is_a_sane_error_without_state_creation() {
        let rig = Rig::new(true);
        let cli = Cli::try_parse_from(["glen", "watch"]).unwrap();
        let error = run(
            &cli,
            &env(&rig, "/missing/omp"),
            &target(&rig),
            &ControlOp::ListModels,
            &Arc::new(AtomicBool::new(false)),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::OmpUnavailable);
        assert!(!error.message.is_empty());
        assert!(!rig.home.root.exists());
    }
    #[test]
    fn rpc_mutations_share_cli_validation_persist_selection_and_map_errors() {
        let rig = Rig::new(true);
        let cli = Cli::try_parse_from(["glen", "watch"]).unwrap();
        let target = target(&rig);
        let env = env(&rig, fake_omp().to_str().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        for (op, expected) in [
            (
                ControlOp::SetModel {
                    provider: "fake".into(),
                    id: "fake-1".into(),
                },
                "✓ e27c0de → fake/fake-1 · thinking",
            ),
            (
                ControlOp::SetThinking("high".into()),
                "✓ e27c0de thinking high · fake/fake-1",
            ),
            (ControlOp::Compact, "✓ e27c0de compacted"),
        ] {
            let result = run(&cli, &env, &target, &op, &stop).unwrap();
            assert!(matches!(result, ControlData::Done(s) if s.starts_with(expected)));
        }
        let opened =
            open::open_unprobed(&cli, &env, target.path.as_ref().unwrap().to_str().unwrap())
                .unwrap();
        let selected = opened.session.selection();
        assert_eq!(selected.model.as_deref(), Some("fake/fake-1"));
        assert_eq!(selected.thinking_level.as_deref(), Some("high"));
        let error = run(
            &cli,
            &env,
            &target,
            &ControlOp::SetModel {
                provider: "nokey".into(),
                id: "locked-1".into(),
            },
            &stop,
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::ControlFailed);
        assert_eq!(error.details.as_ref().unwrap()["reason"], ops::NO_AUTH);
        assert_eq!(
            error.hint.as_deref(),
            Some("log in to that provider in omp, then retry")
        );
    }
    #[test]
    fn cancellation_uses_the_shared_stop_flag_and_protocol_errors_keep_cli_hints() {
        let rig = Rig::new(true);
        let cli = Cli::try_parse_from(["glen", "watch"]).unwrap();
        let error = run(
            &cli,
            &env(&rig, "/missing/omp"),
            &target(&rig),
            &ControlOp::ListModels,
            &Arc::new(AtomicBool::new(true)),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::ControlFailed);
        assert_eq!(error.details.as_ref().unwrap()["reason"], "interrupted");
        let error = model_choices(
            &mut json!({"models":"broken"}),
            ModelsSource::Session,
            &ctl::ErrCtx {
                handle: "e27c0de",
                ..ctl::ErrCtx::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::ControlFailed);
        assert!(error.message.contains("e27c0de"));
        assert_eq!(error.details.as_ref().unwrap()["reason"], "protocol");
        assert!(error.hint.is_some());
    }
    #[test]
    fn choices_use_only_the_current_models_levels_and_clear_profile_selection() {
        let models: Vec<omp_rpc::ModelInfo> =
            serde_json::from_str(include_str!("../../tests/support/fake_models.json")).unwrap();
        let mut data = json!({"models":models.iter().map(ctl::row_from_info).collect::<Vec<_>>(),"current":{"provider":"fake","id":"fake-2"},"thinking_level":"off"});
        let result =
            model_choices(&mut data, ModelsSource::Session, &ctl::ErrCtx::default()).unwrap();
        assert!(
            matches!(result, ControlData::Models { thinking_levels, .. } if thinking_levels == ["off"])
        );
        let result = model_choices(
            &mut data,
            ModelsSource::Unpersisted,
            &ctl::ErrCtx::default(),
        )
        .unwrap();
        assert!(
            matches!(result, ControlData::Models { models, thinking_levels } if thinking_levels.is_empty() && models.iter().all(|m| !m.label.ends_with(" (current)")))
        );
    }
}
