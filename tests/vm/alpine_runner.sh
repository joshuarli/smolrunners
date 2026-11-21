#!/bin/sh
set -eu

printf '%s\n' \
  'alpine_runner: PARKED per D13 after the L2 spike.' \
  'alpine_runner: the musl runner links libstdc++.so.6 and libgcc_s.so.1, which the pinned Alpine base lacks.' \
  'alpine_runner: the pinned Node is glibc-linked and there is no pinned musl Node.' \
  'alpine_runner: retry after sources.lock.json pins a musl Node plus the musl C++ runtime and ICU.' \
  'alpine_runner: see docs/compatibility.md for the measured evidence.' >&2
exit 2
