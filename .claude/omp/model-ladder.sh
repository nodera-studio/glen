# shellcheck shell=bash disable=SC2034 # Sourced; the arrays are read by setup.
# Model/thinking ladder, sourced by omp-profile-setup.sh. Each entry is a
# provider/model id with its own startup thinking level (fix cycle 4, item 11).
# Setup persists the chosen pair as "<id>:<level>"; the engine reads the level
# back from that suffix, never from this ladder (fix cycle 6, MED-2). Owner
# decision (2026-11-01): plain cursor/grok-4.7 at high (not the -fast or -500k
# variants); the Gemini entries were removed because Cursor dropped Gemini.
# Owner decision (2026-11-09): Cursor is retired as an agent (Bugbot only), so
# the profile runs on the owner's openai-codex sign-in. Every entry shares one
# provider: setup and the engine take it from the first entry.
MODEL_LADDER=(openai-codex/gpt-6.1-sol)
THINKING_LADDER=(high)
