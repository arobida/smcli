#!/usr/bin/env bash
# ateam — fan out one brief to any number of agents via kero.
#
# You pick the agents (see AGENTS.md) and pass them as "alias=command" strings.
# Agents start in parallel in the repo dir, each in its own kero session, the
# brief is delivered once each prompt is ready, and progress is reported to
# kero as it goes.
#
# Usage (bash only; from zsh run it, don't source it):
#   bash scripts/ateam.sh <brief> <alias=command>...
#
# <brief> is one of:
#   ""                    start only, send nothing
#   "inline text"         send that text to every agent
#   /path/to/task.md      send "Read /path/to/task.md and do exactly what it says."
#   /path/to/taskdir/     per-agent: send "Read /path/to/taskdir/<alias>.task ..."
#                         (agents whose <alias>.task is missing are started, not briefed)
#
# Examples:
#   bash scripts/ateam.sh "write the tests" \
#     "pikcode=pi --model opencode-go/kimi-k2.7-code --thinking high" \
#     "piluna=pi --model openai/gpt-6-luna --thinking low"
#
#   bash scripts/ateam.sh "$D" \
#     "pisol=pi --model openai/gpt-6.1-sol --thinking low" \
#     "pigrok47=pi --model xai/grok-4.7 --thinking high"
#
# Prints "<alias>\t<session-id>" per started agent on stdout.
#
# Env overrides: ATEAM_SHELL (default zsh), ATEAM_DIR (default $PWD).

if [ -z "${BASH_VERSION:-}" ]; then
  echo "ateam.sh needs bash: run 'bash scripts/ateam.sh <brief> <alias=command>...'" >&2
  return 2 2>/dev/null || exit 2
fi

set -euo pipefail

# A TUI can report idle before its input accepts Enter: resend Enter until the agent starts working.
submit() {
  local t="$1" n
  kero send "$t" "$2" --enter >/dev/null
  for n in 1 2 3 4 5; do
    sleep 2
    kero status "$t" --json | grep -q '"working"' && return 0
    kero key "$t" enter >/dev/null
  done
  echo "dispatch: $t did not start working after its brief" >&2
}

dispatch() {
  local brief="${1:-}"
  shift

  if [ $# -eq 0 ]; then
    echo "usage: dispatch <brief> <alias=command>..." >&2
    echo "  e.g. dispatch 'write X' 'pikcode=pi --model opencode-go/kimi-k2.7-code --thinking high'" >&2
    return 2
  fi

  local total=$# started=0 failed=0
  local -a targets=() names=()
  local spec alias cmd t
  local shell_bin="${ATEAM_SHELL:-zsh}" dir="${ATEAM_DIR:-$PWD}"

  # Phase 1: start every agent (no waiting between starts, so they load in parallel)
  for spec in "$@"; do
    alias="${spec%%=*}"
    cmd="${spec#*=}"
    if [ -z "$alias" ] || [ -z "$cmd" ]; then
      echo "dispatch: bad agent spec '$spec' (want alias=command)" >&2
      failed=$((failed + 1))
      continue
    fi
    kero report working "Starting $alias" --progress $(((started + failed) * 100 / total)) || true
    if t=$(kero new -n "$alias" -d "$dir" -- "$shell_bin" -ic "$cmd"); then
      targets+=("$t"); names+=("$alias"); started=$((started + 1))
    else
      kero report error "Failed to start $alias" || true
      failed=$((failed + 1))
    fi
  done
  [ "$started" -gt 0 ] || { echo "dispatch: no agents started" >&2; return 1; }

  # Phase 2: wait for each to be ready (they load concurrently)
  for t in "${targets[@]}"; do
    kero wait "$t" --timeout 5m >/dev/null \
      || { kero report error "Agent $t never became ready" || true; failed=$((failed + 1)); }
  done

  # Phase 3: brief each and print <alias>\t<id>
  local i=0 name msg f
  for t in "${targets[@]}"; do
    name="${names[$i]}"; i=$((i + 1))
    if [ -z "$brief" ]; then
      msg=""
    elif [ -d "$brief" ]; then
      f="$brief/$name.task"
      if [ ! -f "$f" ]; then
        echo "dispatch: no task file $f for $name (started, not briefed)" >&2
        continue
      fi
      msg="Read $f and do exactly what it says."
    elif [ -f "$brief" ]; then
      msg="Read $brief and do exactly what it says."
    else
      msg="$brief"
    fi
    [ -z "$msg" ] || submit "$t" "$msg"
    printf '%s\t%s\n' "$name" "$t"
  done

  if [ "$failed" -gt 0 ]; then
    kero report error "Dispatched $started agent(s), $failed failed" || true
    return 1
  fi
  kero report done "Dispatched $started agent(s)" || true
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  dispatch "$@"
fi
