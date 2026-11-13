#!/bin/sh
# Recording fake for `hc` and `herdr-app`, used by tests/coord_cli.rs.
# Install: copy this file into a temp dir twice, as `hc` and `herdr-app`,
# chmod 0755, and point HERDR_KIT_HC / HERDR_KIT_HERDR_APP at the copies.
#
# FAKE_DIR (required) holds, per program name (`hc` or `herdr-app`):
#   <name>/calls.tsv          one line per call: HC_TERMINAL_ID=<value|<unset>>, then each argv item, TAB-separated
#   <name>/responses/<key>/   queued responses, consumed in file-name order:
#       NNN.out               printed to stdout
#       NNN.code              optional exit code (default 0)
#       NNN.err               optional stderr text
#   <name>/responses/<key>.default.out / .code   used when the queue is empty
# <key> is the first argument (`run`, `get`, `ls`, `send`, `events`, ...);
# for `herdr-app do <command> ...` it is `do.<command>`.
# A call without any queued or default response prints `{}` and exits 0.
set -eu
name=$(basename "$0")
dir="${FAKE_DIR:?FAKE_DIR is not set}/$name"
mkdir -p "$dir/responses"
{
  printf 'HC_TERMINAL_ID=%s' "${HC_TERMINAL_ID-<unset>}"
  for a in "$@"; do printf '\t%s' "$a"; done
  printf '\n'
} >> "$dir/calls.tsv"
key="${1:-none}"
if [ "$key" = "do" ]; then key="do.${2:-none}"; fi
queue="$dir/responses/$key"
next=""
if [ -d "$queue" ]; then
  next=$(ls "$queue" | grep '\.out$' | sort | head -n 1 || true)
fi
if [ -n "$next" ]; then
  base="$queue/${next%.out}"
  cat "$base.out"
  [ -f "$base.err" ] && cat "$base.err" >&2
  code=0
  [ -f "$base.code" ] && code=$(cat "$base.code")
  rm -f "$base.out" "$base.code" "$base.err"
  exit "$code"
fi
if [ -f "$queue.default.out" ]; then
  cat "$queue.default.out"
  code=0
  [ -f "$queue.default.code" ] && code=$(cat "$queue.default.code")
  exit "$code"
fi
printf '{}\n'
