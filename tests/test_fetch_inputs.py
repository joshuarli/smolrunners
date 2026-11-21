import hashlib
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

from buildlib.locks import FetchError, Input

REPO = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location("fetch_inputs", REPO / "scripts" / "fetch-inputs.py")
fetch_inputs = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fetch_inputs)


class FetchInputsBoundary(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=REPO / ".work" / "tmp")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.cache = self.root / "cache"
        data = b"pinned bytes for boundary tests"
        self.digest = hashlib.sha256(data).hexdigest()
        self.size = len(data)
        self.item = Input(
            name="fixture",
            version="1",
            sha256=self.digest,
            urls=("https://example.invalid/fixture.bin",),
            size=self.size,
            target="aarch64-unknown-linux-musl",
        )
        self.lock_doc = {
            "schema_version": 1,
            "inputs": [
                {
                    "name": "fixture",
                    "version": "1",
                    "url": "https://example.invalid/fixture.bin",
                    "sha256": self.digest,
                    "size": self.size,
                    "target": "aarch64-unknown-linux-musl",
                }
            ],
        }
        self.path = fetch_inputs.destination(self.cache, self.item)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.path.write_bytes(data)

    def write_lock(self, doc):
        path = self.root / "lock.json"
        path.write_text(json.dumps(doc))
        return path

    def test_wrong_digest(self):
        doc = json.loads(json.dumps(self.lock_doc))
        doc["inputs"][0]["sha256"] = "0" * 64
        lock = fetch_inputs.load_sources(self.write_lock(doc))
        with self.assertRaises(FetchError):
            fetch_inputs.fetch_all(lock, self.cache, verify_only=True)

    def test_wrong_size(self):
        doc = json.loads(json.dumps(self.lock_doc))
        doc["inputs"][0]["size"] = self.size + 1
        lock = fetch_inputs.load_sources(self.write_lock(doc))
        with self.assertRaises(FetchError):
            fetch_inputs.fetch_all(lock, self.cache, verify_only=True)

    def test_wrong_target(self):
        lock = fetch_inputs.load_sources(self.write_lock(self.lock_doc))
        with self.assertRaises(FetchError):
            fetch_inputs.select(lock, "x86_64-pc-windows-msvc")

    def test_truncated_download(self):
        self.path.write_bytes(b"pinned bytes")
        lock = fetch_inputs.load_sources(self.write_lock(self.lock_doc))
        with self.assertRaises(FetchError):
            fetch_inputs.fetch_all(lock, self.cache, verify_only=True)

    def test_cache_reuse(self):
        before = self.path.stat().st_mtime_ns
        lock = fetch_inputs.load_sources(self.write_lock(self.lock_doc))
        result = fetch_inputs.fetch_all(lock, self.cache)
        self.assertEqual(result[0][1], self.path)
        self.assertEqual(self.path.stat().st_mtime_ns, before)


if __name__ == "__main__":
    unittest.main()
