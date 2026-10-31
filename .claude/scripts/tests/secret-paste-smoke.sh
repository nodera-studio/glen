#!/usr/bin/env bash
# Smoke test for .claude/hooks/secret-paste-warn.sh.
#
# Every token is built at runtime from repeated characters, so no literal secret-shaped
# string is in the repo for gitleaks to flag. Checks:
#   - each label blocks, names the label, and stdout does not contain the token;
#   - benign text (including `task-…` prose) gives no output and exit 0;
#   - `[allow-secret] <token>` gives no output;
#   - malformed JSON and an empty payload give exit 0 and no output.
#
# Usage: .claude/scripts/tests/secret-paste-smoke.sh   (needs jq)
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
HOOK="$HERE/../../hooks/secret-paste-warn.sh"
command -v jq >/dev/null 2>&1 || { echo "skip: jq not installed"; exit 0; }

fails=0
rep() { printf "%${2}s" '' | tr ' ' "$1"; }

run_hook() { jq -cn --arg p "$1" '{hook_event_name: "UserPromptSubmit", prompt: $p}' | "$HOOK"; }

expect_block() {
  local label=$1 tok=$2 prompt=$3 out rc
  out=$(run_hook "$prompt"); rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "FAIL [$label]: exit $rc"; fails=$((fails + 1)); return
  fi
  if ! printf '%s' "$out" | jq -e '.decision == "block" and .hookSpecificOutput.hookEventName == "UserPromptSubmit" and .hookSpecificOutput.suppressOriginalPrompt == true' >/dev/null 2>&1; then
    echo "FAIL [$label]: no block decision"; fails=$((fails + 1)); return
  fi
  if ! printf '%s' "$out" | jq -e --arg l "$label" '.reason | contains("contains a " + $l + " (") and contains("[allow-secret]")' >/dev/null 2>&1; then
    echo "FAIL [$label]: reason does not name the label"; fails=$((fails + 1)); return
  fi
  if printf '%s' "$out" | grep -qF -- "$tok"; then
    echo "FAIL [$label]: token echoed on stdout"; fails=$((fails + 1)); return
  fi
  echo "ok   block: $label"
}

expect_silent() {
  local name=$1 out rc
  out=$(printf '%s' "$2" | "$HOOK"); rc=$?
  if [ "$rc" -ne 0 ] || [ -n "$out" ]; then
    echo "FAIL [$name]: exit $rc, output ${#out} bytes"; fails=$((fails + 1)); return
  fi
  echo "ok   pass:  $name"
}

A36=$(rep A 36)
GHP="ghp_${A36}"
cases=(
  "GitHub token|$GHP"
  "GitHub token|gho_${A36}"
  "GitHub token|ghu_${A36}"
  "GitHub token|ghs_${A36}"
  "GitHub token|ghr_${A36}"
  "GitHub fine-grained token|github_pat_$(rep B 64)"
  "Anthropic API key|sk-ant-$(rep C 40)"
  "OpenAI API key|sk-proj-$(rep D 40)"
  "OpenAI API key|sk-$(rep E 40)"
  "AWS access key ID|AKIA$(rep F 16)"
  "Slack token|xoxb-$(rep 1 12)-$(rep G 20)"
  "Google API key|AIza$(rep H 35)"
  "Stripe live key|sk_live_$(rep J 24)"
  "Stripe live key|rk_live_$(rep K 24)"
  "private key|-----BEGIN RSA PRIVATE KEY-----"
  "private key|-----BEGIN PRIVATE KEY-----"
  "JWT|eyJ$(rep L 20).eyJ$(rep M 20).$(rep N 30)"
)
for c in "${cases[@]}"; do
  label=${c%%|*}; tok=${c#*|}
  expect_block "$label" "$tok" "here is the value: $tok thanks"
done
# Multi-line prompt, token on a later line.
expect_block "GitHub token" "$GHP" "line one
token=$GHP"

expect_silent "benign text" "$(jq -cn '{prompt: "please refactor the task-management-system-for-teams module"}')"
expect_silent "short prefix" "$(jq -cn '{prompt: "the ghp_ prefix and AKIA prefix are GitHub and AWS"}')"
expect_silent "allow-secret" "$(jq -cn --arg p "[allow-secret] use $GHP" '{prompt: $p}')"
expect_silent "allow-secret, leading space" "$(jq -cn --arg p "  [allow-secret] $GHP" '{prompt: $p}')"
expect_silent "malformed JSON" '{"prompt": "unterminated'
expect_silent "empty payload" ''
expect_silent "no prompt field" '{"hook_event_name":"UserPromptSubmit"}'

if [ "$fails" -gt 0 ]; then
  echo "secret-paste-smoke: $fails failure(s)"; exit 1
fi
echo "secret-paste-smoke: all passed"
