#!/bin/sh
set -eu

repo="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)"
image='debian@sha256:bfc36aecdece0387418759dba474aa856c026ca8087da3606b41b90ab0509c99'
name='sr-x86-64-host'

usage() {
  echo "usage: scripts/x86-64-host.sh up | exec COMMAND... | down | destroy" >&2
  echo "NE2 only: runs on the owner's native x86_64 KVM host, never on the Mac." >&2
  exit 2
}

fail() { printf 'x86-64-host: %s\n' "$*" >&2; exit 1; }

[ "$(uname -m)" = "x86_64" ] || fail "this script is for a native x86_64 KVM host (NE2), not this machine"
[ -c /dev/kvm ] || fail "/dev/kvm is required"

case "${1:-}" in
  up)
    docker run -d --name "$name" --privileged \
      --device /dev/kvm -v "$repo:/work/smolrunners" -w /work/smolrunners \
      "$image" sleep infinity
    docker exec "$name" sh -c 'apt-get update -qq && apt-get install -y -qq --no-install-recommends ca-certificates curl git python3 tar xz-utils e2fsprogs iproute2 procps build-essential >/dev/null'
    ;;
  exec)
    shift; test "$#" -gt 0 || usage
    docker exec "$name" sh -eu -c "$*"
    ;;
  down) docker stop "$name" ;;
  destroy) docker rm -f "$name" ;;
  *) usage ;;
esac
