#!/bin/sh
set -eu

rootfs=${1:?usage: build.sh ROOTFS CACHE}
cache=${2:?usage: build.sh ROOTFS CACHE}
recipe_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
repo=$(CDPATH='' cd -- "$recipe_dir/../.." && pwd -P)

fail() { printf 'fixture build: %s\n' "$*" >&2; exit 1; }

sh "$repo/images/debian13/build.sh" "$rootfs" "$cache"

executor="$repo/tests/fixtures/executor"
[ -f "$executor" ] || fail "missing executor: $executor"
install -d "$rootfs/opt/smolrunners/bin" "$rootfs/opt/smolrunners/scripts"
install -m 0755 "$executor" "$rootfs/opt/smolrunners/bin/executor"
cp "$repo/tests/fixtures/scripts/basic.sh" "$rootfs/opt/smolrunners/scripts/basic.sh"

printf 'fixture build: ok\n'
