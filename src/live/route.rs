use super::{
    client::{self, ClientError, ConnectError, Hello},
    registry::{self, Registry},
};
use crate::glen_home::GlenHome;
use std::{io, time::Duration};
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
        /// The underlying I/O error, when this came from a socket connect or a
        /// registry-cleanup write; lets the caller distinguish a sandbox
        /// permission denial from a genuine endpoint problem.
        io: Option<io::Error>,
    },
}
#[derive(Clone, Copy, Debug)]
pub enum Unhealthy {
    Unreadable,
    PathTooLong,
    Unresponsive,
    Incompatible,
}
fn cleanup(home: &GlenHome, reg: &Registry, socket_too: bool) -> Option<Endpoint> {
    let (reason, io_err) = match registry::remove_if_unchanged(home, reg, socket_too) {
        Ok(true)
            if matches!(
                registry::read(home, &reg.session_id),
                registry::Read::Missing
            ) =>
        {
            return None;
        }
        Ok(_) => (
            "live registry changed during stale cleanup; retry delivery".into(),
            None,
        ),
        Err(e) => (e.message.clone(), Some(e.io)),
    };
    Some(Endpoint::Unhealthy {
        reason,
        kind: Unhealthy::Unreadable,
        io: io_err,
    })
}
pub fn find(home: &GlenHome, id: &str, timeout: Duration) -> Endpoint {
    let reg = match registry::read(home, id) {
        registry::Read::Missing => return Endpoint::None,
        registry::Read::Unreadable(reason) => {
            return Endpoint::Unhealthy {
                reason,
                kind: Unhealthy::Unreadable,
                io: None,
            };
        }
        registry::Read::Ok(r) => r,
    };
    if registry::pid_dead(reg.pid) {
        if let Some(e) = cleanup(home, &reg, true) {
            return e;
        }
        return Endpoint::Stale(format!("pid {} is gone", reg.pid));
    }
    if !super::socket_path_ok(&reg.socket) {
        return Endpoint::Unhealthy {
            reason: "socket path too long".into(),
            kind: Unhealthy::PathTooLong,
            io: None,
        };
    }
    match client::hello(&reg.socket, timeout) {
        Ok(hello) if hello.session_id != id => {
            if let Some(e) = cleanup(home, &reg, false) {
                return e;
            }
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
            io: None,
        },
        Err(ClientError::Connect(ConnectError::Missing | ConnectError::Refused)) => {
            if let Some(e) = cleanup(home, &reg, false) {
                return e;
            }
            Endpoint::Stale("socket went away".into())
        }
        Err(e) => {
            let reason = e.to_string();
            let kind = if matches!(e, ClientError::Refused(_)) {
                Unhealthy::Incompatible
            } else {
                Unhealthy::Unresponsive
            };
            let io = match e {
                ClientError::Connect(ConnectError::Io(io_err)) => Some(io_err),
                _ => None,
            };
            Endpoint::Unhealthy { reason, kind, io }
        }
    }
}
pub fn hint(kind: Unhealthy) -> String {
    if matches!(kind, Unhealthy::PathTooLong) {
        "shorten GLEN_HOME, run `glen install-extension`, then restart that omp session".into()
    } else {
        "run `glen install-extension`, then restart that omp session (only TUI sessions accept glen messages)".into()
    }
}

pub fn lease_held_hint(paths: &crate::omp::paths::OmpPaths) -> String {
    if paths.extensions_dir.join("glen/index.ts").is_file() {
        "restart that omp session so it loads the glen extension (only TUI sessions accept glen messages)".into()
    } else {
        "run `glen install-extension`, then restart that omp session".into()
    }
}
