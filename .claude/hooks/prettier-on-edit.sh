#!/usr/bin/env bash
# PostToolUse (Edit|Write): format the edited file with prettier.
# Warn-don't-block posture — mirrors rebuild-shared.sh. Never exits non-zero.
set -uo pipefail

input=$(cat)
file=$(echo "$input" | jq -r '.tool_input.file_path // .tool_input.path // ""')

case "$file" in
  *.ts|*.tsx|*.js|*.jsx|*.json|*.css|*.scss)
    if [ -f "$file" ]; then
      pnpm exec prettier --write "$file" >/dev/null 2>&1 \
        || echo "Warning: prettier failed on $file (continuing)"
    fi
    ;;
esac
exit 0
