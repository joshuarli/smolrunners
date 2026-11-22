#!/bin/sh
set -eu

rootfs=${1:?usage: install-runtime.sh ROOTFS CACHE}
cache=${2:?usage: install-runtime.sh ROOTFS CACHE}
recipe_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
repo=$(CDPATH='' cd -- "$recipe_dir/../.." && pwd -P)
tmp="$cache/build-tmp/alpine-runtime"

fail() { printf 'alpine-runtime: %s\n' "$*" >&2; exit 1; }

inputs='alpine-musl-aarch64
alpine-libgcc-aarch64
alpine-libstdcxx-aarch64
alpine-icu-libs-aarch64
alpine-icu-data-en-aarch64
alpine-ca-certificates-aarch64
alpine-brotli-libs-aarch64
alpine-c-ares-aarch64
alpine-libcrypto3-aarch64
alpine-libssl3-aarch64
alpine-nghttp2-libs-aarch64
alpine-simdjson-aarch64
alpine-simdutf-aarch64
alpine-sqlite-libs-aarch64
alpine-zlib-aarch64
alpine-zstd-libs-aarch64
alpine-ada-libs-aarch64
alpine-nodejs-aarch64'

mkdir -p "$tmp" "$rootfs"
for name in $inputs; do
  path=$(sh "$repo/bootstrap/buildlib-run" -- "$repo/scripts/fetch-inputs.py" \
    --name "$name" --cache "$cache/inputs" | tail -1 | cut -f2)
  [ -s "$path" ] || fail "no archive for $name"
  unpack="$tmp/$name"
  rm -rf "$unpack"
  mkdir -p "$unpack"
  tar -xzf "$path" -C "$unpack" || fail "cannot unpack $name"
  ( cd "$unpack" && find . -mindepth 1 -maxdepth 1 \
      \( -name '.SIGN.*' -o -name '.PKGINFO' -o -name '.pre-*' -o -name '.post-*' \) -exec rm -rf {} + )
  cp -a "$unpack/." "$rootfs/" || fail "cannot install $name"
done

[ -x "$rootfs/usr/bin/node" ] || fail 'musl node was not installed'
[ -e "$rootfs/lib/ld-musl-aarch64.so.1" ] || fail 'musl loader was not installed'
[ -e "$rootfs/usr/lib/libstdc++.so.6" ] || fail 'libstdc++ was not installed'
printf 'alpine-runtime: musl Node and C++/ICU runtime installed\n'
