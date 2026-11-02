//! The MCP layer (v0.2 W5, design §A22): a synchronous stdio JSON-RPC server that exposes glen's
//! visible commands as tools. Every tool call runs glen itself (`current_exe`, argv, `--json`,
//! never a shell), so every tool result is the CLI's own envelope. The tool catalog
//! (`tools.json`) is generated from the `<!-- mcp: … -->` annotations in `docs/json.md` plus clap.
pub mod catalog;
