use serde::Serialize;

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
    NotImplemented,
}
impl ErrorCode {
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
            Self::JobFailed => 7,
            Self::NotImplemented => 8,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum OkExit {
    Done,
    Running,
    Forked,
}
impl OkExit {
    pub fn exit_code(self) -> u8 {
        match self {
            Self::Done => 0,
            Self::Running => 10,
            Self::Forked => 11,
        }
    }
}
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct GlenError {
    pub code: ErrorCode,
    pub message: String,
    pub hint: Option<String>,
    pub details: Option<serde_json::Value>,
    pub report: Option<String>,
}
impl GlenError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
            details: None,
            report: None,
        }
    }
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
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
impl From<anyhow::Error> for GlenError {
    fn from(e: anyhow::Error) -> Self {
        Self::new(ErrorCode::Internal, format!("{e:#}"))
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Warning {
    pub code: String,
    pub message: String,
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mappings() {
        for (c, n, name) in [
            (ErrorCode::Internal, 1, "internal"),
            (ErrorCode::Io, 1, "io"),
            (ErrorCode::Usage, 2, "usage"),
            (ErrorCode::InvalidEnv, 2, "invalid_env"),
            (ErrorCode::NotFound, 3, "not_found"),
            (ErrorCode::Ambiguous, 4, "ambiguous"),
            (ErrorCode::OmpUnavailable, 5, "omp_unavailable"),
            (ErrorCode::NoRoute, 6, "no_route"),
            (ErrorCode::JobFailed, 7, "job_failed"),
            (ErrorCode::NotImplemented, 8, "not_implemented"),
        ] {
            assert_eq!(c.exit_code(), n);
            assert_eq!(c.as_str(), name);
            assert_eq!(serde_json::to_string(&c).unwrap(), format!("\"{name}\""));
        }
        assert_eq!(OkExit::Done.exit_code(), 0);
        assert_eq!(OkExit::Running.exit_code(), 10);
        assert_eq!(OkExit::Forked.exit_code(), 11);
    }
}
