use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Internal,
    Io,
    Usage,
    InvalidEnv,
    NotFound,
    Ambiguous,
    OmpUnavailable,
    NoRoute,
    JobFailed,
    ControlFailed,
    NotImplemented,
}
impl ErrorCode {
    pub const ALL: [Self; 11] = [
        Self::Internal,
        Self::Io,
        Self::Usage,
        Self::InvalidEnv,
        Self::NotFound,
        Self::Ambiguous,
        Self::OmpUnavailable,
        Self::NoRoute,
        Self::JobFailed,
        Self::ControlFailed,
        Self::NotImplemented,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::Io => "io",
            Self::Usage => "usage",
            Self::InvalidEnv => "invalid_env",
            Self::NotFound => "not_found",
            Self::Ambiguous => "ambiguous",
            Self::OmpUnavailable => "omp_unavailable",
            Self::NoRoute => "no_route",
            Self::JobFailed => "job_failed",
            Self::ControlFailed => "control_failed",
            Self::NotImplemented => "not_implemented",
        }
    }
    pub fn exit_code(self) -> u8 {
        match self {
            Self::Internal | Self::Io => 1,
            Self::Usage | Self::InvalidEnv => 2,
            Self::NotFound => 3,
            Self::Ambiguous => 4,
            Self::OmpUnavailable => 5,
            Self::NoRoute => 6,
            Self::JobFailed | Self::ControlFailed => 7,
            Self::NotImplemented => 8,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum OkExit {
    Done,
    Running,
    Forked,
    Interrupted,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoRouteReason {
    LiveOwner,
    UnknownLease,
    UnhealthyEndpoint,
    EndpointGone,
    EndpointAppeared,
    RpcJobRunning,
    Sandboxed,
    ExtensionOutdated,
}
impl NoRouteReason {
    pub const ALL: [Self; 8] = [
        Self::LiveOwner,
        Self::UnknownLease,
        Self::UnhealthyEndpoint,
        Self::EndpointGone,
        Self::EndpointAppeared,
        Self::RpcJobRunning,
        Self::Sandboxed,
        Self::ExtensionOutdated,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LiveOwner => "live_owner",
            Self::UnknownLease => "unknown_lease",
            Self::UnhealthyEndpoint => "unhealthy_endpoint",
            Self::EndpointGone => "endpoint_gone",
            Self::EndpointAppeared => "endpoint_appeared",
            Self::RpcJobRunning => "rpc_job_running",
            Self::Sandboxed => "sandboxed",
            Self::ExtensionOutdated => "extension_outdated",
        }
    }
}
impl OkExit {
    pub fn exit_code(self) -> u8 {
        match self {
            Self::Done => 0,
            Self::Running => 10,
            Self::Forked => 11,
            Self::Interrupted => 130,
        }
    }
}
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct GlenError {
    pub code: ErrorCode,
    pub message: String,
    pub hint: Option<String>,
    pub details: Option<Box<serde_json::Value>>,
    pub report: Option<String>,
    pub warnings: Vec<Warning>,
}
impl GlenError {
    pub fn no_route(reason: NoRouteReason, message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NoRoute, message).with_detail("reason", reason.as_str())
    }
    pub fn read(path: &std::path::Path, error: std::io::Error) -> Self {
        Self::new(
            if error.kind() == std::io::ErrorKind::NotFound {
                ErrorCode::NotFound
            } else {
                ErrorCode::Io
            },
            format!("cannot read {}: {error}", path.display()),
        )
    }
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
            details: None,
            report: None,
            warnings: vec![],
        }
    }
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(Box::new(details));
        self
    }
    pub fn with_detail(
        mut self,
        key: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        let details = self
            .details
            .get_or_insert_with(|| Box::new(serde_json::json!({})));
        if !details.is_object() {
            **details = serde_json::json!({});
        }
        details[key.into()] = value.into();
        self
    }
    pub fn with_report(mut self, report: impl Into<String>) -> Self {
        self.report = Some(report.into());
        self
    }
}
impl From<std::io::Error> for GlenError {
    fn from(e: std::io::Error) -> Self {
        Self::new(ErrorCode::Io, e.to_string())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Warning {
    pub code: String,
    pub message: String,
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_route_reasons_and_detail_builders_preserve_context() {
        for reason in NoRouteReason::ALL {
            assert_eq!(serde_json::to_value(reason).unwrap(), reason.as_str());
            let error = GlenError::no_route(reason, "refused")
                .with_detail("job", serde_json::json!({"handle":"j1234567"}))
                .with_detail("status", "failed")
                .with_hint("retry");
            assert_eq!(error.code, ErrorCode::NoRoute);
            assert_eq!(error.message, "refused");
            assert_eq!(error.hint.as_deref(), Some("retry"));
            let details = error.details.unwrap();
            assert_eq!(details["reason"], reason.as_str());
            assert_eq!(details["job"]["handle"], "j1234567");
            assert_eq!(details["status"], "failed");
        }
        for initial in [
            serde_json::Value::Null,
            serde_json::json!([]),
            serde_json::json!("old"),
        ] {
            let error = GlenError::new(ErrorCode::Io, "failed")
                .with_details(initial)
                .with_detail("action", "write")
                .with_detail("action", "flush");
            assert_eq!(
                *error.details.unwrap(),
                serde_json::json!({"action":"flush"})
            );
        }
    }
    #[test]
    fn mappings() {
        let table = [
            (ErrorCode::Internal, 1, "internal"),
            (ErrorCode::Io, 1, "io"),
            (ErrorCode::Usage, 2, "usage"),
            (ErrorCode::InvalidEnv, 2, "invalid_env"),
            (ErrorCode::NotFound, 3, "not_found"),
            (ErrorCode::Ambiguous, 4, "ambiguous"),
            (ErrorCode::OmpUnavailable, 5, "omp_unavailable"),
            (ErrorCode::NoRoute, 6, "no_route"),
            (ErrorCode::JobFailed, 7, "job_failed"),
            (ErrorCode::ControlFailed, 7, "control_failed"),
            (ErrorCode::NotImplemented, 8, "not_implemented"),
        ];
        assert_eq!(table.len(), ErrorCode::ALL.len());
        for (c, (expected, n, name)) in ErrorCode::ALL.into_iter().zip(table) {
            assert_eq!(c, expected);
            assert_eq!(c.exit_code(), n);
            assert_eq!(c.as_str(), name);
            assert_eq!(serde_json::to_string(&c).unwrap(), format!("\"{name}\""));
        }
        assert_eq!(OkExit::Done.exit_code(), 0);
        assert_eq!(OkExit::Running.exit_code(), 10);
        assert_eq!(OkExit::Forked.exit_code(), 11);
        assert_eq!(OkExit::Interrupted.exit_code(), 130);
    }
}
