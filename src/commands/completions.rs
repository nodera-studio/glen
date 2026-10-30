use crate::{
    cli::{self, CompletionsArgs},
    error::{ErrorCode, GlenError},
    output::Outcome,
};
pub fn run(args: &CompletionsArgs) -> Result<Outcome, GlenError> {
    let mut buf = Vec::new();
    clap_complete::generate(args.shell, &mut cli::public_command(), "glen", &mut buf);
    let script =
        String::from_utf8(buf).map_err(|e| GlenError::new(ErrorCode::Internal, e.to_string()))?;
    Ok(Outcome::new(
        serde_json::json!({"shell":args.shell.to_string(),"script":script}),
        script,
    ))
}
