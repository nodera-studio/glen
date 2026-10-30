pub mod client;
pub(crate) mod drive;
pub mod registry;
pub mod route;
use crate::glen_home::GlenHome;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
pub const SUN_PATH_MAX: usize = if cfg!(target_os = "macos") { 104 } else { 108 };
pub fn socket_path_ok(p: &Path) -> bool {
    p.as_os_str().len() < SUN_PATH_MAX
}
pub fn run_dir(home: &GlenHome) -> PathBuf {
    home.root.join("run/live")
}
pub const HELLO_SEND: Duration = Duration::from_secs(2);
pub const HELLO_PROBE: Duration = Duration::from_secs(1);
pub const STATE_LS: Duration = Duration::from_millis(300);
pub const WRITE: Duration = Duration::from_secs(2);
pub const ACK: Duration = Duration::from_secs(10);
pub const MAX_FRAME: usize = 16 * 1024 * 1024;
