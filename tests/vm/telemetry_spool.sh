#!/bin/sh
set -u

spool=${1:?usage: telemetry_spool.sh SPOOL [CAP]}
cap=${2:-20000}
overflow="$spool.overflow"
total=0
reported=0

while IFS= read -r line; do
  case "$line" in
    \{*) ;;
    *) continue ;;
  esac
  printf '%s\n' "$line" >> "$spool"
  total=$((total + 1))
  if [ "$total" -gt "$cap" ]; then
    excess=$((total - cap))
    if [ "$reported" -eq 0 ] || [ $((excess % 200)) -eq 0 ]; then
      tail -n "$cap" "$spool" > "$spool.tmp" && mv "$spool.tmp" "$spool"
      printf 'overflow reason=spool_line_cap cap=%s total_lines=%s\n' "$cap" "$total" > "$overflow"
      reported=1
    fi
  fi
done
