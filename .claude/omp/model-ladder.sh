# shellcheck shell=bash disable=SC2034 # Sourced; the arrays are read by setup.
# Model/thinking ladder, sourced by omp-profile-setup.sh. Each entry is a
# Cursor model id with its own startup thinking level (fix cycle 4, item 11).
# Setup persists the chosen pair as "<id>:<level>"; the engine reads the level
# back from that suffix, never from this ladder (fix cycle 6, MED-2). Owner
# decision: gemini-4-argon at medium, falling back to gemini-3.8-flash at
# medium, then plain grok-4.7 at high (not the -fast or -500k variants).
MODEL_LADDER=(cursor/gemini-4-argon cursor/gemini-3.8-flash cursor/grok-4.7)
THINKING_LADDER=(medium medium high)
