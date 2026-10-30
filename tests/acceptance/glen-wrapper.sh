#!/usr/bin/env bash
# Installed with a leading ACC_S assignment by setup. Never use this wrapper for pty mode.
set -uo pipefail
: "${ACC_S:?setup must bake ACC_S into this wrapper}"
REAL="$ACC_S/glen-bin/glen"
MANIFEST="$ACC_S/manifest.json"
LOG="$ACC_S/calls.ndjson"
unset OMP_PROFILE PI_PROFILE
export GLEN_HOME="$ACC_S/home"
agent_dir=$(jq -r '.agent_dir // empty' "$MANIFEST")
if [[ -n $agent_dir ]]; then export PI_CODING_AGENT_DIR="$agent_dir"; fi
now_ms() { perl -MTime::HiRes=time -e 'printf "%.0f",time*1000'; }
argv=$(jq -cn --args '$ARGS.positional' -- "$@")
refuse() {
    jq -cn --argjson argv "$argv" --arg why "$1" '{t:"guard_refused",argv:$argv,reason:$why}' >> "$LOG" 2>/dev/null || :
    printf 'guard refused: %s\n' "$1" >&2
    exit 99
}
command_name='' target='' cwd='' dry=false project=false
args=("$@")
i=0
while ((i < ${#args[@]})); do
    arg=${args[i]}
    case "$arg" in
        --cwd) ((i+=1)); cwd=${args[i]-} ;;
        --cwd=*) cwd=${arg#*=} ;;
        --as|--wait|--timeout|--instructions|--provider|--profile|--color|--model|-n|--limit|--session) ((i+=1)) ;;
        --dry-run) dry=true ;;
        --project) project=true ;;
        --) if [[ -n $command_name && -z $target ]]; then ((i+=1)); target=${args[i]-}; fi; break ;;
        -*) ;;
        *) if [[ -z $command_name ]]; then command_name=$arg; elif [[ -z $target ]]; then target=$arg; fi ;;
    esac
    ((i+=1))
done
case "$command_name" in
    install-extension) $dry || refuse 'install-extension would change omp configuration' ;;
    install-skill) $project || refuse 'user skill installation is outside the scratch project'; [[ $(pwd -P) == "$ACC_S"/* ]] || refuse 'project install cwd is outside scratch' ;;
    send|abort|stop|models|model|thinking|compact)
        # Canonical cwd check rejects ../ escapes and directory symlinks out of scratch.
        canonical=''
        if [[ -n $cwd ]]; then canonical=$(cd -- "$cwd" 2>/dev/null && pwd -P) || refuse 'cwd does not exist'; fi
        case "$target" in
            @new) [[ $canonical == "$ACC_S"/* ]] || refuse '@new requires a scratch --cwd' ;;
            @latest) [[ $canonical == "$ACC_S/project" ]] || refuse '@latest requires --cwd project' ;;
            *)
                normalized=$(printf '%s' "$target" | tr '[:upper:]' '[:lower:]' | tr -d '-')
                [[ $normalized =~ ^[0-9a-f]{4,}$ ]] || refuse 'target is not a manifest handle'
                jq -e --arg t "$normalized" '[.R,.L,.X] | map(select((.id|ascii_downcase|gsub("-";""))|endswith($t))) | length == 1' "$MANIFEST" >/dev/null || refuse 'target is outside manifest'
                ;;
        esac
        ;;
esac
start=$(now_ms)
# A unique call id pairs records even if an agent starts overlapping calls.
call_id="$$-$start"
jq -cn --arg id "$call_id" --argjson argv "$argv" --argjson ts "$start" '{t:"start",id:$id,argv:$argv,ts:$ts}' >> "$LOG" 2>/dev/null || :
tmp=$(mktemp "$ACC_S/call.XXXXXX") || exit 1
"$REAL" "$@" | tee "$tmp"
status=${PIPESTATUS[0]}
end=$(now_ms)
# Capping is for the recording only, never for the tool's actual output.
head -c 262144 "$tmp" > "$tmp.capped"
jq -cn --arg id "$call_id" --argjson argv "$argv" --argjson exit "$status" --argjson ms "$((end-start))" --rawfile text "$tmp.capped" \
    '{t:"end",id:$id,argv:$argv,exit:$exit,ms:$ms,stdout:(try ($text|fromjson) catch $text)}' >> "$LOG" 2>/dev/null || :
rm -f "$tmp" "$tmp.capped"
exit "$status"
