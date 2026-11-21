import os
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
TEMPLATES = ("storage-template.ext4", "overlay-template.ext4")


def fetch(name):
    result = subprocess.run(
        ["sh", str(REPO / "bootstrap" / "buildlib-run"), "--",
         str(REPO / "scripts" / "fetch-inputs.py"), "--name", name],
        cwd=REPO, capture_output=True, text=True, check=True,
    )
    return Path(result.stdout.strip().split("\t", 1)[1])


def install(name, dest):
    archive = fetch(name)
    dest = Path(dest)
    if dest.is_dir() and all((dest / name).is_file() for name in ("smolvm", *TEMPLATES)):
        enforce_permissions(dest)
        return dest
    with tempfile.TemporaryDirectory(dir=REPO / ".work" / "tmp") as tmp:
        with tarfile.open(archive, "r:gz") as tar:
            tar.extractall(tmp, filter="tar")
        extracted = list(Path(tmp).iterdir())
        if len(extracted) != 1 or not extracted[0].is_dir():
            raise RuntimeError(f"{name}: archive does not contain one root directory")
        dest.parent.mkdir(parents=True, exist_ok=True)
        if dest.exists():
            shutil.rmtree(dest)
        shutil.move(str(extracted[0]), str(dest))
    materialize_templates(dest)
    enforce_permissions(dest)
    return dest


def materialize_templates(root):
    for name in TEMPLATES:
        target = root / name
        if target.is_file():
            continue
        compressed = root / f"{name}.zst"
        if not compressed.is_file():
            raise RuntimeError(f"{name}: no template or compressed template in the bundle")
        subprocess.run(
            ["zstd", "-d", "--sparse", "-f", "-o", str(target), str(compressed)],
            check=True,
        )


def enforce_permissions(root):
    for base, dirs, files in os.walk(root, followlinks=False):
        dirs[:] = [name for name in dirs if not (Path(base) / name).is_symlink()]
        for name in dirs:
            os.chmod(os.path.join(base, name), 0o755)
        for name in files:
            path = Path(base) / name
            if path.is_symlink():
                continue
            mode = path.stat().st_mode & 0o777
            new_mode = mode | stat.S_IRGRP | stat.S_IROTH
            if mode & 0o111:
                new_mode |= stat.S_IXGRP | stat.S_IXOTH
            path.chmod(new_mode)
    for name in TEMPLATES:
        path = root / name
        if not path.is_file():
            raise RuntimeError(f"runtime is missing {name}")
        mode = path.stat().st_mode & 0o777
        if mode & 0o444 != 0o444:
            raise RuntimeError(f"{name} must be readable by all, found {mode:04o}")


def main(argv):
    if len(argv) != 2:
        print("usage: install-runtime <input-name> <dest-dir>", file=sys.stderr)
        return 2
    try:
        path = install(argv[0], argv[1])
    except (RuntimeError, subprocess.CalledProcessError, OSError) as error:
        print(f"install-runtime: {error}", file=sys.stderr)
        return 1
    print(path)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
