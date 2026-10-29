# Claude Code Status Line Setup

Shows model, context usage, rate limit, lines changed, and session duration with color-coded values:

```
Opus 4.6 (1M context) | 17% context | session limit 5h: 33% (resets 14:30) | +580 added -197 deleted | 5h37m
```

**Colors:**
- **Cyan** (RGB 0,210,210): healthy values (context < 50%, rate limit < 50%)
- **Yellow**: rate limit warning (50-80%)
- **Red**: context >= 50%, rate limit > 80%, lines deleted
- **Green**: lines added
- **Dim grey**: all labels, pipes, model name, duration

Agent name appears automatically when an agent is active (e.g. `impl:verifier`).

---

## macOS / Linux

### Step 1 — Save the script

Save to `~/.claude/scripts/statusline.sh` and make it executable:

```bash
chmod +x ~/.claude/scripts/statusline.sh
```

```bash
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

# Cyan/yellow/red for percentage thresholds (context, rate limit)
color_for_pct() {
  local b
  b=$(awk -v p="$1" 'BEGIN { if (p < 50) print "healthy"; else if (p <= 80) print "yellow"; else print "red" }')
  case "$b" in
    healthy) printf '%s' "$HEALTHY" ;;
    yellow) printf '%s' "$YELLOW" ;;
    red)    printf '%s' "$RED" ;;
  esac
}

# Extract fields
model=$(echo "$input" | jq -r '.model.display_name // empty')
agent=$(echo "$input" | jq -r '.agent.name // empty')
ctx_pct=$(echo "$input" | jq -r '.context_window.used_percentage // empty')
five_pct=$(echo "$input" | jq -r '.rate_limits.five_hour.used_percentage // empty')
five_resets=$(echo "$input" | jq -r '.rate_limits.five_hour.resets_at // empty')
duration_ms=$(echo "$input" | jq -r '.cost.total_duration_ms // empty')

# --- Build output ---
parts=()

# Model — dim label
[ -n "$model" ] && parts+=("${DIM}${model}${RESET}")

# Agent — dim label (only when present)
[ -n "$agent" ] && parts+=("${DIM}${agent}${RESET}")

# Context % — cyan < 50%, red >= 50%
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
if [ -n "$five_pct" ]; then
  five_int=$(printf '%.0f' "$five_pct")
  five_color=$(color_for_pct "$five_pct")
  reset_str=""
  if [ -n "$five_resets" ]; then
    reset_str=" ${DIM}(resets $(date -r "$five_resets" '+%H:%M' 2>/dev/null || date -d "@$five_resets" '+%H:%M' 2>/dev/null))${RESET}"
  fi
  parts+=("${DIM}session limit 5h:${RESET} ${five_color}${five_int}%${RESET}${reset_str}")
fi

# Lines — branch-level stats via git diff against merge base
branch_dir=$(echo "$input" | jq -r '.workspace.project_dir // .cwd // empty')
if [ -n "$branch_dir" ]; then
  git_stat=$(git -C "$branch_dir" diff --numstat $(git -C "$branch_dir" merge-base HEAD main 2>/dev/null || echo HEAD~1) 2>/dev/null | awk '{a+=$1; d+=$2} END {print a" "d}')
  la=$(echo "$git_stat" | cut -d' ' -f1)
  lr=$(echo "$git_stat" | cut -d' ' -f2)
  if [ -n "$la" ] && [ -n "$lr" ] && [ "$la" != "0" -o "$lr" != "0" ]; then
    parts+=("${GREEN}+${la}${RESET} ${DIM}added${RESET} ${RED}-${lr}${RESET} ${DIM}deleted${RESET}")
  fi
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
```

### Step 2 — Add to settings

Add to `~/.claude/settings.json` (global) or `.claude/settings.json` (project):

```json
{
  "statusLine": {
    "type": "command",
    "command": "~/.claude/scripts/statusline.sh"
  }
}
```

**Requires:** `jq` (`brew install jq` on macOS, `apt install jq` on Linux).

---

## Any platform — Node.js (zero extra installs)

If you already have Node.js (you do if you work on this repo), this version needs nothing else. Works on macOS, Linux, and Windows.

### Step 1 — Save the script

Save to `~/.claude/scripts/statusline.js`:

```javascript
#!/usr/bin/env node
let input = '';
process.stdin.on('data', c => input += c);
process.stdin.on('end', () => {
  const d = JSON.parse(input);

  const DIM = '\x1b[0;90m', GREEN = '\x1b[0;32m', YELLOW = '\x1b[0;33m',
        RED = '\x1b[0;31m', HEALTHY = '\x1b[38;2;0;210;210m', R = '\x1b[0m';

  const pctColor = p => p < 50 ? HEALTHY : p <= 80 ? YELLOW : RED;

  const parts = [];

  // Model
  if (d.model?.display_name) parts.push(`${DIM}${d.model.display_name}${R}`);

  // Agent (only when active)
  if (d.agent?.name) parts.push(`${DIM}${d.agent.name}${R}`);

  // Context % — cyan < 50%, red >= 50%
  const ctx = d.context_window?.used_percentage;
  if (ctx != null) {
    const ctxColor = ctx < 50 ? HEALTHY : RED;
    parts.push(`${ctxColor}${Math.round(ctx)}%${R} ${DIM}context${R}`);
  }

  // 5h rate limit + reset time
  const five = d.rate_limits?.five_hour?.used_percentage;
  const fiveResets = d.rate_limits?.five_hour?.resets_at;
  if (five != null) {
    let reset = '';
    if (fiveResets) {
      const dt = new Date(fiveResets * 1000);
      reset = ` ${DIM}(resets ${dt.getHours().toString().padStart(2,'0')}:${dt.getMinutes().toString().padStart(2,'0')})${R}`;
    }
    parts.push(`${DIM}session limit 5h:${R} ${pctColor(five)}${Math.round(five)}%${R}${reset}`);
  }

  // Lines — branch-level stats via git diff against merge base
  const dir = d.workspace?.project_dir || d.cwd;
  if (dir) {
    try {
      const { execSync } = require('child_process');
      const mb = execSync('git merge-base HEAD main', { cwd: dir, encoding: 'utf8', stdio: ['pipe','pipe','ignore'] }).trim();
      const stat = execSync(`git diff --numstat ${mb}`, { cwd: dir, encoding: 'utf8', stdio: ['pipe','pipe','ignore'] });
      let la = 0, lr = 0;
      stat.trim().split('\n').filter(Boolean).forEach(l => { const [a,d] = l.split('\t'); la += +a||0; lr += +d||0; });
      if (la || lr) parts.push(`${GREEN}+${la}${R} ${DIM}added${R} ${RED}-${lr}${R} ${DIM}deleted${R}`);
    } catch {}
  }

  // Duration
  const ms = d.cost?.total_duration_ms;
  if (ms != null) {
    const s = Math.floor(ms / 1000);
    const dur = s >= 3600 ? `${Math.floor(s/3600)}h${Math.floor((s%3600)/60)}m`
              : s >= 60   ? `${Math.floor(s/60)}m`
              :             `${s}s`;
    parts.push(`${DIM}${dur}${R}`);
  }

  console.log(parts.join(` ${DIM}|${R} `));
});
```

On macOS/Linux, make it executable: `chmod +x ~/.claude/scripts/statusline.js`

### Step 2 — Add to settings

```json
{
  "statusLine": {
    "type": "command",
    "command": "node ~/.claude/scripts/statusline.js"
  }
}
```

On Windows, use the full path:

```json
{
  "statusLine": {
    "type": "command",
    "command": "node C:/Users/<username>/.claude/scripts/statusline.js"
  }
}
```

**Requires:** Node.js (already installed if you work on this repo). Works on all platforms.

**Note:** Windows Terminal supports true color (RGB). Legacy cmd.exe does not — colors will be limited.

---

## Troubleshooting

- **Status line blank:** Check script is executable (`chmod +x`) and outputs to stdout
- **Colors look wrong:** ANSI palette colors vary by terminal theme. The RGB true color (`\033[38;2;R;G;Bm`) bypasses theme mapping — adjust RGB values if needed
- **Values show `--`:** Fields are null before the first API response. This is normal
- **Stale after edits:** Status line re-renders on your next interaction with Claude, not immediately after script edits
