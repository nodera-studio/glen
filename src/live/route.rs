use super::{
    client::{self, ClientError, ConnectError, Hello},
    registry::{self, Registry},
};
use crate::glen_home::GlenHome;
use std::time::Duration;
#[derive(Debug)]
pub enum Endpoint {
    None,
    Stale(String),
    Live {
        reg: Box<Registry>,
        hello: Box<Hello>,
    },
    Unhealthy {
        reason: String,
        kind: Unhealthy,
    },
}
#[derive(Clone, Copy, Debug)]
pub enum Unhealthy {
    Unreadable,
    PathTooLong,
    Unresponsive,
    Incompatible,
}
pub fn find(home: &GlenHome, id: &str, timeout: Duration) -> Endpoint {
    let reg = match registry::read(home, id) {
        registry::Read::Missing => return Endpoint::None,
        registry::Read::Unreadable(reason) => {
            return Endpoint::Unhealthy {
                reason,
                kind: Unhealthy::Unreadable,
            };
        }
        registry::Read::Ok(r) => r,
    };
    if registry::pid_dead(reg.pid) {
        registry::remove_if_unchanged(home, &reg, true);
        return Endpoint::Stale(format!("pid {} is gone", reg.pid));
    }
    if !super::socket_path_ok(&reg.socket) {
        return Endpoint::Unhealthy {
            reason: "socket path too long".into(),
            kind: Unhealthy::PathTooLong,
        };
    }
    match client::hello(&reg.socket, timeout) {
        Ok(hello) if hello.session_id != id => {
            registry::remove_if_unchanged(home, &reg, false);
            Endpoint::Stale(format!("now on {}", hello.session_id))
        }
        Ok(hello) if hello.protocol == 1 && hello.ops.iter().any(|s| s == "send") => {
            Endpoint::Live {
                reg,
                hello: Box::new(hello),
            }
        }
        Ok(_) => Endpoint::Unhealthy {
            reason: "endpoint does not support protocol 1 send".into(),
            kind: Unhealthy::Incompatible,
        },
        Err(ClientError::Connect(ConnectError::Missing | ConnectError::Refused)) => {
            registry::remove_if_unchanged(home, &reg, false);
            Endpoint::Stale("socket went away".into())
        }
        Err(e) => Endpoint::Unhealthy {
            kind: if matches!(e, ClientError::Refused(_)) {
                Unhealthy::Incompatible
            } else {
                Unhealthy::Unresponsive
            },
            reason: e.to_string(),
        },
    }
}
pub fn hint(kind: Unhealthy) -> String {
    if matches!(kind, Unhealthy::PathTooLong) {
        "shorten GLEN_HOME, run `glen install-extension`, then restart that omp session".into()
    } else {
        "run `glen install-extension`, then restart that omp session (only TUI sessions accept glen messages)".into()
    }
}
