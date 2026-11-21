import argparse
import os
import sys
from pathlib import Path

from buildlib.locks import FetchError, LockError, fetch_input, load_sources, verify_file

REPO = Path(__file__).resolve().parent.parent
DEFAULT_LOCK = REPO / "sources.lock.json"
DEFAULT_CACHE = REPO / ".work" / "cache" / "inputs"
HOST_TARGETS = {
    ("Darwin", "arm64"): "aarch64-apple-darwin",
    ("Linux", "x86_64"): "x86_64-unknown-linux-musl",
    ("Linux", "aarch64"): "aarch64-unknown-linux-musl",
    ("Linux", "arm64"): "aarch64-unknown-linux-musl",
}


def known_targets(lock):
    return {item.target for item in lock.inputs if item.target is not None}


def select(lock, target):
    if target is None:
        return list(lock.inputs)
    if target not in known_targets(lock):
        raise FetchError(f"unknown target {target!r}; expected one of {sorted(known_targets(lock))}")
    return [item for item in lock.inputs if item.target is None or item.target == target]


def destination(cache, item):
    filename = item.url.rsplit("/", 1)[-1].split("?", 1)[0]
    return Path(cache) / item.name / item.version / filename


def fetch_all(lock, cache, target=None, verify_only=False):
    results = []
    for item in select(lock, target):
        path = destination(cache, item)
        path.parent.mkdir(parents=True, exist_ok=True)
        if verify_only:
            verify_file(path, item)
        else:
            fetch_input(item, path)
        results.append((item.name, path))
    return results


def main(argv=None):
    parser = argparse.ArgumentParser(description="Fetch and verify pinned inputs")
    parser.add_argument("--lock", type=Path, default=DEFAULT_LOCK)
    parser.add_argument("--cache", type=Path, default=DEFAULT_CACHE)
    parser.add_argument("--target", default=None)
    parser.add_argument("--name", default=None)
    parser.add_argument("--verify-only", action="store_true")
    args = parser.parse_args(argv)

    if args.target is None:
        import platform

        key = (platform.system(), platform.machine())
        args.target = HOST_TARGETS.get(key)
        if args.target is None:
            print(f"fetch-inputs: unsupported host {key[0]} {key[1]}; use --target", file=sys.stderr)
            return 2

    try:
        lock = load_sources(args.lock)
        if args.name is not None:
            selected = [item for item in lock.inputs if item.name == args.name]
            if not selected:
                raise FetchError(f"unknown input {args.name!r}")
            results = []
            for item in selected:
                path = destination(args.cache, item)
                path.parent.mkdir(parents=True, exist_ok=True)
                if args.verify_only:
                    verify_file(path, item)
                else:
                    fetch_input(item, path)
                results.append((item.name, path))
            for name, path in results:
                print(f"{name}\t{path}")
            return 0
        for name, path in fetch_all(lock, args.cache, args.target, args.verify_only):
            print(f"{name}\t{path}")
    except (FetchError, LockError) as error:
        print(f"fetch-inputs: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
