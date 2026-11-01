# Changelog

All notable changes to glen are listed here, newest first.

## v0.1.1 — 2026-11-01

### Fixed

- `glen tail -f` no longer prints the whole transcript again when omp 18.14.0 migrates a v1 session file to the current format. omp gives every entry a new id when it migrates, so glen now matches the entries it already printed by position, kind, timestamp, role and content.
- On a slow or loaded machine, an answer from the omp extension that arrived in time could still count as a timeout. `show`, `tail` and `ls` then reported a live session's state from disk, and `send` refused the live endpoint. Connecting to the endpoint and reading its answer now run on glen's own thread with one deadline, and session-control requests stop cleanly when cancelled.

### Site

- The site shows the released state: the install script first, the latest release under Binary, and a bare version in the header, hero and footer.
- The closing call-to-action offers the install script, the same command the Install section opens on.

## v0.1.0 — 2026-10-31

First release. glen is a Rust CLI that lets Claude Code and Codex discover, read, send to and receive replies from [omp](https://github.com/can1357/oh-my-pi) (oh-my-pi) sessions.

### Added

#### Discovery and reading

- `glen ls` lists omp sessions in the current project. `--all` covers every project, `--cwd DIR` picks one, `--live` shows open sessions only, `--since DUR` filters by age and `-n` sets the row limit (default 20).
- `glen show <session>` prints a transcript (active branch). `--tools collapsed|full|none`, `--thinking` and `--raw` control the detail.
- `glen tail <session>` prints the latest items (`-n`, default 10). `-f` follows new items until interrupted.
- A session handle is a short suffix of its id, normally the last 7 hex digits. `@latest` picks the newest session in the project, and `glen send @new` starts one.
- glen reads omp's session files leniently and never writes to them. Unknown entry types are skipped.
- `--profile NAME` picks an omp profile on any command (it overrides `OMP_PROFILE` and `PI_PROFILE`).

#### Sending and replies

- `glen send <session> <message>` sends a message and waits for the reply (default 90 s). `--steer` or `--follow-up` (or `--as auto|steer|follow-up`) chooses how it is delivered. `--wait DUR` changes the wait and `--no-wait` returns a job handle at once. A message of `-` reads stdin.
- `glen send @new <message> --cwd DIR` starts a new session in that directory.
- `glen send --thinking <level>` sets omp's startup reasoning effort (off, minimal, low, medium, high, xhigh, max or auto) when glen starts omp for the send. A session already open in the omp TUI ignores it with an `ignored_flag` warning; use `glen thinking` there.
- `glen wait <job>` keeps waiting for a job, and `glen jobs` lists jobs (`--active`, `--session`, `-n`). A job keeps running after glen stops waiting.
- Replies from the omp model come back through a `glen_reply` tool. They appear in `data.replies` and in `glen inbox`.
- `--cwd DIR` scopes session resolution on `show`, `tail`, `send` and `inbox`.

#### Live sessions: extension and inbox

- `glen install-extension` installs a bundled omp extension (one file under the omp agent dir). Restart the omp TUI afterwards.
- With the extension, a session open in the omp TUI is reachable over its live endpoint: `send` and the session controls reach it without forking the session. Such a session shows up as `live` in `ls`, and in `show`, `tail` and `send`, even before omp has written its transcript.
- `glen install-extension` supports `--dry-run`, `--uninstall` and `--force`. It will not overwrite a newer, edited or foreign file without `--force`.
- Each request gets its own answer. Requests omp answers together share one and carry a `batched` warning.
- `glen inbox` shows replies and messages omp sent back (`--peek`, `--all`, `--session`, `--cwd`, `-f`). Messages are marked read only after the output is written.

#### Session controls

- `glen abort <session>` stops the running turn (`glen stop` is an alias).
- `glen models [session]` lists the models a session can use, or the profile's models without a session (`--provider`). `glen model <session> <provider/id>` switches the model.
- `glen thinking <session> <level>` sets the level: off, minimal, low, medium, high, xhigh or max, where the model supports it.
- `glen compact <session>` compacts the context (`--instructions TEXT`). A busy session is refused.
- All controls take `--timeout DUR` (default 90 s). If omp goes away after glen submitted a change, the control fails with reason `outcome_unknown`: the change may have been applied.

#### `glen watch`

- `glen watch [session]` is a full-screen terminal view with a session list, the selected transcript, job activity, replies and a prompt. `--all` and `--cwd DIR` set the scope.
- It sends through the same path as `glen send`, with auto, steer and follow-up delivery (Ctrl-T cycles it).
- Model, thinking, compact and abort are available from the keyboard. Press `?` or F1 for help.
- It supports light and dark themes (`GLEN_THEME`), `NO_COLOR` and `--color`. It needs an interactive terminal and exits 2 without one.

#### Agent skill and JSON contract

- `glen install-skill` installs the glen skill for Claude Code and Codex. `--claude` or `--codex` picks one, `--project` writes into the current directory, and `--dry-run`, `--force` and `--uninstall` are available.
- With `--json`, or when stdout is not a terminal, every command prints one v1 envelope: `{"v":1,"ok":true,"data":…}` or `{"v":1,"ok":false,"error":{…}}`, with optional `warnings`. `tail -f` and `inbox -f` stream one JSON object per line. Large output spills to a file and the envelope names the path.
- Exit codes tell an agent what to do next: 0 done, 1 internal or I/O error, 2 usage or environment, 3 not found, 4 ambiguous, 5 omp unavailable, 6 refused (`no_route`, with a typed `details.reason`), 7 job or control failed, 10 still running, 11 session forked, 130 interrupted. The full contract is in `docs/json.md`.
- `glen doctor` checks omp, its version, paths, `GLEN_HOME`, the installed skill and extension, and the shell sandbox.
- `glen completions <bash|elvish|fish|powershell|zsh>` generates shell completions.
- Text glen did not write is sanitised. Human output shows control and bidi characters as escapes. JSON keeps the original value, but escapes DEL, C1 and bidi controls as `\uXXXX`, so a terminal showing raw JSON never acts on them.
- `install-extension` and `install-skill` replace files with an atomic swap. glen refuses a `GLEN_HOME` (or a directory under it) that is a symlink, belongs to another user, or is group- or world-writable (exit 2, `invalid_env`).

#### Install and platforms

- `install.sh` picks the archive for your platform, checks its SHA-256 and installs `glen` to `~/.local/bin`. It never uses sudo. Flags: `--version`, `--prefix`, `--dry-run`.
- Release archives for macOS Apple silicon and Intel, and Linux x86_64 and arm64 (musl, static). Each holds the binary, `LICENSE` and `THIRD-PARTY-LICENSES`, and comes with a `.sha256` file, plus `SHA256SUMS`.
- `cargo install --git https://github.com/nodera-studio/glen --locked` builds from source (Rust 1.89 or newer).

### Notes

- glen targets omp 18.14.0. omp's RPC and session formats change between releases, so other versions are untested, and `glen doctor` warns on them.
- On Linux, the Codex sandbox hides session leases. Resuming an existing session over RPC is refused with exit 6 (`no_route`, reason `sandboxed`, `lease:"unknown"`), and `ls` shows existing sessions as `unknown`. The live route (a session open in the omp TUI with the glen extension), `@new`, and running glen outside the sandbox still work.
- The binaries are not signed or notarised. A browser download on macOS needs `xattr -d com.apple.quarantine ./glen` once. `install.sh` is not affected.
- `glen watch` updates per saved transcript entry, not per token. It does not handle suspend (SIGTSTP); run `reset` if a forced stop leaves the terminal in a bad state.
