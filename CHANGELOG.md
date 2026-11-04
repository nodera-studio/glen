# Changelog

All notable changes to glen are listed here, newest first.

## v0.2.0 — 2026-11-04

### Added

- `glen doctor --probe` starts omp briefly, with no session and no prompt, and checks its RPC capabilities: the negotiated protocol, host tools and the event filter. They appear in a new `omp_capabilities` row and in `data.omp.capabilities`. Only a missing host-tool capability fails doctor; an omp that cannot start (for example with no model selected) is a warning. RPC jobs also record the negotiated protocol as `rpc_protocol` in their `status.json`.
- The nightly `omp-latest` workflow runs glen's omp surface contract and source facts against the latest omp release, with a scratch home and no credentials, and reports a bump candidate. It never gates a pull request or a release.
- `glen wait <job> --for reply [--after <reply-id>]` returns within a second of the omp model calling `glen_reply`: exit 10 with `data.woke` set to `reply`, the replies you have not seen yet, and a `data.next` command that skips them. If the job ends first it returns the job's usual exit (0, 6, 7 or 11) with those replies; an unknown `--after` id is exit 2.
- `glen inbox --wait [--timeout DUR]` waits (90 s by default, at most 24 h) for one unread message: exit 0 with the messages, or exit 10 with none and a `next` command. It honours `--session`, `--cwd` and `--peek`, and marks messages read only after printing them.
- Job views in `send`, `wait`, `jobs` and job error details carry `task_state`, the job's state in A2A's vocabulary (`submitted`, `working`, `completed`, `canceled`, `rejected`, `failed`). `status` is unchanged and still decides the exit code.
- The agent skill has a "Replies without polling" section: Claude Code starts `glen wait <job> --for reply` as a background command and is woken when it finishes; a subagent waits in the foreground or hands the job to its parent; Codex runs a blocking wait and peeks the inbox at the start of each turn. Codex: blocking wait, not push.
- `glen send --warm DUR` (or `GLEN_WARM`, at most 10m) keeps a settled job's omp process alive so a following send to the same session skips omp's startup; the worker holds the session, and omp's own owner lease, until the window ends or `glen release <handle>` ends it early, so the omp TUI cannot open that session meanwhile. A queued job whose binary, profile, thinking level, cwd or environment digest differs is never adopted: the worker cools and it runs cold instead.
- `glen release [<target>] [--all] [--force] [--timeout DUR]` ends warm workers: a target is a session id, a hex tail or a job handle, and `--all` ends every one. `--force` kills a worker that does not release in time, only after proving its pid and start time (and omp's own, when recorded) are unchanged.
- `glen mcp` serves glen's commands to an MCP host such as Claude Code or Codex over stdio (`claude mcp add glen -- glen mcp`, `codex mcp add glen -- glen mcp`): 14 tools (`glen_ls`, `glen_send`, `glen_wait`, and the rest), each a byte-for-byte copy of the matching command's `--json` envelope. `isError` mirrors `ok:false`, so a still-running, forked or SIGINT-interrupted result (exits 10, 11, 130) is not an error. Cancelling a tool call (`notifications/cancelled`) stops the exec-self child but never aborts the job, which keeps running. Codex's default `tool_timeout_sec` (60 s) is shorter than `send`'s and `wait`'s 90 s defaults; set it higher (120 in the `[mcp_servers.glen]` example) or pass a shorter `wait`/`timeout`.
- `src/mcp/tools.json`, the generated MCP tool catalog, is checked in and generated from `docs/json.md`'s `<!-- mcp: … -->` annotations plus clap; `cargo test --locked --test mcp_contract` fails on any drift between the catalog and either source, naming the regenerate command.
- Warnings `session_identity_unverified` (send, wait: omp did not confirm the session id after the turn, so a fork may be unreported; the worker does not stay warm) and `replies_truncated` (send, wait: a live job kept only its first 256 replies or 8 MiB of reply text; later replies are still in `glen inbox`).

### Fixed

- `glen watch`'s terminal teardown no longer races the last frame: on SIGINT, SIGTERM, SIGHUP and a panic, the terminal is restored before any further draw, so a stray frame can no longer be written after the terminal has already been handed back.
- `glen release --all` no longer stops at the first warm worker it fails to release: every target is still attempted, and a failure's message and `details.workers` report every worker's outcome, ending with `(k of n warm workers released)`.
- An unreadable warm-worker registry file no longer lets `glen release --all` report `total:0` or succeed silently; it now exits 1 `io` with `details.unreadable`, after every readable worker has still been processed.
- `doctor --probe` and `glen models`'s throwaway omp process now run in a private, empty directory with `--no-extensions --no-skills --no-rules`, instead of the caller's own directory with whatever extensions, skills and rules it carries; a side effect is that `glen models` can no longer list a provider that only an extension had registered.
- A job's final identity check (`get_state`) failing silently no longer lets a fork go unreported as if the session were confirmed: a failed check is now retried once within the job's deadline and warned on.
- Live `glen_reply` frames are now validated and capped by count and byte size before they are persisted, instead of trusting whatever the omp extension sent.
- On Linux, a live TUI's registry and socket are no longer deleted just because its pid is invisible from glen's own process view (for example inside a pid namespace); under a detected Codex sandbox marker the lease now reports as unknown instead.
- Warm-worker registry file names must now equal the session's own lock key, closing a path where `glen release --force` could be pointed at a forged identity.

### Security

- Every workflow in `.github/workflows/` pins its actions, including GitHub's own `actions/*`, to full commit SHAs and declares a read-only top-level `permissions:`, both enforced by a workflow-contract test.
- CI's `site-deploy` job now runs `npm ci --ignore-scripts`.
- `install.sh`'s `GLEN_INSTALL_BASE_URL` now accepts only an `https://` mirror or a `file://` directory (for fixtures); any other scheme is a usage error (exit 2) before any download is attempted.
- Untrusted text passed through `render::sanitize` and the JSON escaper now also escapes tag characters, zero-width characters and U+2028; session-id-derived strings reaching human output (`release`, `jobs`, `doctor`) are sanitized the same way as everywhere else.
- The Claude Code skill's `allowed-tools` now pre-approves only `glen ls`, `show`, `tail`, `jobs`, `wait`, `inbox`, `doctor` and `send`, not every `glen` command: the others (`abort`, `models`, `model`, `thinking`, `compact`, `release`, the installers and the rest) ask first. glen still starts omp under omp's own default approval mode, so a pre-approved `send` can start an omp agent whose tools run without asking (`docs/design.md` §A13). Run `glen install-skill` again after upgrading to get the narrower list.

### Changed

- The agent skill no longer tells agents to poll `glen inbox`: every inbox instruction uses `--wait`, `--peek` or `-f`, and the loop points at the new receivers.
- Exit 10 now has two documents under `glen wait --for reply`: a reply wake (`data.woke` is `reply`) and a bound expiry (no `woke`). Read `woke` before treating exit 10 as a timeout. Exit 10 without `--for reply` is unchanged.
- `glen doctor` judges omp against a supported range declared in `src/omp/compat.json` and reports it in `data.omp.supported`. A version older than the minimum warns with the new code `omp_version_unsupported`; one newer than the newest tested version still warns `omp_version_untested`. `omp --version` output that cannot be parsed is now a warning (`omp_version_untested`, row value `unprobed`), not exit 5; an `omp --version` that fails or times out is still exit 5 `omp_unavailable`.
- The supported omp range is 18.14.0 to 18.16.0. glen is still built and tested against omp 18.14.0 (the `omp-rpc` pin); 18.16.0 is the newest release the nightly `omp-latest` checks passed on.
- Job views in `send`, `wait`, `jobs` and job error details gain `omp_reused`, true only for a job that ran on an omp process a warm worker kept from an earlier job.
- `glen doctor` gains a `warm_workers` check, right after `live_endpoints`: how many warm-worker registries are fresh, stale (no heartbeat) or dead; it warns, never fails, naming the fix. `glen ls` shows warmth in `state_detail` and with a ` (warm)`/` (warm, stale)` suffix on the human state cell.
- The agent skill gains one guarded bullet (shown once `glen release` is visible) telling agents to use `--warm` only when another send is coming soon, and to end it with `glen release`.
- The agent skill gains two guarded lines (shown once `glen mcp` is visible) telling agents an MCP host can register `glen mcp` and call glen as tools instead of Bash, and that a tool call keeps the 90 s `send`/`wait` defaults.

### Documentation

- `docs/design.md` describes v0.2.0. New sections cover omp compatibility (§A19), replies without a polling loop (§A20), the warm worker (§A21), the MCP layer (§A22) and the ACP spike (§A23). Job states also get their A2A names (§A7). (This described the v0.2 W0 foundation wave, which added only docs and tests; the CLI itself changed substantially across the waves below.)
- §A20 describes push to the calling agent as shipped in v0.2 W2: the two receivers, the unseen-reply rule, why `wait` leaves the inbox marker alone, the two exit-10 documents and the per-caller delivery table. A background wait also wakes a headless `claude -p` run: verified on Claude Code 2.1.316 in stream-json mode, where the completion notification resumed the run after its first result. The skill's allowed-tools may not apply to that resumed turn under `--permission-mode dontAsk`.
- §A21 describes the warm worker as shipped in v0.2 W4: the stay-warm rule, the on-disk registry and release request, the match key and the compare-and-claim protocol, the idle loop and its bounds, the one teardown and its reasons, `glen release`'s outcomes, controls and `ls`/doctor wording on a warm session, the queued-job liveness decision, the lease cost and two residual races.
- §A22 describes the MCP layer as shipped in v0.2 W5: the supported protocol revisions and why (`initialize`-era only, no batching), the annotation grammar and the generated catalog, naming and schema rules, the exec-self argv and result-mapping rules, cancellation and shutdown, the limits, Codex's and Claude Code's host-specific quirks, and the security notes.
- §A23 records the `omp acp` (ACP v1) spike, its experiment results and the verdict. The spike ran through a hidden command, `__acp`, which is not part of glen's interface and may change or go away.

### Site

- The landing page shows what v0.2 ships: the Hear back card shows `glen wait --for reply` and `glen inbox --wait`, the Send card `--warm` and `glen release`, the Agents section gains an MCP tab with the `claude mcp add` and `codex mcp add` commands, the exit table splits exit 10 into running and woke, and the FAQ covers replies without polling, `--warm` and the supported omp range.
- The Agents tabs say push only for Claude Code, which a background wait wakes; Codex gets replies through a blocking wait.
- The hero chip and the FAQ read the supported omp range from `src/omp/compat.json`, and a unit test keeps the site's version equal to `Cargo.toml`'s.

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
