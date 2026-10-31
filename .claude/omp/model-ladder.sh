# Shared model/thinking ladder, sourced by omp-profile-setup.sh and
# omp-engine.sh. One file keeps both scripts' selections in sync: each
# entry is a Cursor model id with its own startup thinking level (fix
# cycle 4, item 11). Owner decision: gemini-4-argon at medium, falling
# back to gemini-3.8-flash at medium, then plain grok-4.7 at high (not
# the -fast or -500k variants).
MODEL_LADDER=(cursor/gemini-4-argon cursor/gemini-3.8-flash cursor/grok-4.7)
THINKING_LADDER=(medium medium high)
# Prints the thinking level paired with $1 in the ladder, with exit 0. For an
# off-ladder id (an explicit --model, fix cycle 5 decision 7), prints nothing
# and exits 1: the engine passes no --thinking at all for those, instead of
# guessing a level that was never configured for that model.
thinking_level_for() {
  local model="$1" i=0
  while [ "$i" -lt "${#MODEL_LADDER[@]}" ]; do
    if [ "${MODEL_LADDER[$i]}" = "$model" ]; then
      printf '%s\n' "${THINKING_LADDER[$i]}"
      return 0
    fi
    i=$((i + 1))
  done
  return 1
}
