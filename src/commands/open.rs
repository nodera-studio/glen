use crate::{
    cli::Cli,
    env::Env,
    error::{ErrorCode, GlenError},
    omp::{
        continuation, handle,
        lease::{self, Lease, State},
        listing::{self, Catalog, SessionFile, Summary},
        paths,
        session::{LoadError, Session, TurnStatus, in_flight_of_branch},
    },
    render::transcript,
};
use std::{
    fs::{File, Metadata},
    io::{Read, Seek, SeekFrom},
    path::Path,
};
pub(crate) struct Opened {
    pub session: Session,
    pub summary: Summary,
    pub handle: String,
    pub lease: Lease,
    pub state: State,
    pub continued_from: Vec<String>,
    pub catalog: Catalog,
    pub metadata: Metadata,
}
pub(crate) fn load_error(path: &Path, error: LoadError) -> GlenError {
    match error {
        LoadError::NotSession => GlenError::new(
            ErrorCode::NotFound,
            format!("not an omp session file: {}", path.display()),
        ),
        LoadError::Io(e) => GlenError::read(path, e),
    }
}
/// Metadata and bytes always come from the same descriptor, even across rename.
pub(crate) fn snapshot(
    path: &Path,
    file: &mut File,
    metadata: &Metadata,
    complete_only: bool,
) -> Result<Session, LoadError> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = vec![];
    file.take(metadata.len()).read_to_end(&mut bytes)?;
    if complete_only {
        let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        bytes.truncate(end);
    }
    Session::parse(path, &bytes)
}
pub(crate) fn load_snapshot(
    path: &Path,
    complete_only: bool,
) -> Result<(Session, Metadata), GlenError> {
    let mut file = File::open(path).map_err(|e| GlenError::read(path, e))?;
    let metadata = file.metadata().map_err(|e| GlenError::read(path, e))?;
    let session =
        snapshot(path, &mut file, &metadata, complete_only).map_err(|e| load_error(path, e))?;
    Ok((session, metadata))
}
pub(crate) fn open(cli: &Cli, env: &Env, arg: &str) -> Result<Opened, GlenError> {
    open_mode(cli, env, arg, false)
}
pub(crate) fn open_follow(cli: &Cli, env: &Env, arg: &str) -> Result<Opened, GlenError> {
    open_mode(cli, env, arg, true)
}
fn open_mode(cli: &Cli, env: &Env, arg: &str, complete_only: bool) -> Result<Opened, GlenError> {
    let paths = paths::resolve(env, cli.global.profile.as_deref())?;
    // Paths need no assigned handles during resolution. Include an external
    // session in the one assignment after loading it.
    let mut catalog = if arg.contains('/') || arg.ends_with(".jsonl") {
        Catalog::read(&paths.sessions_dir)?
    } else {
        Catalog::scan(&paths.sessions_dir)?
    };
    let path = handle::resolve(arg, &catalog, env)?;
    let followed = continuation::follow(&path)?;
    let (session, metadata) = load_snapshot(&followed.path, complete_only)?;
    let file = SessionFile::from_metadata(&followed.path, &metadata)
        .map_err(|e| GlenError::read(&followed.path, e))?;
    let mut summary = listing::summarize(file, &session);
    let branch = session.active_branch();
    summary.last_turn = TurnStatus::of_branch(&branch);
    summary.in_flight = in_flight_of_branch(&branch);
    summary.model = branch
        .iter()
        .rev()
        .filter(|e| e.kind == "message" && e.raw["message"]["role"] == "assistant")
        .find_map(|e| listing::model_of(&e.raw["message"]))
        .or_else(|| {
            branch
                .iter()
                .rev()
                .filter(|e| e.kind == "model_change")
                .find_map(|e| e.raw["model"].as_str().map(str::to_owned))
        });
    let lease = lease::probe(&paths.session_owners_dir, &session.header.id);
    let state = lease::infer_state(&lease, summary.in_flight);
    if !catalog.handles.contains_key(&session.header.id) {
        catalog.assign_handles(Some(&session.header.id));
    }
    let handle = catalog.handle(&session.header.id);
    Ok(Opened {
        session,
        summary,
        handle,
        lease,
        state,
        continued_from: followed.from,
        catalog,
        metadata,
    })
}
pub(crate) fn header<'a>(o: &'a Opened) -> transcript::Header<'a> {
    transcript::Header {
        title: o
            .summary
            .title
            .as_deref()
            .or(o.summary.first_message.as_deref()),
        handle: &o.handle,
        cwd: o.summary.cwd.as_deref(),
        model: o.summary.model.as_deref(),
        state: o.state,
        updated: o.summary.file.mtime,
        continued: &o.continued_from,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::omp::fixtures;
    use std::fs;
    #[test]
    fn snapshot_identity_and_bytes_share_descriptor_across_rename() {
        let tree = fixtures::tree();
        let path = fixtures::path(&tree, 'A');
        let mut fd = File::open(&path).unwrap();
        let metadata = fd.metadata().unwrap();
        let replacement = path.with_file_name("replacement.tmp");
        fs::write(&replacement, fixtures::bytes('G')).unwrap();
        fs::rename(replacement, &path).unwrap();
        let snapshot = snapshot(&path, &mut fd, &metadata, true).unwrap();
        assert!(snapshot.header.id.ends_with("e27c0de"));
        assert_eq!(snapshot.consumed, metadata.len());
        let (next, next_metadata) = load_snapshot(&path, true).unwrap();
        assert!(next.header.id.ends_with("b8e4a07"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_ne!(metadata.ino(), next_metadata.ino());
        }
    }
    #[test]
    fn missing_load_maps_to_not_found_with_path() {
        let tree = tempfile::tempdir().unwrap();
        let path = tree.path().join("missing.jsonl");
        let error = load_snapshot(&path, true).unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(error.message.contains(path.to_str().unwrap()));
        let denied = GlenError::read(&path, std::io::ErrorKind::PermissionDenied.into());
        assert_eq!(denied.code, ErrorCode::Io);
        assert!(denied.message.contains(path.to_str().unwrap()));
    }
}

#[cfg(test)]
mod assignment_tests {
    use super::*;
    use crate::omp::fixtures;
    use clap::Parser;
    use std::fs;
    #[test]
    fn show_assigns_once_for_handles_catalog_paths_and_external_paths() {
        let tree = tempfile::tempdir().unwrap();
        let agent = tree.path().join("agent");
        let bucket = agent.join("sessions/bucket");
        fs::create_dir_all(&bucket).unwrap();
        let inside = bucket.join("inside.jsonl");
        fs::write(&inside, fixtures::bytes('A')).unwrap();
        let outside = tree.path().join("outside.jsonl");
        let content = String::from_utf8(fixtures::bytes('A').to_vec())
            .unwrap()
            .replace(
                "01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",
                "00000000-0000-0000-0000-00001e27c0de",
            );
        fs::write(&outside, content).unwrap();
        let env = Env::for_test(
            Some(tree.path()),
            tree.path(),
            &[("PI_CODING_AGENT_DIR", agent.to_str().unwrap())],
        );
        for (arg, expected) in [
            ("e27c0de", "e27c0de"),
            (inside.to_str().unwrap(), "e27c0de"),
            (outside.to_str().unwrap(), "1e27c0de"),
        ] {
            let cli = Cli::try_parse_from(["glen", "show", arg]).unwrap();
            handle::ASSIGN_CALLS.with(|count| count.set(0));
            let opened = open(&cli, &env, arg).unwrap();
            assert_eq!(opened.handle, expected);
            handle::ASSIGN_CALLS.with(|count| assert_eq!(count.get(), 1));
        }
    }
}
