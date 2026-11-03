//! Process and timing helpers that hold under `scripts/flake-check.sh`, which repeats the suite
//! beside CPU burners. Shared by the `common` tests and the conformance suite.
#![allow(dead_code)]
use std::{sync::OnceLock, time::Duration};

/// `GLEN_TEST_TIME_SCALE`, a whole number from 1 that `scripts/flake-check.sh` exports for its
/// loaded runs; 1 when unset.
pub fn time_scale() -> u32 {
    static SCALE: OnceLock<u32> = OnceLock::new();
    *SCALE.get_or_init(|| match std::env::var("GLEN_TEST_TIME_SCALE") {
        Err(std::env::VarError::NotPresent) => 1,
        value => value
            .as_deref()
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|n| *n >= 1)
            .unwrap_or_else(|| {
                panic!("GLEN_TEST_TIME_SCALE must be a whole number from 1: {value:?}")
            }),
    })
}

/// A test's wait, or its upper bound on how long the product takes, times [`time_scale`]: exactly
/// `d` in a normal run, so no product bound is ever widened there. A lower bound is never scaled.
pub fn scaled(d: Duration) -> Duration {
    d * time_scale()
}

/// The live direct children of `parent` (zombies left out), read from the kernel: no `/bin/ps`,
/// whose spawn per poll is slow under load.
#[cfg(target_os = "linux")]
pub fn children_of(parent: u32) -> Vec<u32> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return vec![];
    };
    dir.filter_map(|entry| {
        let pid: u32 = entry.ok()?.file_name().to_str()?.parse().ok()?;
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // `pid (comm) state ppid …`: comm may hold spaces and parentheses.
        let mut fields = stat.rsplit_once(')')?.1.split_whitespace();
        let (state, ppid) = (fields.next()?, fields.next()?.parse::<u32>().ok()?);
        (ppid == parent && !matches!(state, "Z" | "X")).then_some(pid)
    })
    .collect()
}

/// The live direct children of `parent` (zombies left out), read from the kernel: no `/bin/ps`,
/// whose spawn per poll is slow under load.
#[cfg(target_os = "macos")]
pub fn children_of(parent: u32) -> Vec<u32> {
    let mut pids = [0 as libc::pid_t; 1024];
    let bytes = libc::c_int::try_from(std::mem::size_of_val(&pids)).unwrap();
    // SAFETY: `pids` is writable storage of `bytes` bytes, the most the call writes; it returns
    // the number of pids written (zombies included).
    let n =
        unsafe { libc::proc_listchildpids(parent as libc::pid_t, pids.as_mut_ptr().cast(), bytes) };
    pids[..usize::try_from(n).unwrap_or(0).min(pids.len())]
        .iter()
        .filter(|&&pid| live(pid))
        .map(|&pid| pid as u32)
        .collect()
}

/// Whether `pid` runs: it exists and is not a zombie.
#[cfg(target_os = "macos")]
fn live(pid: libc::pid_t) -> bool {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).unwrap();
    // SAFETY: `info` is writable storage of `size` bytes, the most the call writes.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    // SAFETY: a return of `size` means the call filled the whole structure.
    n == size && unsafe { info.assume_init() }.pbi_status != libc::SZOMB
}
