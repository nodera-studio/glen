#!/usr/bin/env bash
echo "Reminder: check available skills before starting work."
if [ -f .claude/.orchestrator-mode ]; then
  echo "Orchestrator mode is ON (dispatch-only — see .claude/agents/orchestrator/orchestrator-mode.md). Run /orchestrate off to disable."
fi
exit 0
