#!/bin/sh
set -eu

repo="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)"
state="$repo/.work/linux-host"
machine="${SMOLRUNNERS_HOST_NAME:-sr-dev}"
darwin="$repo/.work/runtime/darwin-arm64"
linux_runtime="$repo/.work/runtime/linux-arm64"
cpus="${SMOLRUNNERS_LINUX_HOST_CPUS:-6}"
memory="${SMOLRUNNERS_LINUX_HOST_MEMORY_MIB:-12288}"

usage() {
  echo "usage: scripts/linux-host.sh up | sync | exec COMMAND... | status | down | destroy" >&2
  exit 2
}

fail() { printf 'linux-host: %s\n' "$*" >&2; exit 1; }

test "$(uname -sm)" = "Darwin arm64" || fail "the nested Linux host runs on an Apple Silicon Mac"
git -C "$repo" check-ignore -q -- .work/tmp || fail ".work must be ignored through .git/info/exclude"
mkdir -p "$state/home/tmp" "$state/home/.config" "$state/home/.cache" "$state/home/.local/share"

native() {
  env -i PATH="$PATH" HOME="$state/home" TMPDIR="$state/home/tmp" \
    XDG_CONFIG_HOME="$state/home/.config" XDG_CACHE_HOME="$state/home/.cache" \
    XDG_DATA_HOME="$state/home/.local/share" \
    "$darwin/smolvm" "$@"
}

guest() {
  native machine exec --name "$machine" --timeout "${SMOLRUNNERS_LINUX_HOST_TIMEOUT:-3h}" "$@"
}

in_guest() {
  guest -- sh -eu -c "export PATH=/opt/node/bin:/opt/bin:\$PATH HOME=/root TMPDIR=/work/tmp; mkdir -p /work/tmp; cd /work/smolrunners 2>/dev/null || cd /; $*"
}

exists() { native machine ls 2>/dev/null | grep -q "^$machine\b"; }
running() { native machine status --name "$machine" 2>/dev/null | grep -qi running; }

provision() {
  guest -- sh -eu -c '
    export DEBIAN_FRONTEND=noninteractive HOME=/root
    test -x /opt/bin/jaq && test -d /opt/runtime/lib && exit 0
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends ca-certificates curl git python3 tar xz-utils e2fsprogs iproute2 procps coreutils findutils >/dev/null
    mkdir -p /opt/bin /work/tmp
  '
}

up() {
  test -x "$darwin/smolvm" || fail "install the macOS runtime: scripts/install-runtime smolvm-darwin-arm64 .work/runtime/darwin-arm64"
  test -f "$linux_runtime/smolvm" || fail "install the Linux runtime: scripts/install-runtime smolvm-linux-arm64 .work/runtime/linux-arm64"
  if ! exists; then
    native machine create --name "$machine" -I debian:13 --nested --net \
      --cpus "$cpus" --mem "$memory" --storage 60 --overlay 20 \
      -v "$repo:/mnt/repo:ro" -v "$linux_runtime:/opt/runtime"
  fi
  running || native machine start --name "$machine"
  guest -- sh -eu -c '
    mkdir -p /work /work/tmp
    mountpoint -q /work && exit 0
    test -d /storage/work || cp -a /work /storage/work
    mount --bind /storage/work /work
  '
  provision
  guest -- sh -c 'test -c /dev/kvm' || fail "the nested guest has no /dev/kvm; the Mac needs M3 or newer on macOS 15+"
}

sync() {
  archive="$state/smolrunners-tree.tar"
  ( cd "$repo" && git ls-files -z --cached --others --exclude-standard --deduplicate |
      tar --no-xattrs --null -T - -cf - ) > "$archive"
  native machine cp "$archive" "$machine:/work/tmp/smolrunners-tree.tar"
  guest -- sh -eu -c 'mkdir -p /work/smolrunners/.work/tmp && tar -C /work/smolrunners -xf /work/tmp/smolrunners-tree.tar && rm -f /work/tmp/smolrunners-tree.tar'
}

case "${1:-}" in
  up) up ;;
  sync) sync ;;
  exec) shift; test "$#" -gt 0 || usage; in_guest "$*" ;;
  status) native machine status --name "$machine" ;;
  down) native machine stop --name "$machine" ;;
  destroy) native machine stop --name "$machine" 2>/dev/null || true; native machine delete --name "$machine" -f ;;
  *) usage ;;
esac
