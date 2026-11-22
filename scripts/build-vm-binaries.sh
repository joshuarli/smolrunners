#!/bin/sh
set -eu

repo=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)
target=${TARGET:-aarch64-unknown-linux-musl}
zig=${ZIG:-/opt/homebrew/bin/zig}
tmp=${TMPDIR:-$repo/.work/tmp}/cross
wrapper="$tmp/zig-cc"

command -v "$zig" >/dev/null 2>&1 || { printf 'build-vm-binaries: zig not found at %s\n' "$zig" >&2; exit 2; }
mkdir -p "$tmp" "$repo/tests/vm/bin" "$repo/tests/cli/bin"

cat > "$wrapper" <<EOF
#!/bin/sh
args=''
for arg in "\$@"; do
  case "\$arg" in
    --target=*|-Wl,--fix-cortex-a53-843419|-B/usr/local/libexec/mold|-nostartfiles) ;;
    *self-contained/crt*) ;;
    *) args="\$args '\$arg'" ;;
  esac
done
eval "exec $zig cc -target aarch64-linux-musl \$args"
EOF
chmod 0755 "$wrapper"

printf 'build-vm-binaries: cross-building %s from current source\n' "$target"
cd "$repo"
CC_aarch64_unknown_linux_musl="$wrapper" \
AR_aarch64_unknown_linux_musl="$zig ar" \
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER="$wrapper" \
  cargo build --workspace --target "$target"

out="$repo/target/$target/debug"
for name in smolrunners smolrunners-collector smolrunners-launcher smolrunners-cache; do
  [ -x "$out/$name" ] || { printf 'build-vm-binaries: missing %s\n' "$name" >&2; exit 2; }
  cp "$out/$name" "$repo/tests/vm/bin/$name"
done
cp "$out/smolrunners" "$repo/tests/vm/bin/smolrunners-doctor"
cp "$out/smolrunners" "$repo/tests/vm/bin/smolrunners-storage"
cp "$out/smolrunners-cache" "$repo/crates/smolrunners/src/host/cache/smolrunners-cache"
cp "$out/smolrunners" "$repo/tests/cli/bin/smolrunners"

printf 'build-vm-binaries: building the static Go adapter\n'
GOOS=linux GOARCH=arm64 CGO_ENABLED=0 \
  go build -C "$repo/adapter" -o "$repo/tests/cli/bin/smolrunners-adapter" .

printf 'build-vm-binaries: rebuilt guest binaries from current source\n'
