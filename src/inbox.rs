use crate::{
    glen_home::{GlenHome, write_private_atomic},
    jobs::Reply,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Cursor {
    pub offset: u64,
    pub ino: u64,
}
pub fn cursor(home: &GlenHome, stem: &str) -> Cursor {
    fs::read(home.root.join("inbox/.read").join(stem))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}
pub fn sessions(home: &GlenHome, filter: Option<&str>) -> Vec<(String, PathBuf)> {
    let mut files = fs::read_dir(home.root.join("inbox"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_owned();
            if name.starts_with('.') {
                return None;
            }
            let stem = name.strip_suffix(".ndjson")?;
            if let Some(f) = filter {
                let f = f.to_ascii_lowercase();
                if !stem.eq_ignore_ascii_case(&f)
                    && !(f.len() >= 4
                        && f.bytes().all(|b| b.is_ascii_hexdigit())
                        && stem.replace('-', "").to_ascii_lowercase().ends_with(&f))
                {
                    return None;
                }
            }
            Some((stem.to_owned(), e.path()))
        })
        .collect::<Vec<_>>();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}
pub struct Batch {
    pub records: Vec<(Reply, u64)>,
    pub end: u64,
    pub skipped: usize,
    pub ino: u64,
}
pub fn read_new(path: &Path, from: Cursor) -> io::Result<Batch> {
    let mut file = File::open(path)?;
    let meta = file.metadata()?;
    let start = if from.ino != meta.ino() || from.offset > meta.len() {
        0
    } else {
        from.offset
    };
    file.seek(SeekFrom::Start(start))?;
    let mut reader = BufReader::new(file.take(meta.len().saturating_sub(start)));
    let mut b = Batch {
        records: vec![],
        end: start,
        skipped: 0,
        ino: meta.ino(),
    };
    loop {
        let mut line = vec![];
        let n = reader.read_until(b'\n', &mut line)?;
        if n == 0 || !line.ends_with(b"\n") {
            break;
        }
        b.end += n as u64;
        match serde_json::from_slice::<Reply>(&line) {
            Ok(r) => b.records.push((r, b.end)),
            Err(_) => b.skipped += 1,
        }
    }
    Ok(b)
}
/// Checks, before any output, that `mark` will be able to write a cursor (W8-ERR-1). When
/// `inbox/.read` exists it gets `mark`'s own `ensure_dir`; when it does not, the probe goes in
/// `inbox/`, where `mark` would create it, so a call that later fails to print leaves no
/// `.read` behind. The probe file starts with `.` so `sessions` never lists it.
pub fn probe_writable(home: &GlenHome) -> io::Result<()> {
    let dir = if fs::symlink_metadata(home.root.join("inbox/.read")).is_ok() {
        home.ensure_dir("inbox/.read")?
    } else {
        home.ensure_dir("inbox")?
    };
    let probe = dir.join(format!(".probe.{}.tmp", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    drop(options.open(&probe)?);
    fs::remove_file(&probe)
}
pub fn mark(home: &GlenHome, stem: &str, cursor: Cursor) -> io::Result<()> {
    let dir = home.ensure_dir("inbox/.read")?;
    let b =
        serde_json::to_vec(&serde_json::json!({"v":1,"offset":cursor.offset,"ino":cursor.ino}))?;
    write_private_atomic(&dir.join(stem), &b)
}
