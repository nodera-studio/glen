#!/usr/bin/env bash
set -euo pipefail

# Match command strings, not a full shell grammar. Keep exceptions within the same
# simple command so a profile flag on a different command cannot authorize a write.
input=$(cat)
cmd=$(printf '%s\n' "$input" | jq -r '.tool_input.command // ""')
profile_re='(^|[[:space:]])--profile(=|[[:space:]]+)([^[:space:]]+)'

printf '%s\n' "$cmd" | tr ';|&`()' '\n' | tr -d "\"'" |
  while IFS= read -r simple; do
    if ! printf '%s\n' "$simple" | grep -qE '(^|[[:space:]/])omp[[:space:]]+(-[^[:space:]]+[[:space:]]+([^[:space:]]+[[:space:]]+)?)*(config[[:space:]]+(set|reset)|login|logout|plugins?|install|setup|skills?|update)([[:space:]]|$)' &&
      ! printf '%s\n' "$simple" | grep -qE '(^|[[:space:]/])glen[[:space:]]+(-[^[:space:]]+[[:space:]]+([^[:space:]]+[[:space:]]+)?)*install-extension([[:space:]]|$)'; then
      continue
    fi
    if printf '%s\n' "$simple" | grep -qE '(^|[[:space:]])(OMP_PROFILE|PI_CODING_AGENT_DIR)=' ||
      printf '%s\n' "$simple" | grep -qE '(^|[[:space:]])--dry-run([[:space:]]|$)'; then
      continue
    fi
    profile=''
    rest="$simple"
    while [[ "$rest" =~ $profile_re ]]; do
      profile="${BASH_REMATCH[3]}"
      rest="${rest#*"${BASH_REMATCH[0]}"}"
    done
    if [ -n "$profile" ] && [ "$profile" != default ]; then
      continue
    fi
    cat >&2 <<'EOF'
This would write the owner's default omp profile (~/.omp/agent). Harness omp work uses --profile glen-harness (.claude/skills/omp/SKILL.md); otherwise ask the owner to run it.
EOF
    exit 2
  done
exit 0
