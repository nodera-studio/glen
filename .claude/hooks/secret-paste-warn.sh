#!/usr/bin/env bash
# UserPromptSubmit — secret-paste warning.
#
# Blocks a prompt that looks like it carries a credential (GitHub, Anthropic, OpenAI, AWS,
# Slack, Google, Stripe, a PEM private key, a JWT), so the value never reaches the model.
# A UserPromptSubmit block's `reason` is shown to the user and not added to context, and
# `suppressOriginalPrompt` keeps the prompt text out of the block message and the transcript.
# Blocking is the only way to warn without the pasted value reaching the model.
#
# The hook never prints, logs or writes the matched value or the prompt: the reason names
# only the label and the match length.
#
# Bypass: start the message with [allow-secret] (leading whitespace is ignored).
#
# Posture: FAIL OPEN (no jq, malformed JSON, no prompt → exit 0, no output); BLOCK only on a
# pattern match.
set -uo pipefail
export LC_ALL=C
command -v jq >/dev/null 2>&1 || exit 0
INPUT=$(cat 2>/dev/null) || exit 0
PROMPT=$(printf '%s' "$INPUT" | jq -r '.prompt // empty' 2>/dev/null) || exit 0
[ -z "$PROMPT" ] && exit 0

# Bypass prefix.
trimmed=${PROMPT#"${PROMPT%%[![:space:]]*}"}
case "$trimmed" in
  '[allow-secret]'*) exit 0 ;;
esac

# Token patterns must start at a word boundary, so `task-…` does not read as `sk-…`. A leading
# space stands in for the start of the prompt.
B='[^A-Za-z0-9_-]'
TEXT=" $PROMPT"

# label|regex; the token is the regex's first capture group. Order matters: specific first.
PATTERNS=(
  "GitHub token|${B}((ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36})"
  "GitHub fine-grained token|${B}(github_pat_[A-Za-z0-9_]{60,})"
  "Anthropic API key|${B}(sk-ant-[A-Za-z0-9_-]{20,})"
  "OpenAI API key|${B}(sk-proj-[A-Za-z0-9_-]{20,})"
  "OpenAI API key|${B}(sk-[A-Za-z0-9_-]{20,})"
  "AWS access key ID|${B}(AKIA[A-Z0-9]{16})"
  "Slack token|${B}(xox[abposr]-[A-Za-z0-9-]{10,})"
  "Google API key|${B}(AIza[A-Za-z0-9_-]{35})"
  "Stripe live key|${B}((sk|rk)_live_[A-Za-z0-9]{20,})"
  "private key|(-----BEGIN [A-Z ]*PRIVATE KEY-----)"
  "JWT|${B}(eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,})"
)

for entry in "${PATTERNS[@]}"; do
  label=${entry%%|*}
  re=${entry#*|}
  if [[ $TEXT =~ $re ]]; then
    n=${#BASH_REMATCH[1]}
    reason="This message looks like it contains a ${label} (${n} characters). It was not sent. If it is real, rotate it; keep secrets in the Mnemosyne secrets store, not chat. To send anyway, start the message with [allow-secret]."
    jq -cn --arg reason "$reason" \
      '{decision: "block", reason: $reason, hookSpecificOutput: {hookEventName: "UserPromptSubmit", suppressOriginalPrompt: true}}' \
      2>/dev/null || exit 0
    exit 0
  fi
done
exit 0
