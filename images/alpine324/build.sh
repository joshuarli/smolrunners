#!/bin/sh
set -eu

rootfs=${1:?usage: build.sh ROOTFS CACHE}
cache=${2:?usage: build.sh ROOTFS CACHE}
recipe_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
repo=$(CDPATH='' cd -- "$recipe_dir/../.." && pwd -P)
manifest_src="$recipe_dir/manifest.json"

fail() { printf 'image build: %s\n' "$*" >&2; exit 1; }
pin() { sed -n "s/.*\"$1\": *\"\\([^\"]*\\)\".*/\\1/p" "$manifest_src" | head -1; }

for tool in curl python3 tar gzip; do
  command -v "$tool" >/dev/null 2>&1 || fail "missing tool: $tool"
done

tmp="$cache/build-tmp"
mkdir -p "$tmp" "$rootfs"

base_digest=$(pin base_digest)
[ -n "$base_digest" ] || fail 'manifest lacks base_digest'

pull_base() {
  registry=https://registry-1.docker.io
  repository=library/alpine
  token=$(curl -fsSL --max-time 60 \
    "https://auth.docker.io/token?service=registry.docker.io&scope=repository:$repository:pull" |
    python3 -c 'import sys,json; print(json.load(sys.stdin)["token"])')
  curl -fsSL --max-time 120 -H "Authorization: Bearer $token" \
    -H 'Accept: application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json' \
    "$registry/v2/$repository/manifests/$base_digest" -o "$tmp/index.json"
  plat_digest=$(python3 - "$tmp/index.json" <<'PY'
import json
import sys

index = json.load(open(sys.argv[1]))
if "manifests" not in index:
    raise SystemExit("pinned Alpine digest is not a multi-arch index")
for entry in index["manifests"]:
    platform = entry.get("platform", {})
    if platform.get("os") == "linux" and platform.get("architecture") == "arm64":
        print(entry["digest"])
        break
else:
    raise SystemExit("no linux/arm64 manifest in base index")
PY
)
  curl -fsSL --max-time 120 -H "Authorization: Bearer $token" \
    -H 'Accept: application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json' \
    "$registry/v2/$repository/manifests/$plat_digest" -o "$tmp/layers.json"
  python3 - "$tmp/layers.json" > "$tmp/layers.txt" <<'PY'
import json
import sys

manifest = json.load(open(sys.argv[1]))
for layer in manifest["layers"]:
    print(layer["mediaType"], layer["digest"])
PY
  while read -r media_type layer_digest; do
    case "$media_type" in
      *gzip*) decoder='gzip -dc' ;;
      *) decoder=cat ;;
    esac
    curl -fsSL --max-time 300 -H "Authorization: Bearer $token" \
      "$registry/v2/$repository/blobs/$layer_digest" | $decoder | tar -x -C "$rootfs"
  done < "$tmp/layers.txt"
  [ -f "$rootfs/etc/alpine-release" ] || fail 'base rootfs is not Alpine'
  release=$(cat "$rootfs/etc/alpine-release")
  case "$release" in
    3.24.*) ;;
    *) fail "unexpected Alpine release: $release" ;;
  esac
}

fetch_input() {
  name=$1
  sh "$repo/bootstrap/buildlib-run" -- "$repo/scripts/fetch-inputs.py" \
    --name "$name" --cache "$cache/inputs" > "$tmp/$name.path"
  path=$(cut -f2 "$tmp/$name.path" | head -1)
  [ -s "$path" ] || fail "fetch-inputs produced no $name archive"
  printf '%s' "$path"
}

assert_version() {
  label=$1
  expected=$2
  actual=$3
  [ "$actual" = "$expected" ] || fail "$label: expected $expected, got $actual"
}

printf 'image build: pulling base rootfs\n'
pull_base

printf 'image build: fetching pinned inputs\n'
docker_tgz=$(fetch_input docker-static-aarch64)
buildx_bin=$(fetch_input buildx-linux-arm64)
compose_bin=$(fetch_input compose-linux-arm64)

printf 'image build: installing components\n'
mkdir -p "$rootfs/usr/local/bin" \
  "$rootfs/usr/local/lib/docker/cli-plugins" \
  "$rootfs/opt/smolrunners/bin"

mkdir -p "$tmp/docker"
tar -xzf "$docker_tgz" -C "$tmp/docker"
for binary in docker dockerd containerd runc ctr docker-init docker-proxy containerd-shim-runc-v2; do
  [ -f "$tmp/docker/docker/$binary" ] || fail "docker bundle lacks $binary"
  cp "$tmp/docker/docker/$binary" "$rootfs/usr/local/bin/$binary"
  chmod 0755 "$rootfs/usr/local/bin/$binary"
done
cp "$buildx_bin" "$rootfs/usr/local/lib/docker/cli-plugins/docker-buildx"
chmod 0755 "$rootfs/usr/local/lib/docker/cli-plugins/docker-buildx"
cp "$compose_bin" "$rootfs/usr/local/lib/docker/cli-plugins/docker-compose"
chmod 0755 "$rootfs/usr/local/lib/docker/cli-plugins/docker-compose"

collector_path=$(find "$repo" -path '*/tests/vm/bin/smolrunners-collector' -type f -print -quit 2>/dev/null || true)
[ -n "$collector_path" ] || fail 'missing pinned collector binary'
cp "$collector_path" "$rootfs/opt/smolrunners/bin/smolrunners-collector"
chmod 0755 "$rootfs/opt/smolrunners/bin/smolrunners-collector"

cp "$manifest_src" "$rootfs/opt/smolrunners/manifest.json"
cp "$recipe_dir/verify.sh" "$rootfs/opt/smolrunners/verify.sh"
chmod 0755 "$rootfs/opt/smolrunners/verify.sh"

printf 'image build: checking installed versions against the manifest\n'
assert_version docker "$(pin docker)" \
  "$("$rootfs/usr/local/bin/docker" --version | sed -E 's/^Docker version ([0-9.]+).*/\1/')"
assert_version containerd "$(pin containerd)" \
  "$("$rootfs/usr/local/bin/containerd" --version | awk '{print $3}')"
assert_version runc "$(pin runc)" \
  "$("$rootfs/usr/local/bin/runc" --version | head -1 | awk '{print $3}')"
assert_version buildx "$(pin buildx)" \
  "$("$rootfs/usr/local/lib/docker/cli-plugins/docker-buildx" version | awk '{print $2}')"
assert_version compose "$(pin compose)" \
  "$("$rootfs/usr/local/lib/docker/cli-plugins/docker-compose" version --short | head -1)"

printf 'image build: ok\n'
