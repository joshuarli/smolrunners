import os
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent


def mapping(script, arch):
    env = dict(os.environ, ARCH=arch, SMOLRUNNERS_PRINT_ARCH="1")
    result = subprocess.run(["sh", str(REPO / script)], env=env, text=True, capture_output=True, check=True)
    return dict(line.split("=", 1) for line in result.stdout.splitlines())


class PilotSetupTests(unittest.TestCase):
    def test_architecture_mappings(self):
        for arch, platform, runner, docker, buildx, compose, node, triple in (
            ("aarch64", "arm64", "runner-linux-arm64", "docker-static-aarch64", "buildx-linux-arm64", "compose-linux-arm64", "node-linux-arm64", "aarch64-linux-musl"),
            ("x86_64", "amd64", "runner-linux-x64", "docker-static-x86_64", "buildx-linux-amd64", "compose-linux-x64", "node-linux-x64", "x86_64-linux-musl"),
        ):
            with self.subTest(arch=arch):
                image = mapping("images/debian13/build.sh", arch)
                self.assertEqual((image["platform"], image["runner"], image["docker"], image["buildx"], image["compose"], image["node"], image["libdir"]), (platform, runner, docker, buildx, compose, node, f"{arch}-linux-gnu"))
                build = mapping("scripts/build-vm-binaries.sh", arch)
                self.assertEqual((build["target"], build["zig_target"], build["goarch"]), (f"{arch}-unknown-linux-musl", triple, platform))
                bundle = mapping("scripts/pilot-bundle.sh", arch)
                self.assertEqual((bundle["arch"], bundle["runtime"]), (arch, f"smolvm-linux-{'arm64' if arch == 'aarch64' else 'x86_64'}"))
        self.assertEqual(mapping("scripts/build-vm-binaries.sh", "amd64")["target"], "x86_64-unknown-linux-musl")
        self.assertEqual(mapping("images/debian13/build.sh", "arm64")["platform"], "arm64")

    def test_jit_deleted_before_runner_and_exit_propagates(self):
        with tempfile.TemporaryDirectory(dir=REPO / ".work" / "tmp") as directory:
            root = Path(directory)
            runner = root / "runner"
            runner.mkdir()
            jit = root / "jit"
            jit.write_text("secret-jit-value\n")
            (runner / "run.sh").write_text('#!/bin/sh\n[ ! -e "$JIT_PATH" ] || exit 40\n[ "$ACTIONS_RUNNER_INPUT_JITCONFIG" = secret-jit-value ] || exit 41\n[ "$RUNNER_ALLOW_RUNASROOT" = 1 ] || exit 42\nexit 23\n')
            (runner / "run.sh").chmod(0o755)
            script = (REPO / "images/debian13/run-job").read_text().replace("/usr/local/bin/dockerd", str(root / "no-dockerd")).replace("/opt/runner", str(runner))
            executable = root / "run-job"
            executable.write_text(script)
            result = subprocess.run(["sh", str(executable), str(jit)], env=dict(os.environ, JIT_PATH=str(jit)), text=True, capture_output=True)
            self.assertEqual(result.returncode, 23, result.stderr)
            self.assertFalse(jit.exists())
            self.assertNotIn("secret-jit-value", result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
