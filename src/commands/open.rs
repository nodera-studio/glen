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
use std::path::Path;
pub(crate) struct Opened {
    pub session: Session,
    pub summary: Summary,
    pub handle: String,
    pub lease: Lease,
    pub state: State,
    pub continued_from: Vec<String>,
    pub catalog: Catalog,
}
pub(crate) fn load(path: &Path) -> Result<Session, GlenError> {
    Session::load(path).map_err(|e| {
        GlenError::new(
            if matches!(e, LoadError::NotSession) {
                ErrorCode::NotFound
            } else {
                ErrorCode::Io
            },
            format!("cannot read session {}: {e}", path.display()),
        )
    })
}
pub(crate) fn open(cli: &Cli, env: &Env, arg: &str) -> Result<Opened, GlenError> {
    let paths = paths::resolve(env, cli.global.profile.as_deref())?;
    let catalog = Catalog::scan(&paths.sessions_dir)?;
    let path = handle::resolve(arg, &catalog, env)?;
    let followed = continuation::follow(&path)?;
    let session = load(&followed.path)?;
    let mut summary = listing::scan_head(SessionFile::at(&followed.path)?).ok_or_else(|| {
        GlenError::new(
            ErrorCode::NotFound,
            format!("not an omp session file: {}", followed.path.display()),
        )
    })?;
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
    let handles = handle::assign(
        catalog
            .sessions
            .iter()
            .map(|s| s.id.as_str())
            .chain([session.header.id.as_str()]),
    );
    let handle = handles
        .get(&session.header.id)
        .cloned()
        .unwrap_or_else(|| session.header.id.clone());
    Ok(Opened {
        session,
        summary,
        handle,
        lease,
        state,
        continued_from: followed.from,
        catalog,
    })
}
pub(crate) fn header<'a>(o: &'a Opened) -> transcript::Header<'a> {
    transcript::Header {
        title: o.summary.title.as_deref(),
        handle: &o.handle,
        cwd: o.summary.cwd.as_deref(),
        model: o.summary.model.as_deref(),
        state: o.state,
        updated: o.summary.file.mtime,
        continued: &o.continued_from,
    }
}
