#!/usr/bin/env bash
set -euo pipefail

# Decode JSON before the bash-only fast check, so JSON escapes cannot hide a name.
# No submitted text is ever executed. jq is already a harness dependency.
command_text="$(jq -er '.tool_input.command // "" | if type == "string" then . else error("command must be text") end')" || {
  printf '%s\n' 'Cannot decode the omp profile guard input; refusing the command.' >&2
  exit 2
}
# A cheap over-approximation also catches joined spellings such as o"m"p.
# Removing quote/escape characters never evaluates text; false matches merely
# go to Python. Dollar signs/backticks always survive and force full inspection.
fast_text="${command_text//$'\\\n'/}"
fast_text="${fast_text//\"/}"
fast_text="${fast_text//\'/}"
fast_text="${fast_text//\\/}"
case "$fast_text" in
  *[Oo][Mm][Pp]* | *[Gg][Ll][Ee][Nn]* | *[Xx][Aa][Rr][Gg][Ss]* | *'$'* | *'`'*) ;;
  *) exit 0 ;;
esac

# fd 3 carries command text; stdin carries isolated Python source. Capture a
# completion marker as well as status: missing Python, CLT placeholders (even a
# stub returning zero), and parser crashes must never silently allow a command.
parser_status=0
parse_command() {
python3 -I - 3<<< "$command_text" <<'PY'
import os
import re
import shlex
import sys

MESSAGE = "This would write the owner's default omp profile (~/.omp/agent). Harness omp work uses --profile glen-harness (.claude/skills/omp/SKILL.md); otherwise ask the owner to run it."
BASE = dict(os.environ)
UNKNOWN = "\x00"  # Cannot occur in a shell argv; never an affirmative signal.
ASSIGN = re.compile(r"([A-Za-z_][A-Za-z0-9_]*)=")
VARIABLE = re.compile(r"\$(?:\{([A-Za-z_][A-Za-z0-9_]*)\}|([A-Za-z_][A-Za-z0-9_]*))")
REDIR = re.compile(r"\d*(?:<<<|<<-?|>>|>&|<&|<>|>\||[<>])|&>>?")
OPERATORS = (";;&", "&&", "||", ";;", ";&", "|&", ";", "|", "&", "(", ")", "{", "}", "\n")
RESERVED = {"!", "if", "then", "do", "else", "elif", "while", "until", "}"}
PROFILE_VARS = {"OMP_PROFILE", "PI_PROFILE", "PI_CODING_AGENT_DIR"}

# Source audit: omp 18.14.0 reference 31147f9, packages/coding-agent/src/.
# Only these complete verb surfaces are admitted without a profile:
# - commands/grep.ts -> cli/grep-cli.ts: native search + stdout; initTheme reads.
# - commands/toks.ts: reads input, offline native token counts, stdout.
# - commands/play.ts -> stream/player.ts: reads recordings and paints the terminal.
# Native extraction is shared ~/.omp/natives infrastructure, not agent/ state.
READ_ONLY = {"omp": {"grep", "toks", "play"}, "glen": {"completions"}}
# Glen completions: src/commands/completions.rs only builds/prints clap metadata.
# Every remaining registered omp surface is excluded from the default allowlist:
# launch/acp/join -> main.runRootCommand: settings/auth/session runtime;
# auth/auth-broker/auth-gateway/login/token -> auth stores, refresh/token writes;
# agents -> cli/agents-cli (unpack); anonymize -> cli/anonymize-cli (Bun.write);
# bench/if-bench/dry-balance -> provider/settings/auth benchmark runtimes;
# browser-relay -> cli/browser-relay-cli (extension install/daemon state);
# cleanse/commit/compress -> agent sessions, edits/commits/output writes;
# config -> cli/config-cli -> Settings.init (migration/quarantine even on get/path);
# find -> judgment/standalone (Settings.init/auth/registry/cache);
# gc -> cli/gc-cli (apply deletes/compresses); gallery/git/read/render/shell/ttsr -> Settings.init;
# collab -> collab/registry (listing prunes stale metadata/socket files);
# grievances -> cli/grievances-cli (clean/push); images/img -> cli/images-cli (GC/daemon);
# install/plugin/plugins/skill/skills/setup/update -> installs/settings/auth;
# predict -> cli/predict-cli -> predict/client (daemon/model lifecycle);
# ps -> cli/ps-cli (stop/kill/restart); say -> Settings.init/model downloads/output;
# clip/stream -> stencil/credential (auth discovery/refresh), upload/stream runtime;
# share -> SessionManager.open + shareSession upload; session -> pack/unpack/moved;
# ssh -> cli/ssh-cli -> ssh/config-writer (add/remove); tiny-models -> downloads;
# stats -> cli/stats-cli -> omp-stats (session ingestion + rollup DB writes);
# usage -> cli/usage-cli: auth discovery/revalidate even accounts, invalidate/reset;
# worktree/wt -> add/clear and Settings.init; search/q/web-search -> Settings.init.
# completions imports EVERY command module; __complete uses SessionManager.list.
# Their transitive startup effects are not proven read-only: conservatively deny
# them without a non-default profile rather than widening this audited allowlist.
STATEFUL = set("launch acp auth auth-broker auth-gateway agents anonymize bench browser-relay cleanse collab commit completions __complete compress config dry-balance find gc gallery git grievances images img if-bench install join login logout models plugin plugins predict ps say clip share session setup shell read render skill skills ssh stats stream update usage tiny-models token ttsr worktree wt search q web-search settings install-extension install-skill".split())
ALWAYS = set("login logout plugin plugins install setup skill skills update settings config".split())
RISK = re.compile(r"(?<![\w-])(?:" + "|".join(re.escape(x) for x in sorted(STATEFUL | {"set", "reset", "add", "download", "clear", "unpack", "--apply", "--force"}, key=len, reverse=True)) + r")(?![\w-])", re.I)


def quoted_end(text, start, ansi=False):
    quote, i = text[start], start + 1
    while i < len(text):
        if text[i] == "\\" and (quote == '"' or ansi):
            i += 2
        elif text[i] == quote:
            return i + 1
        elif quote == '"' and (text.startswith("$(", i) or text[i] == "`"):
            i = substitution_end(text, i)
        else:
            i += 1
    raise ValueError("unclosed quote")


def heredoc_spec(raw, strip_tabs):
    return literal(raw, None), any(c in raw for c in "'\"\\"), strip_tabs


def heredoc_bodies(text, position, pending):
    bodies = []
    for delimiter, quoted, strip_tabs in pending:
        start = position
        while position < len(text):
            end = text.find("\n", position)
            end = len(text) if end < 0 else end
            line = text[position:end]
            if (line.lstrip("\t") if strip_tabs else line) == delimiter:
                bodies.append((text[start:position], quoted))
                position = min(end + 1, len(text))
                break
            position = min(end + 1, len(text))
        else:
            raise ValueError("unterminated heredoc")
    return position, bodies


def substitution_end(text, start):
    if text[start] == "`":
        i = start + 1
        while i < len(text):
            if text[i] == "`":
                return i + 1
            i += 2 if text[i] == "\\" else 1
        raise ValueError("unclosed backtick")
    i, depth, pending = start + 2, 1, []
    while i < len(text):
        if text.startswith("$'", i):
            i = quoted_end(text, i + 1, ansi=True)
        elif text[i] in "'\"":
            i = quoted_end(text, i)
        elif text[i] == "\\":
            i += 2
        elif text.startswith("$(", i) or text[i] == "`":
            i = substitution_end(text, i)
        elif text.startswith("<<", i) and not text.startswith("<<<", i):
            strip_tabs = text.startswith("<<-", i)
            i += 3 if strip_tabs else 2
            while i < len(text) and text[i] in " \t":
                i += 1
            end = word_end(text, i)
            pending.append(heredoc_spec(text[i:end], strip_tabs))
            i = end
        elif text[i] == "\n" and pending:
            i, _ = heredoc_bodies(text, i + 1, pending)
            pending = []
        elif text[i] == "#" and (i == 0 or text[i - 1].isspace()):
            end = text.find("\n", i)
            i = len(text) if end < 0 else end
        else:
            depth += (text[i] == "(") - (text[i] == ")")
            i += 1
            if depth == 0:
                return i
    raise ValueError("unclosed substitution")


def word_end(text, start):
    i = start
    while i < len(text):
        if text.startswith("$'", i):
            i = quoted_end(text, i + 1, ansi=True)
        elif text[i] in "'\"":
            i = quoted_end(text, i)
        elif text.startswith(("$(", "<(", ">("), i) or text[i] == "`":
            i = substitution_end(text, i)
        elif text.startswith("${", i):
            end = text.find("}", i + 2)
            if end < 0:
                raise ValueError("unclosed parameter")
            i = end + 1
        elif text[i] == "\\":
            if i + 1 >= len(text):
                raise ValueError("trailing escape")
            i += 2
        elif text[i].isspace() or text[i] in ";|&()<>":
            break
        else:
            i += 1
    if i == start:
        raise ValueError("empty word")
    return i


def tokens(command):
    i, pending, delimiter_op = 0, [], None
    while i < len(command):
        c = command[i]
        if c in " \t\r":
            i += 1
            continue
        if c == "#":
            end = command.find("\n", i)
            i = len(command) if end < 0 else end
            continue
        match = REDIR.match(command, i)
        if match and not command.startswith(("<(", ">("), i):
            raw = match[0]
            yield "redir", raw
            delimiter_op = raw if raw.lstrip("0123456789") in ("<<", "<<-") else None
            i = match.end()
            continue
        op = next((op for op in OPERATORS if command.startswith(op, i)), None)
        if op in {"{", "}"} and i + 1 < len(command) and not (command[i + 1].isspace() or command[i + 1] in ";|&()"):
            op = None  # Braces embedded in an argument are not reserved words.
        if op:
            yield "op", op
            i += len(op)
            if op == "\n" and pending:
                i, bodies = heredoc_bodies(command, i, pending)
                for body in bodies:
                    yield "heredoc", body
                pending = []
            continue
        end = word_end(command, i)
        raw = command[i:end]
        kind = "word"
        if delimiter_op:
            pending.append(heredoc_spec(raw, delimiter_op.endswith("-")))
            delimiter_op = None
            kind = "delimiter"
        yield kind, raw
        i = end
    if pending or delimiter_op:
        raise ValueError("missing heredoc body")


def ansi_text(text):
    def decode(match):
        value = match[1]
        if value[0] in "xXuU" and len(value) > 1:
            return chr(int(value[1:], 16))
        if value[0] in "01234567":
            return chr(int(value, 8))
        return {"a": "\a", "b": "\b", "e": "\x1b", "E": "\x1b", "f": "\f", "n": "\n", "r": "\r", "t": "\t", "v": "\v", "\\": "\\", "'": "'", '"': '"'}.get(value, "\\" + value)
    return re.sub(r"\\(x[0-9a-fA-F]{1,2}|u[0-9a-fA-F]{4}|U[0-9a-fA-F]{8}|[0-7]{1,3}|.)", decode, text)


def literal(raw, variables):
    """Quote removal and known expansions; never evaluate shell code."""
    out, i, quote = [], 0, None
    while i < len(raw):
        c = raw[i]
        if quote is None and raw.startswith("$'", i):
            end = quoted_end(raw, i + 1, ansi=True)
            out.append(ansi_text(raw[i + 2:end - 1]))
            i = end
            continue
        if c in "'\"" and (quote is None or quote == c):
            quote = c if quote is None else None
        elif c == "\\" and quote != "'" and i + 1 < len(raw):
            if quote != '"' or raw[i + 1] in '$`"\\':
                i += 1
            out.append(raw[i])
        elif variables is not None and quote != "'" and c == "$":
            match = VARIABLE.match(raw, i)
            if not match:
                return "".join(out) + UNKNOWN
            value = variables.get(match[1] or match[2]) or UNKNOWN
            out.append(UNKNOWN if quote is None and any(ch.isspace() for ch in value) else value)
            i = match.end() - 1
        elif variables is not None and quote != "'" and (c == "`" or raw.startswith(("<(", ">("), i)):
            return "".join(out) + UNKNOWN
        elif variables is not None and quote is None and c in "*?[":
            out.append(UNKNOWN)
        else:
            out.append(c)
        i += 1
    return "".join(out)


def substitutions(raw, heredoc=False):
    """Heredoc quote characters are data; only quoted DELIMITERS stop expansion."""
    i, quote = 0, None
    while i < len(raw):
        c = raw[i]
        if c == "\\" and quote != "'":
            i += 2
            continue
        if not heredoc and quote is None and raw.startswith("$'", i):
            i = quoted_end(raw, i + 1, ansi=True)
            continue
        if not heredoc and c in "'\"" and (quote is None or quote == c):
            quote = c if quote is None else None
        elif quote != "'" and (raw.startswith("$(", i) or not heredoc and raw.startswith(("<(", ">("), i) or c == "`"):
            end = substitution_end(raw, i)
            body = raw[i + (1 if c == "`" else 2):end - 1]
            yield body.replace("\\`", "`") if c == "`" else body
            i = end
            continue
        i += 1


def named(profile):
    return profile is not None and profile.strip() != "default" and bool(
        re.fullmatch(r"[a-z0-9][a-z0-9._-]{0,63}", profile.strip()))


def nondefault_dir(value):
    if not value or UNKNOWN in value:
        return False
    home = BASE.get("HOME", os.path.expanduser("~"))
    if value == "~" or value.startswith("~/"):
        value = home + value[1:]
    defaults = [os.path.join(home, ".omp", "agent"),
                os.path.join(home, (BASE.get("PI_CONFIG_DIR") or ".omp").lstrip("/"), "agent")]
    return all(os.path.realpath(value) != os.path.realpath(path) for path in defaults)


def env_split(text, variables):
    r"""GNU env -S, not shlex: no shell operators/substitutions, special escapes.

    coreutils doc/coreutils.texi, '-S/--split-string syntax': \_ separates only
    outside double quotes; \c stops; # comments only at a word start; only ${VAR}
    expands, before env's own -i/-u/assignments. Single quotes disable expansion.
    """
    words, word, quote, started, i = [], [], None, False, 0
    while i < len(text):
        c = text[i]
        if quote is None and c.isspace():
            if started:
                words.append("".join(word))
                word, started = [], False
        elif quote is None and c == "#" and not started:
            break
        elif c in "'\"" and (quote is None or quote == c):
            quote = c if quote is None else None
            started = True
        elif c == "\\" and i + 1 < len(text):
            nxt = text[i + 1]
            if quote == "'" and nxt not in "'\\":
                word.append(c)
                started = True
            else:
                i += 1
                if nxt == "c" and quote is None:
                    break
                if nxt == "_" and quote is None:
                    if started:
                        words.append("".join(word))
                        word, started = [], False
                else:
                    escapes = {"f": "\f", "n": "\n", "r": "\r", "t": "\t", "v": "\v", "_": " ", "#": "#", "$": "$", '"': '"', "'": "'", "\\": "\\"}
                    if nxt not in escapes:
                        raise ValueError("unsupported env -S escape")
                    word.append(escapes[nxt])
                    started = True
        elif c == "$" and quote != "'":
            match = re.match(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}", text[i:])
            if not match:
                raise ValueError("unsupported env -S variable")
            word.append(variables.get(match[1]) or UNKNOWN)
            started = True
            i += match.end() - 1
        else:
            word.append(c)
            started = True
        i += 1
    if quote:
        raise ValueError("unclosed env -S quote")
    if started:
        words.append("".join(word))
    return words


def guarded(words, parent, inherited):
    variables, signals, i, env_args = dict(parent), dict(inherited), 0, False
    if words and words[0] in {"for", "case", "function", "esac", "done", "fi"}:
        return False  # Headers/end words; operators expose their bodies separately.
    while i < len(words):
        raw = words[i]
        if raw in RESERVED:
            i += 1
            continue
        value = literal(raw, parent if env_args else variables)
        assignment = (ASSIGN.match(raw) or ASSIGN.match(value)) if env_args else ASSIGN.match(raw)
        if assignment:
            name = assignment[1]
            assigned = value[len(name) + 1:] if env_args else literal(raw[len(name) + 1:], variables)
            variables[name] = signals[name] = assigned
            i += 1
            continue
        program = os.path.basename(value).lower()
        if program == "eval":
            args = [literal(word, parent) for word in words[i + 1:]]
            if args and args[0] == "--":
                args = args[1:]
            script = " ".join(args)
            return UNKNOWN in script or inspect(script, variables, signals)
        if program in {"bash", "sh", "zsh"}:
            j = i + 1
            while j < len(words):
                option = literal(words[j], parent)
                if option == "--" or not option.startswith(("-", "+")):
                    break
                if option.startswith("-") and not option.startswith("--") and "c" in option[1:]:
                    script_at = j + 1 + int(any(flag in option[1:] for flag in "oO"))
                    if script_at >= len(words):
                        return True
                    script = literal(words[script_at], parent)
                    return UNKNOWN in script or inspect(script, variables, signals)
                j += 2 if option in {"-o", "+o", "-O", "+O", "--emulate", "--rcfile", "--init-file"} else 1
            return False
        wrappers = {"env", "command", "builtin", "sudo", "nice", "nohup", "exec", "time", "timeout", "caffeinate", "stdbuf", "xargs"}
        if program not in wrappers:
            break
        env_args = program == "env"
        env_input = dict(variables)
        preserve = program != "sudo"
        i += 1
        while i < len(words):
            option = literal(words[i], parent)
            if option == "--":
                i += 1
                break
            if not option.startswith("-") or option == "-" and program != "env":
                break
            if program == "command" and option in {"-v", "-V"}:
                return False
            split_flag = re.fullmatch(r"-([iv0]*)S(.*)", option) if program == "env" else None
            if program == "env" and (split_flag or option.startswith("--split-string=") or option == "--split-string"):
                fused = split_flag[2] if split_flag else option.partition("=")[2]
                if not fused and (split_flag or option == "--split-string"):
                    if i + 1 >= len(words):
                        return True
                    split, after = literal(words[i + 1], parent), i + 2
                else:
                    split, after = fused, i + 1
                if UNKNOWN in split:
                    return True
                expanded = [shlex.quote(word) for word in env_split(split, env_input)]
                if split_flag:
                    expanded = ["-" + flag for flag in split_flag[1]] + expanded
                return inspect(" ".join(words[:i] + expanded + words[after:]), parent, inherited)
            if program == "env" and (option in {"-i", "--ignore-environment", "-"} or re.fullmatch(r"-[v0]*i[iv0]*", option)):
                signals.clear()
                variables.clear()
            unset = None
            if program == "env":
                if option.startswith("--unset="):
                    unset = option.split("=", 1)[1]
                elif option.startswith("-u") and option != "-u":
                    unset = option[2:]
                elif option in {"-u", "--unset"} and i + 1 < len(words):
                    unset = literal(words[i + 1], parent)
            if unset is not None:
                signals.pop(unset, None)
                variables.pop(unset, None)
            if program == "sudo" and option in {"-E", "--preserve-env"}:
                preserve = True
            if program == "exec" and option == "-c":
                signals.clear()
                variables.clear()
            takes_value = {
                "env": {"-u", "--unset", "-C", "--chdir"},
                "sudo": {"-u", "--user", "-g", "--group", "-h", "--host", "-p", "--prompt", "-C", "--close-from"},
                "nice": {"-n", "--adjustment"}, "exec": {"-a"}, "time": {"-f", "--format", "-o", "--output"},
                "timeout": {"-s", "--signal", "-k", "--kill-after"}, "caffeinate": {"-t", "-w"},
                "stdbuf": {"-i", "-o", "-e", "--input", "--output", "--error"},
                "xargs": {"-I", "-J", "-E", "-L", "-n", "-P", "-s", "-R", "-S", "-d", "--arg-file", "-a", "--delimiter", "--max-args", "--max-procs", "--max-chars", "--process-slot-var"},
            }
            i += 2 if option in takes_value.get(program, set()) else 1
        if program == "timeout":
            i += 1  # One duration operand, after the wrapper's options.
        if program == "xargs" and i >= len(words):
            return True  # No explicit template: stdin is not statically inspectable.
        if not preserve:
            signals.clear()
            for key in PROFILE_VARS:
                variables.pop(key, None)
    if i >= len(words):
        return False
    # CLI argv expands in the parent shell, not under prefix assignments.
    value = literal(words[i], parent)
    args = [literal(word, parent) for word in words[i + 1:]]
    if UNKNOWN in value:
        return any(UNKNOWN in arg or RISK.search(arg) for arg in args) or not args
    program = os.path.basename(value).lower()
    if program not in READ_ONLY:
        return False
    profile, explicit, j = None, False, 0
    value_flags = {"--model", "--provider", "--config", "--thinking", "--session", "--session-dir", "--cwd", "--color"}
    while j < len(args):
        arg = args[j]
        if arg == "--":
            j += 1
            break
        if UNKNOWN in arg:
            return True
        if arg == "--profile":
            explicit = True
            profile = args[j + 1] if j + 1 < len(args) else None
            j += 2
        elif arg.startswith("--profile="):
            explicit, profile = True, arg.split("=", 1)[1]
            j += 1
        elif arg in {"--version", "-v", "--help", "-h"}:
            return False
        elif arg in value_flags:
            j += 2
        elif arg.startswith("-"):
            j += 1
        else:
            break
    rest = [arg.lower() for arg in args[j:]]
    verb = rest[0] if rest else "launch"
    second = rest[1] if len(rest) > 1 else ""
    if UNKNOWN in verb:
        return True
    if program == "glen" and verb == "install-extension":
        return True  # Unconditional unattended-install ban, including dry runs.
    if program == "omp":
        if verb in ALWAYS:
            return True
        if (verb == "gc" and "--apply" in rest or verb == "agents" and "--force" in rest
                or verb == "auth" and second == "import" or verb == "session" and second == "unpack"
                or verb == "ssh" and second == "add" or verb == "tiny-models" and second in {"", "download"}
                or verb in {"worktree", "wt"} and second == "clear"):
            return True
        if verb == "models":
            return not (explicit and named(profile))
    if verb in READ_ONLY[program]:
        return False
    if any(UNKNOWN in arg for arg in rest):
        return True  # Could hide one of the unconditional actions above.
    if explicit:
        return not named(profile)
    selected = signals.get("OMP_PROFILE") if "OMP_PROFILE" in signals else signals.get("PI_PROFILE")
    return not (named(selected) or nondefault_dir(signals.get("PI_CODING_AGENT_DIR")))


def inspect(command, parent=None, inherited=None):
    parent = BASE if parent is None else parent
    inherited = {} if inherited is None else inherited
    stream = list(tokens(command.replace("\\\n", "")))
    words, skip_redirect, i = [], False, 0
    while i < len(stream):
        kind, raw = stream[i]
        if kind == "heredoc":
            body, quoted = raw
            if not quoted and any(inspect(nested, parent, inherited) for nested in substitutions(body, heredoc=True)):
                return True
        elif kind == "op":
            if guarded(words, parent, inherited):
                return True
            words, skip_redirect = [], False
        elif kind == "delimiter":
            skip_redirect = False  # Heredoc delimiter words never expand.
        elif kind == "redir":
            skip_redirect = True
        else:
            # f(){...} defines f; the name itself is not a command invocation.
            if (not words or words == ["function"]) and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", raw) and stream[i + 1:i + 4] == [("op", "("), ("op", ")"), ("op", "{")]:
                words, skip_redirect, i = [], False, i + 4
                continue
            if any(inspect(nested, parent, inherited) for nested in substitutions(raw)):
                return True
            if skip_redirect:
                skip_redirect = False
            else:
                words.append(raw)
        i += 1
    return guarded(words, parent, inherited)


try:
    denied = inspect(os.fdopen(3).read())
except (ValueError, RecursionError):
    print("Cannot safely inspect this shell syntax. " + MESSAGE, file=sys.stderr)
    denied = True
if denied:
    print(MESSAGE, file=sys.stderr)
print("omp-guard-parsed")
sys.exit(2 if denied else 0)
PY
}
parser_output="$(parse_command)" || parser_status=$?
case "$parser_status:$parser_output" in
  0:omp-guard-parsed | 2:omp-guard-parsed) exit "$parser_status" ;;
  *) printf '%s\n' 'Cannot run the omp profile guard parser; refusing the command.' >&2; exit 2 ;;
esac
