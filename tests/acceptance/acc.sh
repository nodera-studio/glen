#!/usr/bin/env bash
# Real acceptance is owner/orchestrator-only. `check` reads recordings, never calls glen.
set -euo pipefail
A=$(cd -- "$(dirname -- "$0")" && pwd -P)
ROOT=$(cd -- "$A/../.." && pwd -P)
die() { printf '%s\n' "$*" >&2; exit 2; }
need() { command -v "$1" >/dev/null || die "missing prerequisite: $1"; }
copy_transcript() {
    if [[ -f $1 ]]; then
        jq -c 'select(.type == "user" or .type == "assistant" or .type == "system" or .type == "summary")' "$1" > "$2"
    fi
}
nonce() { printf '%s-' "$1"; od -An -N4 -tx1 /dev/urandom | tr -d ' \n'; }
# Portable names + mtimes + sizes, no contents. No credentials enter the evidence.
snapshot() {
    local dir=$1
    if [[ ! -d $dir ]]; then printf '[]\n'; return; fi
    find "$dir" -type f -name '*.jsonl' -print0 | perl -0 -MJSON::PP -MTime::HiRes=stat -e '
      my @rows; while(<STDIN>){chomp; my @s=stat($_); push @rows,{path=>$_,mtime=>$s[9],size=>$s[7]};}
      print JSON::PP->new->canonical->encode([sort {$a->{path} cmp $b->{path}} @rows]),"\n";'
}
safe_scratch() {
    [[ -d $1 && ! -L $1 && -f $1/manifest.json ]] || die 'not an acceptance scratch directory'
    local actual
    actual=$(cd -- "$1" && pwd -P)
    [[ $actual == "$(jq -r .scratch "$actual/manifest.json")" ]] || die 'scratch path differs from manifest'
}
# macOS /usr/bin/screen 4.00.03 runs -X stuff/hardcopy against no window in a never-attached
# session (a silent no-op, an empty hardcopy), so every window command selects window 0.
tui() { screen -S "$1" -p 0 -X "${@:2}"; }
# omp's prepaint keeps the pty cooked until its settings load, so a CR typed then reaches the editor
# as LF (insert newline) and nothing is sent. omp draws the resumed transcript only after it takes
# raw input, so wait for the session's first prompt on screen before typing.
tui_ready() {
    local name=$1 first=$2 deadline=$((SECONDS+60))
    until tui "$name" hardcopy -h "$S/$name.ready.txt" && sleep 0.3 && LC_ALL=C grep -aqF -- "$first" "$S/$name.ready.txt"; do
        ((SECONDS<deadline)) || die "$name: omp TUI not ready"; sleep 0.2
    done
}
require_w3_surface() {
    local binary=$1 help
    help=$("$binary" --help) || die 'acceptance needs W3'
    grep -Eq '^  inbox[[:space:]]' <<< "$help" || die 'acceptance needs W3'
    grep -Eq '^  install-extension[[:space:]]' <<< "$help" || die 'acceptance needs W3'
}
setup() {
    local mode=$1
    [[ $mode =~ ^(a|b|b2|b3|c1|c2)$ ]] || die 'mode must be a, b, b2, b3, c1 or c2'
    for tool in jq perl screen git shasum; do need "$tool"; done
    case "$mode" in a) need expect;; b|b2|b3) need claude;; c1|c2) need codex;; esac
    if [[ $mode == c2 && ${ACC_C2_CONSENT-} != yes ]]; then die 'c2 owner-pending: explicit owner consent required (ACC_C2_CONSENT=yes)'; fi
    (cd "$ROOT" && cargo build --release --locked) >&2
    local binary="$ROOT/target/release/glen"
    require_w3_surface "$binary"
    need omp
    [[ $(omp --version) == 'omp/18.14.0' ]] || die 'acceptance needs omp/18.14.0'
    local S P G report sessions agent probe R L X ext suffix screen_l screen_x deadline out
    S=$(mktemp -d "${TMPDIR:-/tmp}/ga.XXXXXX"); S=$(cd -- "$S" && pwd -P)
    P="$S/project"; G="$S/glen-bin/glen"
    printf 'scratch: %s\n' "$S" >&2
    ((${#S}+5+28 < 104)) || die 'scratch GLEN_HOME socket path exceeds macOS limit'
    mkdir -p "$S/glen-bin" "$S/bin" "$P" "$S/probe"
    cp "$binary" "$G"; git init -q "$P"
    unset OMP_PROFILE PI_PROFILE
    export GLEN_HOME="$S/home"
    # O1: keep the owner's normal agent dir/auth. Never export/import credentials.
    report=$("$G" doctor --json)
    sessions=$(jq -r .data.paths.sessions_dir <<< "$report")
    agent=$(jq -r .data.paths.agent_dir <<< "$report")
    export PI_CODING_AGENT_DIR="$agent"
    [[ $sessions == /* && $sessions != null ]] || die 'cannot resolve owner sessions directory'
    snapshot "$sessions" > "$S/owner-sessions.before.json"
    # Install state is names/metadata only; skills are installed solely in the scratch project.
    for dir in "${CLAUDE_CONFIG_DIR:-$HOME/.claude}/skills" "$HOME/.agents/skills"; do ls -lad "$dir" "$dir/glen" "$dir/glen/SKILL.md" 2>/dev/null || true; done > "$S/owner-skills.before.txt"
    if ! probe=$("$G" send @new 'Reply with the word ready.' --cwd "$S/probe" --wait 120s --json); then
        printf 'blocked: no usable omp credentials/model in the normal agent dir; scratch: %s\n' "$S" >&2
        exit 3
    fi
    [[ $(jq -r .data.status <<< "$probe") == completed ]] || die 'credential probe did not complete'
    ext="$S/extension-unused"
    if [[ $mode != b2 ]]; then
        env PI_CODING_AGENT_DIR="$S/extension-agent" "$G" install-extension --json > "$S/extension-install.json"
        ext="$S/extension-agent/extensions/glen/index.ts"
    fi
    R=$("$G" send @new 'Acceptance R: reply with the single word ready.' --cwd "$P" --wait 120s --json)
    L=$("$G" send @new 'Acceptance L: reply with the single word ready.' --cwd "$P" --wait 120s --json)
    X=$("$G" send @new 'Acceptance X: reply with the single word ready.' --cwd "$P" --wait 120s --json)
    jq -n --arg mode "$mode" --arg scratch "$S" --arg project "$P" --arg sessions "$sessions" --arg agent "$agent" --arg nr "$(nonce R)" --arg nl "$(nonce L)" --arg nt "$(nonce T)" \
      --argjson r "$R" --argjson l "$L" --argjson x "$X" --argjson probe "$probe" \
      '{mode:$mode,scratch:$scratch,project:$project,sessions_dir:$sessions,agent_dir:$agent,R:$r.data.session,L:$l.data.session,X:$x.data.session,NONCE_R:$nr,NONCE_L:$nl,NONCE_T:$nt,buckets:([$r.data.session.path,$l.data.session.path,$x.data.session.path,$probe.data.session.path]|map(sub("/[^/]+$";""))|unique)}' > "$S/manifest.json"
    jq -e --arg dir "$sessions/" '[.R,.L,.X]|all(.path|startswith($dir))' "$S/manifest.json" >/dev/null || die 'session outside resolved sessions dir'
    suffix=${S##*.}; screen_l="ga-L-$suffix"; screen_x="ga-X-$suffix"
    jq --arg l "$screen_l" --arg x "$screen_x" '. + {screen_l:$l,screen_x:$x}' "$S/manifest.json" > "$S/manifest.tmp"
    mv "$S/manifest.tmp" "$S/manifest.json"
    if [[ $mode != b2 ]]; then
        # Launcher scripts avoid shell-quoting session paths into screen's command string.
        printf '#!/usr/bin/env bash\ncd %q\nunset OMP_PROFILE PI_PROFILE\nexport GLEN_HOME=%q\nexport PI_CODING_AGENT_DIR=%q\nexec omp --no-extensions -e %q --resume %q\n' "$P" "$GLEN_HOME" "$agent" "$ext" "$(jq -r .L.path "$S/manifest.json")" > "$S/live.sh"
        printf '#!/usr/bin/env bash\ncd %q\nunset OMP_PROFILE PI_PROFILE\nexport GLEN_HOME=%q\nexport PI_CODING_AGENT_DIR=%q\nexec omp --no-extensions --resume %q\n' "$P" "$GLEN_HOME" "$agent" "$(jq -r .X.path "$S/manifest.json")" > "$S/held.sh"
        screen -dmS "$screen_l" bash "$S/live.sh"
        deadline=$((SECONDS+30))
        while [[ ! -f $GLEN_HOME/run/live/$(jq -r .L.id "$S/manifest.json").json ]]; do ((SECONDS<deadline)) || die 'live endpoint timeout'; sleep 0.2; done
        tui_ready "$screen_l" 'Acceptance L: reply with the single word ready.'
        screen -dmS "$screen_x" bash "$S/held.sh"
        tui_ready "$screen_x" 'Acceptance X: reply with the single word ready.'
        tui "$screen_x" stuff $'Acceptance X: answer with the word ok.\r'
        deadline=$((SECONDS+120))
        while :; do
            out=$("$G" ls --cwd "$P" --json)
            if jq -e --arg id "$(jq -r .X.id "$S/manifest.json")" '.data.sessions|any(.id==$id and (.state=="live" or .state=="busy"))' <<< "$out" >/dev/null; then break; fi
            ((SECONDS<deadline)) || die 'X lease timeout'; sleep 0.5
        done
        if [[ $mode != a ]]; then
            tui "$screen_l" stuff "Use glen_reply to tell the calling agent: $(jq -r .NONCE_T "$S/manifest.json")"$'\r'
            deadline=$((SECONDS+120))
            while :; do
                out=$("$G" inbox --peek --json)
                if jq -e --arg n "$(jq -r .NONCE_T "$S/manifest.json")" 'tostring|contains($n)' <<< "$out" >/dev/null; then break; fi
                ((SECONDS<deadline)) || die 'talk-back timeout'; sleep 0.5
            done
        fi
    fi
    if [[ $mode != a ]]; then (cd "$P" && "$G" install-skill --project --json) > "$S/skill-install.json"; fi
    printf '#!/usr/bin/env bash\nACC_S=%q\n' "$S" > "$S/bin/glen"
    tail -n +2 "$A/glen-wrapper.sh" >> "$S/bin/glen"; chmod 755 "$S/bin/glen"
    : > "$S/calls.ndjson"
    case "$mode" in b) local template=claude;; b3) local template=claude-discover;; *) local template=codex;; esac
    # Render with jq, so paths/nonces are data rather than executable substitutions.
    local skill_path="$P/.claude/skills/glen/SKILL.md" skill=''
    if [[ -f $skill_path ]]; then skill=$(cat "$skill_path"); fi
    jq -nr --rawfile template "$A/prompts/$template.md" --arg glen "$S/bin/glen" --arg project "$P" --arg path "$skill_path" --arg skill "$skill" --slurpfile m "$S/manifest.json" \
      '$template|split("{{GLEN}}")|join($glen)|split("{{PROJECT}}")|join($project)|split("{{SKILL_PATH}}")|join($path)|split("{{SKILL}}")|join($skill)|split("{{NONCE_R}}")|join($m[0].NONCE_R)|split("{{NONCE_L}}")|join($m[0].NONCE_L)|split("{{NONCE_T}}")|join($m[0].NONCE_T)' > "$S/prompt-$mode.md"
    find "$GLEN_HOME/jobs" -mindepth 1 -maxdepth 1 -type d -print 2>/dev/null | sort > "$S/jobs.before.txt"
    jq -n --arg sha "$(git -C "$ROOT" rev-parse HEAD)" --arg binary_sha "$(shasum -a 256 "$G" | cut -d ' ' -f1)" --arg omp "$(omp --version)" --arg claude "$(claude --version 2>/dev/null || true)" --arg codex "$(codex --version 2>/dev/null || true)" --arg jq "$(jq --version)" --arg screen "$(screen --version)" --arg expect "$(expect -v)" \
      '{git_sha:$sha,binary_sha256:$binary_sha,tools:{omp:$omp,claude:$claude,codex:$codex,jq:$jq,screen:$screen,expect:$expect},env_names:["GLEN_HOME","PATH","TERM","COLUMNS","OMP_PROFILE","PI_PROFILE","PI_CODING_AGENT_DIR"],credential_copy:false}' > "$S/provenance.json"
    printf '%s\n' "$S"
}
check_mode() {
    local mode=$1 S=$2
    [[ $mode =~ ^(a|b|b2|b3|c1|c2)$ ]] || die 'unknown check mode'
    [[ -f $S/manifest.json ]] || die 'missing manifest'
    if [[ ! -f $S/calls.ndjson ]]; then
        # Reuse the mode's check IDs, but fail every check when setup left no recording.
        jq -n --arg mode "$mode" --arg scratch "$S" --arg report '' --argjson person '{}' \
          --argjson jobs_unchanged false --argjson config_restored false --slurpfile m "$S/manifest.json" \
          --argjson calls '[]' --argjson transcript '[]' --argjson discovered '[]' --argjson probe '[]' --argjson control '[]' \
          -f "$A/check.jq" | jq --arg path "$S/calls.ndjson" \
          '.pass=false | .checks |= map(.pass=false | .evidence=("missing recording: "+$path))' > "$S/verdict.json"
        jq -r '.checks[]|"FAIL \(.id): \(.evidence)"' "$S/verdict.json"
        jq -e '.pass' "$S/verdict.json" >/dev/null
        return
    fi
    # Empty optional inputs are arrays; required evidence is checked by its mode.
    local empty
    empty=$(mktemp "${TMPDIR:-/tmp}/ga-check.XXXXXX"); printf '' > "$empty"
    local report='' person='{}' jobs=true config=false
    [[ ! -f $S/report.md ]] || report=$(cat "$S/report.md")
    [[ ! -f $S/person-verdict.json ]] || person=$(cat "$S/person-verdict.json")
    if [[ $mode == c1 ]]; then
        find "$S/home/jobs" -mindepth 1 -maxdepth 1 -type d -print 2>/dev/null | sort > "$S/jobs.after.txt" || true
        cmp -s "$S/jobs.before.txt" "$S/jobs.after.txt" || jobs=false
    fi
    if [[ $mode == c2 && -f $S/config.before.sha256 ]]; then
        [[ $(shasum -a 256 "$HOME/.codex/config.toml" | cut -d ' ' -f1) == "$(cat "$S/config.before.sha256")" ]] && config=true
    fi
    jq -n --arg mode "$mode" --arg scratch "$S" --arg report "$report" --argjson person "$person" --argjson jobs_unchanged "$jobs" --argjson config_restored "$config" \
      --slurpfile m "$S/manifest.json" --slurpfile calls "${S}/calls.ndjson" \
      --slurpfile transcript "$([[ -f $S/transcript.jsonl ]] && printf '%s' "$S/transcript.jsonl" || printf '%s' "$empty")" \
      --slurpfile discovered "$([[ -f $S/b3.jsonl ]] && printf '%s' "$S/b3.jsonl" || printf '%s' "$empty")" \
      --slurpfile probe "$([[ -f $S/b2-probe.jsonl ]] && printf '%s' "$S/b2-probe.jsonl" || printf '%s' "$empty")" \
      --slurpfile control "$([[ -f $S/b2-control.jsonl ]] && printf '%s' "$S/b2-control.jsonl" || printf '%s' "$empty")" \
      -f "$A/check.jq" > "$S/verdict.json"
    rm -f "$empty"
    jq -r '.checks[]|"\(if .pass then "PASS" else "FAIL" end) \(.id): \(.evidence)"' "$S/verdict.json"
    jq -e '.pass' "$S/verdict.json" >/dev/null
}
teardown() {
    local S=$1 evidence=$2
    safe_scratch "$S"; need gitleaks
    local name
    for name in "$(jq -r '.screen_l // empty' "$S/manifest.json")" "$(jq -r '.screen_x // empty' "$S/manifest.json")"; do
        [[ -n $name && $name == ga-* ]] || continue
        tui "$name" hardcopy -h "$S/$name.screen.txt" 2>/dev/null || true
        tui "$name" stuff $'/exit\r' 2>/dev/null || true
    done
    sleep 3
    for name in "$(jq -r '.screen_l // empty' "$S/manifest.json")" "$(jq -r '.screen_x // empty' "$S/manifest.json")"; do
        [[ -n $name && $name == ga-* ]] || continue
        screen -S "$name" -X quit 2>/dev/null || true
    done
    local status=0 sessions
    sessions=$(jq -r .sessions_dir "$S/manifest.json")
    if [[ -d $S/home/run/live ]] && [[ -n $(find "$S/home/run/live" -type f -print -quit) ]]; then printf 'FAIL live endpoints remain\n' >&2; status=1; fi
    snapshot "$sessions" > "$S/owner-sessions.after.json"
    if ! jq -en --slurpfile before "$S/owner-sessions.before.json" --slurpfile after "$S/owner-sessions.after.json" --slurpfile m "$S/manifest.json" '
      all($before[0][]; . as $old | any($after[0][]; .==$old)) and
      all($after[0][]; . as $new | any($before[0][];.==$new) or any($m[0].buckets[]; . as $bucket | $new.path|startswith($bucket+"/")))' >/dev/null; then printf 'FAIL owner_session_snapshot\n' >&2; status=1; fi
    for dir in "${CLAUDE_CONFIG_DIR:-$HOME/.claude}/skills" "$HOME/.agents/skills"; do ls -lad "$dir" "$dir/glen" "$dir/glen/SKILL.md" 2>/dev/null || true; done > "$S/owner-skills.after.txt"
    cmp -s "$S/owner-skills.before.txt" "$S/owner-skills.after.txt" || { printf 'FAIL owner skills changed\n' >&2; status=1; }
    mkdir -p "$evidence"
    local file
    for file in manifest.json calls.ndjson provenance.json verdict.json report.md b3.jsonl b2-probe.jsonl b2-control.jsonl person-verdict.json owner-sessions.before.json owner-sessions.after.json owner-skills.before.txt owner-skills.after.txt; do [[ ! -f $S/$file ]] || cp "$S/$file" "$evidence/"; done
    copy_transcript "$S/transcript.jsonl" "$evidence/transcript.jsonl" || { printf 'FAIL transcript copy\n' >&2; status=1; }
    for file in "$S"/prompt-*.md "$S"/*.typescript "$S"/*.screen.txt; do [[ ! -f $file ]] || cp "$file" "$evidence/"; done
    jq -r '.buckets[]' "$S/manifest.json" > "$evidence/scratch-buckets.txt"
    jq -r --slurpfile m "$S/manifest.json" '.[] | .path as $path | select(any($m[0].buckets[]; . as $b|$path|startswith($b+"/"))) | .path' "$S/owner-sessions.after.json" > "$evidence/sessions-left.txt"
    local secrets_clean=true
    gitleaks dir --no-banner --redact "$evidence" || { secrets_clean=false; status=1; }
    if [[ -f $evidence/verdict.json ]]; then
        jq --argjson safe "$([[ $status == 0 ]] && printf true || printf false)" --argjson secrets "$secrets_clean" '.checks += [{id:"AC-419-teardown",pass:$safe,evidence:"session/skill snapshots and endpoint cleanup"},{id:"AC-421-secrets",pass:$secrets,evidence:"gitleaks dir"}] | .pass=(.pass and $safe and $secrets)' "$evidence/verdict.json" > "$evidence/verdict.tmp"
        mv "$evidence/verdict.tmp" "$evidence/verdict.json"
    fi
    # O1: no credential store was copied; never delete agent.db or omp session files.
    return "$status"
}
[[ ${BASH_SOURCE[0]} == "$0" ]] || return 0
case "${1-}" in
    setup) [[ $# == 2 ]] || die 'usage: acc.sh setup MODE'; setup "$2" ;;
    person) [[ $# == 2 ]] || die 'usage: acc.sh person S'; safe_scratch "$2"; expect "$A/person.exp" "$2" ;;
    check) [[ $# == 3 ]] || die 'usage: acc.sh check MODE S'; check_mode "$2" "$3" ;;
    teardown) [[ $# == 3 ]] || die 'usage: acc.sh teardown S EVIDENCE'; teardown "$2" "$3" ;;
    *) die 'usage: acc.sh setup MODE | person S | check MODE S | teardown S EVIDENCE' ;;
esac
