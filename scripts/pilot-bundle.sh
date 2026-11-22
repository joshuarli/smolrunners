#!/bin/sh
set -eu
repo=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)
case ${ARCH:-aarch64} in
  aarch64|arm64) arch=aarch64; runtime=smolvm-linux-arm64; inputs='runner-linux-arm64 docker-static-aarch64 buildx-linux-arm64 compose-linux-arm64 node-linux-arm64' ;;
  x86_64|amd64) arch=x86_64; runtime=smolvm-linux-x86_64; inputs='runner-linux-x64 docker-static-x86_64 buildx-linux-amd64 compose-linux-x64 node-linux-x64' ;;
  *) printf 'pilot-bundle: unsupported architecture: %s\n' "$ARCH" >&2; exit 2 ;;
esac
if [ "${SMOLRUNNERS_PRINT_ARCH:-}" = 1 ]; then
  printf 'arch=%s\nruntime=%s\n' "$arch" "$runtime"
  exit 0
fi
bundle="$repo/.work/pilot/$arch"
mkdir -p "$repo/.work/tmp" "$bundle/bin" "$bundle/scripts" "$bundle/images" "$bundle/.work/cache/inputs"
ARCH="$arch" sh "$repo/scripts/build-vm-binaries.sh"
if [ "$arch" = aarch64 ]; then
  for name in smolrunners smolrunners-launcher smolrunners-collector smolrunners-cache; do cp "$repo/tests/vm/bin/$name" "$bundle/bin/$name"; done
  cp "$repo/tests/cli/bin/smolrunners-adapter" "$bundle/bin/"
fi
cp -R "$repo/images/debian13" "$bundle/images/"
cp "$repo/scripts/fetch-inputs.py" "$repo/scripts/install-runtime" "$repo/scripts/install-runtime.py" "$bundle/scripts/"
cp "$repo/scripts/pilot-host-setup.sh" "$bundle/pilot-host-setup.sh"
cp "$repo/sources.lock.json" "$bundle/"
cp -R "$repo/bootstrap" "$bundle/"
cat > "$bundle/bootstrap/buildlib-run" <<'SHIM'
#!/bin/sh
set -eu
script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
[ "${1:-}" != -- ] || shift
case ${1:-} in
  */scripts/fetch-inputs.py) shift; exec python3 "$script_dir/verify-inputs.py" "$@" ;;
  *) exec python3 "$@" ;;
esac
SHIM
cat > "$bundle/bootstrap/verify-inputs.py" <<'PYTHON'
import argparse
import hashlib
import json
from pathlib import Path

repo = Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser()
parser.add_argument('--name', required=True)
parser.add_argument('--cache', type=Path, default=repo / '.work/cache/inputs')
parser.add_argument('--verify-only', action='store_true')
args = parser.parse_args()
with (repo / 'sources.lock.json').open() as source:
    inputs = json.load(source)['inputs']
items = [item for item in inputs if item['name'] == args.name]
if len(items) != 1:
    raise SystemExit(f'pilot input: unknown or duplicate input {args.name}')
item = items[0]
filename = item['url'].rsplit('/', 1)[-1].split('?', 1)[0]
path = args.cache / item['name'] / item['version'] / filename
try:
    digest = hashlib.sha256()
    size = 0
    with path.open('rb') as archive:
        for block in iter(lambda: archive.read(1024 * 1024), b''):
            size += len(block)
            digest.update(block)
except OSError as error:
    raise SystemExit(f'pilot input: {error}')
if size != item['size'] or digest.hexdigest() != item['sha256']:
    raise SystemExit(f'pilot input: verification failed for {args.name}')
print(f'{args.name}\t{path}')
PYTHON
for input in $runtime $inputs; do
  sh "$repo/bootstrap/buildlib-run" -- "$repo/scripts/fetch-inputs.py" --name "$input" --cache "$bundle/.work/cache/inputs"
done
tar -czf "$repo/.work/pilot/smolrunners-pilot-$arch.tar.gz" -C "$repo/.work/pilot" "$arch"
printf 'pilot-bundle: %s\n' "$repo/.work/pilot/smolrunners-pilot-$arch.tar.gz"
