#!/bin/sh
set -eu

repo=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)
case ${ARCH:-aarch64} in
  aarch64|arm64) arch=aarch64; goarch=arm64 ;;
  x86_64|amd64) arch=x86_64; goarch=amd64 ;;
  *) printf 'build-vm-binaries: unsupported architecture: %s\n' "$ARCH" >&2; exit 2 ;;
esac
target="$arch-unknown-linux-musl"
zig_target="$arch-linux-musl"
if [ "${SMOLRUNNERS_PRINT_ARCH:-}" = 1 ]; then
  printf 'target=%s\nzig_target=%s\ngoarch=%s\n' "$target" "$zig_target" "$goarch"
  exit 0
fi
zig=${ZIG:-$(command -v zig || true)}
[ -n "$zig" ] && command -v "$zig" >/dev/null 2>&1 || { printf 'build-vm-binaries: zig not found\n' >&2; exit 2; }
tmp=${TMPDIR:-$repo/.work/tmp}/cross
wrapper="$tmp/zig-cc-$arch"
mkdir -p "$tmp"
cat > "$wrapper" <<WRAPPER
#!/bin/sh
args=''
for arg in "\$@"; do
  case "\$arg" in
    --target=*|-Wl,--fix-cortex-a53-843419|-B/usr/local/libexec/mold|-nostartfiles) ;;
    *self-contained/crt*|*self-contained/rcrt*) ;;
    *) args="\$args '\$arg'" ;;
  esac
done
eval "exec $zig cc -target $zig_target \$args"
WRAPPER
chmod 0755 "$wrapper"
printf 'build-vm-binaries: cross-building %s from current source\n' "$target"
cd "$repo"
case $arch in
  aarch64) CC_aarch64_unknown_linux_musl="$wrapper" AR_aarch64_unknown_linux_musl="$zig ar" CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER="$wrapper" cargo build --workspace --target "$target" ;;
  x86_64) CC_x86_64_unknown_linux_musl="$wrapper" AR_x86_64_unknown_linux_musl="$zig ar" CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="$wrapper" cargo build --workspace --target "$target" ;;
esac
out="$repo/target/$target/debug"
if [ "$arch" = aarch64 ]; then
  bin="$repo/tests/vm/bin"
  mkdir -p "$bin" "$repo/tests/cli/bin"
else
  bin="$repo/.work/pilot/$arch/bin"
  mkdir -p "$bin"
fi
for name in smolrunners smolrunners-collector smolrunners-launcher smolrunners-cache; do
  [ -x "$out/$name" ] || { printf 'build-vm-binaries: missing %s\n' "$name" >&2; exit 2; }
  cp "$out/$name" "$bin/$name"
done
if [ "$arch" = aarch64 ]; then
  cp "$out/smolrunners" "$bin/smolrunners-doctor"
  cp "$out/smolrunners" "$bin/smolrunners-storage"
  cp "$out/smolrunners-cache" "$repo/crates/smolrunners/src/host/cache/smolrunners-cache"
  cp "$out/smolrunners" "$repo/tests/cli/bin/smolrunners"
  adapter="$repo/tests/cli/bin/smolrunners-adapter"
else
  adapter="$bin/smolrunners-adapter"
fi
printf 'build-vm-binaries: building the static Go adapter\n'
GOOS=linux GOARCH="$goarch" CGO_ENABLED=0 go build -C "$repo/adapter" -o "$adapter" .
printf 'build-vm-binaries: rebuilt guest binaries from current source\n'
