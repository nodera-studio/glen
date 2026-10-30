#!/usr/bin/env bash
set -euo pipefail

# Static shell inspection only: never execute the submitted command. Python's
# isolated mode ignores user startup/import settings. fd 3 carries the hook JSON;
# stdin carries this parser, keeping command text out of argv and the environment.
parser_status=0
python3 -I - 3<&0 <<'PY' || parser_status=$?
import json
import os
import re
import sys

MESSAGE = "This would write the owner's default omp profile (~/.omp/agent). Harness omp work uses --profile glen-harness (.claude/skills/omp/SKILL.md); otherwise ask the owner to run it."
# Retain RAW words until command positions are known. Quoted separators, comments
# and executable names inside another command's arguments are ordinary word data.
TOKEN = re.compile(r'''(?P<space>[ \t\r]+)|(?P<comment>\#[^\n]*)|(?P<redir>\d*(?:>>|<<-?|>&|<&|[<>]))|(?P<op>&&|\|\||[;|&()\n])|(?P<word>(?:[^ \t\r\n;&|()<>"'\\`]+|'[^']*'|"(?:\\.|[^"\\])*"|`(?:\\.|[^`\\])*`|\\.)+)''')
ASSIGN = re.compile(r"([A-Za-z_][A-Za-z0-9_]*)=")
VARIABLE = re.compile(r"\$(?:\{([A-Za-z_][A-Za-z0-9_]*)\}|([A-Za-z_][A-Za-z0-9_]*))")
BASE = dict(os.environ)
UNKNOWN = "\x00"  # Cannot occur in a shell argv; never an affirmative signal.


def literal(raw, variables):
    """Quote removal plus known $VAR/${VAR} expansion; no evaluation or globbing."""
    out, quote, i = [], None, 0
    while i < len(raw):
        c = raw[i]
        if c in "\"'" and (quote is None or quote == c):
            quote = c if quote is None else None
        elif c == "\\" and quote != "'" and i + 1 < len(raw):
            if quote != '"' or raw[i + 1] in '$`"\\':
                i += 1
            out.append(raw[i])
        elif c == "$" and quote != "'":
            match = VARIABLE.match(raw, i)
            if not match:
                return "".join(out) + UNKNOWN
            out.append(variables.get(match[1] or match[2]) or UNKNOWN)
            i = match.end() - 1
        elif c == "`" and quote != "'":
            return "".join(out) + UNKNOWN
        else:
            out.append(c)
        i += 1
    return "".join(out)


def substitutions(raw):
    """Inspect executable substitutions, including those inside double quotes."""
    i, quote = 0, None
    while i < len(raw):
        c = raw[i]
        if c == "\\" and quote != "'":
            i += 2
            continue
        if c in "\"'" and (quote is None or quote == c):
            quote = c if quote is None else None
        elif quote != "'" and c == "`":
            end = i + 1
            while end < len(raw) and raw[end] != "`":
                end += 2 if raw[end] == "\\" else 1
            yield raw[i + 1:end]
            i = end
        elif quote != "'" and raw.startswith("$(", i):
            start, end, depth, inner_quote = i + 2, i + 2, 1, None
            while end < len(raw) and depth:
                ch = raw[end]
                if ch == "\\" and inner_quote != "'":
                    end += 2
                    continue
                if ch in "\"'" and (inner_quote is None or inner_quote == ch):
                    inner_quote = ch if inner_quote is None else None
                elif inner_quote is None:
                    depth += (ch == "(") - (ch == ")")
                end += 1
            yield raw[start:end - 1] if depth == 0 else raw[start:end]
            i = end - 1
        i += 1


def named(profile):
    # dirs.ts normalizes empty/default to no profile; OMP_PROFILE wins over PI_PROFILE.
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


def guarded(words):
    variables, signals, i, env_args = dict(BASE), {}, 0, False
    while i < len(words):
        raw = words[i]
        value = literal(raw, BASE if env_args else variables)
        assignment = (ASSIGN.match(raw) or ASSIGN.match(value or "")) if env_args else ASSIGN.match(raw)
        if assignment:
            name = assignment[1]
            assigned = (value[len(name) + 1:] if value is not None else None) if env_args else literal(raw[len(name) + 1:], variables)
            variables[name] = signals[name] = assigned
            i += 1
            continue
        program = os.path.basename(value or "").lower()
        if program not in ("env", "command", "sudo", "nice"):
            break
        env_args = program == "env"
        preserve = program != "sudo"
        i += 1
        while i < len(words):
            option = literal(words[i], BASE)
            if option == "--":
                i += 1
                break
            if option is None or not option.startswith("-"):
                break
            if program == "command" and option in ("-v", "-V"):
                return False
            if program == "env" and option in ("-i", "--ignore-environment", "-"):
                signals.clear()
            if program == "env" and option.startswith("-u") and option != "-u":
                signals.pop(option[2:], None)
            if program == "env" and option in ("-u", "--unset") and i + 1 < len(words):
                signals.pop(literal(words[i + 1], BASE), None)
            if program == "env" and option.startswith("--unset="):
                signals.pop(option.split("=", 1)[1], None)
            if program == "sudo" and option in ("-E", "--preserve-env"):
                preserve = True
            takes_value = {"env": ("-u", "--unset", "-C", "--chdir"),
                           "sudo": ("-u", "--user", "-g", "--group", "-h", "--host", "-p", "--prompt", "-C", "--close-from"),
                           "nice": ("-n", "--adjustment"), "command": ()}
            i += 2 if option in takes_value[program] else 1
        if not preserve:
            signals.clear()
    if i >= len(words):
        return False
    # Arguments expand in the parent shell, BEFORE prefix assignments take effect.
    # Do not let `OMP_PROFILE=safe omp --profile "$OMP_PROFILE" ...` invent a flag value.
    program = os.path.basename(literal(words[i], BASE) or "").lower()
    if program not in ("omp", "glen"):
        return False
    args = [literal(word, BASE) for word in words[i + 1:]]
    explicit, profile, rest, j, uncertain = False, None, [], 0, False
    value_flags = {"--model", "--provider", "--config", "--thinking", "--session", "--session-dir", "--cwd", "--color"}
    while j < len(args):
        arg = args[j]
        if arg == "--profile":
            explicit = True
            profile = args[j + 1] if j + 1 < len(args) else None
            j += 2
            continue
        if arg is not None and arg.startswith("--profile="):
            explicit, profile = True, arg.split("=", 1)[1]
        elif not rest and arg in value_flags:
            j += 2
            continue
        elif not rest and (arg is None or UNKNOWN in arg):
            uncertain = True
            rest.append(arg)
        elif not rest and arg is not None and arg.startswith("-"):
            if arg in ("--version", "-v", "--help", "-h"):
                return False
        else:
            rest.append(arg.lower() if arg is not None else None)
        j += 1
    verb = rest[0] if rest else None
    second = rest[1] if len(rest) > 1 else None
    unknown_second = len(rest) > 1 and (second is None or UNKNOWN in second)
    blocked = uncertain or (program == "glen" and verb == "install-extension") or (
        program == "omp" and (verb in {"login", "logout", "plugin", "plugins", "install", "setup", "skill", "skills", "update", "settings"}
        or (verb == "config" and (second in {"set", "reset"} or unknown_second))
        or (verb in {"auth", "session"} and unknown_second)
        or (verb == "gc" and "--apply" in rest)
        or (verb == "auth" and second == "import")
        or (verb == "agents" and "--force" in rest)
        or (verb == "session" and second == "unpack")))
    if not blocked:
        return False
    if program == "glen" and verb == "install-extension" and "--dry-run" in rest:
        return False
    if explicit:
        return not named(profile)
    selected = signals.get("OMP_PROFILE") if "OMP_PROFILE" in signals else signals.get("PI_PROFILE")
    return not (named(selected) or nondefault_dir(signals.get("PI_CODING_AGENT_DIR")))


def inspect(command):
    # Continuations first; comments are removed lexically before command analysis.
    command = command.replace("\\\n", "")
    words, skip_redirect, position = [], False, 0
    while position < len(command):
        match = TOKEN.match(command, position)
        if not match:
            # Invalid/unclosed shell syntax cannot be characterized statically.
            raise ValueError("unrecognized shell syntax")
        position = match.end()
        kind, raw = match.lastgroup, match[0]
        if kind in ("space", "comment"):
            continue
        if kind == "redir":
            skip_redirect = True
        elif kind == "op":
            if guarded(words):
                return True
            words, skip_redirect = [], False
        else:
            if any(inspect(nested) for nested in substitutions(raw)):
                return True
            if skip_redirect:
                skip_redirect = False
            else:
                words.append(raw)
    return guarded(words)


try:
    data = json.load(os.fdopen(3))
    denied = inspect(data.get("tool_input", {}).get("command", ""))
except (ValueError, RecursionError):
    print("Cannot safely inspect this shell syntax. " + MESSAGE, file=sys.stderr)
    sys.exit(2)
if denied:
    print(MESSAGE, file=sys.stderr)
sys.exit(2 if denied else 0)
PY
case "$parser_status" in
  0 | 2) exit "$parser_status" ;;
  *) printf '%s\n' 'Cannot run the omp profile guard parser; refusing the command.' >&2; exit 2 ;;
esac
