#!/usr/bin/env bash
# Claude Code status line script
# Input: JSON via stdin

input=$(cat)

# Colors: values get color, labels get dim grey
DIM='\033[0;90m'
HEALTHY='\033[38;2;0;210;210m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
RED='\033[0;31m'
RESET='\033[0m'

# Teal/yellow/red for percentage thresholds (context, rate limit)
color_for_pct() {
  local b
  b=$(awk -v p="$1" 'BEGIN { if (p < 50) print "healthy"; else if (p <= 80) print "yellow"; else print "red" }')
  case "$b" in
    healthy) printf '%s' "$HEALTHY" ;;
    yellow) printf '%s' "$YELLOW" ;;
    red)    printf '%s' "$RED" ;;
  esac
}

# Format a rate-limit resets_at (epoch seconds OR ISO-8601) as a local time string.
# Echoes nothing on failure — caller renders an empty parenthetical (fail-soft).
human_time() { # human_time <value> <strftime-fmt>
  local v="$1" f="$2"
  case "$v" in
    '') return 0 ;;
    *[!0-9]*) date -d "$v" "$f" 2>/dev/null || date -j -f '%Y-%m-%dT%H:%M:%SZ' "$v" "$f" 2>/dev/null ;; # ISO: GNU || BSD
    *) date -d "@$v" "$f" 2>/dev/null || date -r "$v" "$f" 2>/dev/null ;;                                # epoch: GNU || BSD
  esac
}

# Extract fields
model=$(echo "$input" | jq -r '.model.display_name // empty')
agent=$(echo "$input" | jq -r '.agent.name // empty')
ctx_pct=$(echo "$input" | jq -r '.context_window.used_percentage // empty')
five_pct=$(echo "$input" | jq -r '.rate_limits.five_hour.used_percentage // empty')
duration_ms=$(echo "$input" | jq -r '.cost.total_duration_ms // empty')

# --- Build output ---
parts=()

# Model — dim label
[ -n "$model" ] && parts+=("${DIM}${model}${RESET}")

# Agent — dim label (only when present)
[ -n "$agent" ] && parts+=("${DIM}${agent}${RESET}")

# Context % — cyan < 50%, red >= 50% (stricter than rate limit)
if [ -n "$ctx_pct" ]; then
  ctx_int=$(printf '%.0f' "$ctx_pct")
  ctx_color=$(awk -v p="$ctx_pct" 'BEGIN { print (p < 50) ? "healthy" : "red" }')
  case "$ctx_color" in
    healthy) ctx_color="$HEALTHY" ;;
    red)     ctx_color="$RED" ;;
  esac
  parts+=("${ctx_color}${ctx_int}%${RESET} ${DIM}context${RESET}")
fi

# 5h rate limit — value colored, label dim, reset time
five_resets=$(echo "$input" | jq -r '.rate_limits.five_hour.resets_at // empty')
if [ -n "$five_pct" ]; then
  five_int=$(printf '%.0f' "$five_pct")
  five_color=$(color_for_pct "$five_pct")
  reset_str=""
  if [ -n "$five_resets" ]; then
    five_ht=$(human_time "$five_resets" '+%H:%M')
    [ -n "$five_ht" ] && reset_str=" ${DIM}(resets ${five_ht})${RESET}"
  fi
  parts+=("${DIM}session limit 5h:${RESET} ${five_color}${five_int}%${RESET}${reset_str}")
fi

# 7d (weekly) rate limit — value colored, label dim, reset day+time
seven_pct=$(echo "$input" | jq -r '.rate_limits.seven_day.used_percentage // empty')
seven_resets=$(echo "$input" | jq -r '.rate_limits.seven_day.resets_at // empty')
if [ -n "$seven_pct" ]; then
  seven_int=$(printf '%.0f' "$seven_pct")
  seven_color=$(color_for_pct "$seven_pct")
  sreset_str=""
  if [ -n "$seven_resets" ]; then
    seven_ht=$(human_time "$seven_resets" '+%a %H:%M')
    [ -n "$seven_ht" ] && sreset_str=" ${DIM}(resets ${seven_ht})${RESET}"
  fi
  parts+=("${DIM}weekly 7d:${RESET} ${seven_color}${seven_int}%${RESET}${sreset_str}")
fi

# Lines — branch-level stats via git diff against merge base
branch_dir=$(echo "$input" | jq -r '.workspace.project_dir // .cwd // empty')
if [ -n "$branch_dir" ]; then
  git_stat=$(git -C "$branch_dir" diff --numstat $(git -C "$branch_dir" merge-base HEAD main 2>/dev/null || echo HEAD~1) 2>/dev/null | grep -v -E '(package-lock\.json|\.lock$|dist/|\.next/)' | awk '{a+=$1; d+=$2} END {print a" "d}')
  la=$(echo "$git_stat" | cut -d' ' -f1)
  lr=$(echo "$git_stat" | cut -d' ' -f2)
  if [ -n "$la" ] && [ -n "$lr" ] && [ "$la" != "0" -o "$lr" != "0" ]; then
    parts+=("${GREEN}+${la}${RESET} ${DIM}added${RESET} ${RED}-${lr}${RESET} ${DIM}deleted${RESET}")
  fi
fi

# Session cost (USD) — dim
cost_usd=$(echo "$input" | jq -r '.cost.total_cost_usd // empty')
if [ -n "$cost_usd" ]; then
  cost_fmt=$(awk -v c="$cost_usd" 'BEGIN { printf "%.2f", c }')
  parts+=("${DIM}\$${cost_fmt}${RESET}")
fi

# Duration — dim
if [ -n "$duration_ms" ]; then
  total_sec=$(awk -v ms="$duration_ms" 'BEGIN { printf "%d", ms / 1000 }')
  if [ "$total_sec" -ge 3600 ]; then
    hours=$((total_sec / 3600))
    mins=$(( (total_sec % 3600) / 60 ))
    [ "$mins" -eq 0 ] && d="${hours}h" || d="${hours}h${mins}m"
  elif [ "$total_sec" -ge 60 ]; then
    d="$((total_sec / 60))m"
  else
    d="${total_sec}s"
  fi
  parts+=("${DIM}${d}${RESET}")
fi

# Join with dim " | "
output=""
for part in "${parts[@]}"; do
  [ -z "$output" ] && output="$part" || output="${output} ${DIM}|${RESET} ${part}"
done

printf '%b\n' "$output"
