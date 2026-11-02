//! The MCP tool catalog stays generated (v0.2 W5, AC-v2-501 and AC-v2-506; design §A22):
//! `src/mcp/tools.json` equals `catalog::generate` over `docs/json.md` and clap, byte for byte.
use glen::{cli::public_command, mcp::catalog};
use std::path::PathBuf;

const REGENERATE: &str = "cargo test --locked --test mcp_contract regenerate_catalog -- --ignored";
const JSON_MD: &str = include_str!("../docs/json.md");

fn catalog_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/mcp/tools.json")
}
fn generated() -> String {
    catalog::generate(JSON_MD, &public_command())
        .unwrap_or_else(|e| panic!("cannot generate the MCP catalog: {e}"))
}

#[test]
fn catalog_is_current() {
    let want = generated();
    let on_disk = std::fs::read_to_string(catalog_path()).unwrap();
    assert!(
        on_disk == want,
        "src/mcp/tools.json is stale against docs/json.md and clap; run `{REGENERATE}`"
    );
    assert!(
        catalog::TOOLS_JSON == want,
        "the embedded catalog is stale (rebuild after `{REGENERATE}`)"
    );
    assert_eq!(generated(), want, "generation is deterministic");
}

/// Rewrites `src/mcp/tools.json`; run it with `-- --ignored` after an annotation or clap edit.
#[test]
#[ignore = "writes src/mcp/tools.json"]
fn regenerate_catalog() {
    std::fs::write(catalog_path(), generated()).unwrap();
}
