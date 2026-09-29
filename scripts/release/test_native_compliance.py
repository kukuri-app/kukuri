import hashlib
import json
import pathlib
import struct
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).parent))
import native_compliance as native


class NativeTests(unittest.TestCase):
    def test_only_the_exact_md5_section_can_differ(self):
        data = bytearray(256)
        data[:6] = b"\x7fELF\x02\x01"
        struct.pack_into("<Q", data, 40, 128)
        struct.pack_into("<HHH", data, 58, 64, 2, 0)
        names = b"\0.shstrtab\0.digest_md5\0"
        data[64:64 + len(names)] = names
        struct.pack_into("<I", data, 128, 1)
        struct.pack_into("<QQ", data, 152, 64, len(names))
        struct.pack_into("<I", data, 192, 11)
        struct.pack_into("<QQ", data, 216, 96, 16)
        spec = {"runtime_size": 256, "digest_md5_offset": 96, "digest_md5_size": 16,
                "normalized_prefix_sha256": hashlib.sha256(data).hexdigest()}
        data[100] = 12
        native.bind_runtime(data, spec)
        data[113] = 1
        with self.assertRaises(ValueError): native.bind_runtime(data, spec)

    def test_dsc_requires_exact_source_identity_size_and_hash(self):
        with tempfile.TemporaryDirectory() as work:
            root = pathlib.Path(work)
            archive = root / "source_1.0+dfsg~1.tar.xz"
            archive.write_bytes(b"source")
            digest = hashlib.sha256(b"source").hexdigest()
            (root / "source.dsc").write_text(f"Source: example\nVersion: 1.0\nChecksums-Sha256:\n {digest} 6 {archive.name}\n")
            native.verify_dsc(root, "example", "1.0")
            with self.assertRaises(ValueError): native.verify_dsc(root, "wrong", "1.0")
            archive.write_bytes(b"tampered")
            with self.assertRaises(ValueError): native.verify_dsc(root, "example", "1.0")

    def test_fixed_source_inventory_has_unique_names_and_checksums(self):
        spec = json.loads(native.SPEC_FILE.read_text())
        seen = set()
        for row in spec["sources"]:
            identity = (row["package"], row["name"])
            self.assertNotIn(identity, seen)
            seen.add(identity)
            self.assertTrue(row["url"].startswith("https://"))
            if "sha512" in row: self.assertRegex(row["sha512"], r"^[0-9a-f]{128}$")
            else: self.assertRegex(row["sha256"], r"^[0-9a-f]{64}$")
        self.assertEqual(len(seen), 31)

    def test_bundled_runtime_is_the_reviewed_binary(self):
        # #1431: build が渡す runtime は、source を固定した審査済みの版そのもの。
        runtime = native.SPEC_FILE.parents[2] / "packaging/linux/appimage/type2-runtime-x86_64"
        spec = json.loads(native.SPEC_FILE.read_text())["runtime"]
        data = runtime.read_bytes()
        self.assertEqual(len(data), spec["runtime_size"])
        self.assertEqual(hashlib.sha256(data).hexdigest(), spec["normalized_prefix_sha256"])


if __name__ == "__main__":
    unittest.main()
