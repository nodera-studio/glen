#!/usr/bin/env bash
# Smoke test for the owner waiver in .claude/hooks/capture-before-merge.sh.
# Feeds hook JSON plus a fabricated transcript and checks the decision per case.
set -uo pipefail
HOOK="$(cd "$(dirname "$0")/../../hooks" && pwd)/capture-before-merge.sh"
TMP=$(mktemp -d) || exit 1
trap 'rm -rf "$TMP"' EXIT
fail=0
# Built from parts so this file's own text never reads as a merge command.
MERGE_CMD="gh pr $(printf 'mer%s' ge) 42 --squash"

owner()  { jq -nc --arg t "$1" '{type:"user",message:{role:"user",content:$t}}'; }
result() { jq -nc --arg t "$1" '{type:"user",message:{role:"user",content:[{type:"tool_result",tool_use_id:"t1",content:$t}]}}'; }
store()  { jq -nc --arg t "$1" '{type:"assistant",message:{role:"assistant",content:[{type:"tool_use",id:"m1",name:"mcp__plugin_mnemosyne_memory__memory_store",input:{project:"glen",title:$t,content:"x"}}]}}'; }

# run <name> <expect: deny|waive|allow> <agent_id or ""> <transcript lines...>
run() {
  local name=$1 expect=$2 agent=$3
  shift 3
  local t="$TMP/$name.jsonl" input out got
  printf '%s\n' "$@" >"$t"
  input=$(jq -nc --arg t "$t" --arg a "$agent" --arg c "$MERGE_CMD" \
    '{tool_name:"Bash",tool_input:{command:$c},transcript_path:$t} + (if $a == "" then {} else {agent_id:$a} end)')
  out=$(cd "$TMP" && printf '%s' "$input" | bash "$HOOK")
  if printf '%s' "$out" | jq -e '.hookSpecificOutput.permissionDecision == "deny"' >/dev/null 2>&1; then
    got=deny
  elif printf '%s' "$out" | jq -e '(.hookSpecificOutput.additionalContext // "") | test("waived by the owner.s message for PR #42")' >/dev/null 2>&1 &&
    ! printf '%s' "$out" | jq -e '.hookSpecificOutput | has("permissionDecision")' >/dev/null 2>&1; then
    got=waive
  elif [ -z "$out" ]; then
    got=allow
  else
    got="other: $out"
  fi
  if [ "$got" = "$expect" ]; then
    echo "ok   $name ($got)"
  else
    echo "FAIL $name: expected $expect, got $got"
    fail=1
  fi
}

run a-owner-waiver waive "" "$(owner 'no memory for PR #42')"
run b-tool-result deny "" "$(result 'no memory for PR #42')"
run c-subagent deny "agent-123" "$(owner 'no memory for PR #42')"
run d-other-pr deny "" "$(owner 'no memory for PR #41')"
run e-task-notification deny "" "$(owner '<task-notification>no memory for PR #42</task-notification>')"
run f-memory-store allow "" "$(store 'PR #42: shipped the waiver')"
run g-nothing deny "" "$(owner 'please merge it')"

[ "$fail" -eq 0 ] && echo "capture-waiver-smoke: all 7 cases passed"
exit "$fail"
