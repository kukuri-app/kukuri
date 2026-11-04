"""配布候補の生成と照合の試験（#1984 AC-2）。Unity と本物の binary は要らない。"""
import json
import pathlib
import struct
import subprocess
import tarfile
import tempfile
import unittest
import zipfile

import game_sdk_dist as dist

VERSION = "1.2.3"
OS_IMPORTS = ["KERNEL32.dll", "ws2_32.dll"]


def pe(marker, imports):
    """import table だけを持つ最小の 64-bit PE。"""
    header = 0x40
    table = header + 24 + 240
    raw = 0x400
    names = b""
    offsets = []
    for name in imports:
        offsets.append(len(names))
        names += name.encode() + b"\0"
    address = 0x1000
    descriptors = b"".join(
        struct.pack("<IIIII", 0, 0, 0, address + 20 * (len(imports) + 1) + each, 0) for each in offsets
    ) + b"\0" * 20
    body = descriptors + names + marker
    data = bytearray(raw + len(body))
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 0x3C, header)
    data[header : header + 4] = b"PE\0\0"
    struct.pack_into("<H", data, header + 6, 1)
    struct.pack_into("<H", data, header + 20, 240)
    struct.pack_into("<H", data, header + 24, 0x20B)
    struct.pack_into("<I", data, header + 24 + 120, address)
    struct.pack_into("<IIII", data, table + 8, len(body), address, len(body), raw)
    data[raw:] = body
    return bytes(data)


class Fixture:
    def __init__(self, root):
        self.root = root
        self.repository = root / "repository"
        files = {
            "LICENSE": "license",
            "docs/THIRD_PARTY_NOTICES.md": "notices",
            "docs/licenses/libvpx-LICENSE.txt": "libvpx license",
            "docs/licenses/libvpx-PATENTS.txt": "libvpx patents",
            f"{dist.PACKAGE}/package.json": json.dumps({"version": VERSION, "dependencies": {"a": "1"}}),
            f"{dist.PACKAGE}/Runtime/Client.cs": "// client",
            f"{dist.PACKAGE}/Runtime/Plugins/x86_64/{dist.DLL}.meta": "meta",
            f"{dist.SAMPLE}/Assets/Game.cs": "// game",
            f"{dist.SAMPLE}/Packages/manifest.json": json.dumps({"dependencies": {"com.kukuri.gamesdk": "file:../../../sdk", "b": "2"}}),
            f"{dist.SAMPLE}/test.ps1": "# test",
            f"{dist.SAMPLE}/slo.ps1": "# slo",
            f"{dist.SAMPLE}/slo/Probe.cs": "// probe",
        }
        for name, content in files.items():
            path = self.repository / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")
        self.git("init", "-q")
        self.git("add", ".")
        self.git("-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "-q", "-m", "fixture")
        self.source = self.git("rev-parse", "HEAD").strip()
        # 追跡していない file は配布物に入らない。
        (self.repository / dist.PACKAGE / "kukuri.identity-key").write_text("secret", encoding="utf-8")
        self.dll = self.binary("dll", self.marker())
        self.sidecar = self.binary("exe", self.marker())
        self.output = root / "dist"

    def git(self, *args):
        return subprocess.run(["git", *args], cwd=self.repository, check=True, capture_output=True, text=True).stdout

    def marker(self):
        return f"kukuri-game-sdk-build:{VERSION}:{self.source}:end".encode()

    def binary(self, name, marker, imports=OS_IMPORTS):
        path = self.root / name
        path.write_bytes(pe(marker, imports))
        return path

    def package(self, dll=None, sidecar=None):
        dist.package(self.repository, self.source, dll or self.dll, sidecar or self.sidecar, self.output, "rustc 1.98.1")

    def verify(self):
        return dist.verify(self.repository, self.source, self.output)

    def reseal(self):
        """改ざんの後に checksum を合わせ直す（中身の照合だけで検出することを確かめる）。"""
        package_name, sample_name = dist.names(VERSION)
        dist.write_sums(self.output, [dist.DLL, dist.SIDECAR, package_name, sample_name, dist.INFO])


class GameSdkDist(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.fixture = Fixture(pathlib.Path(temporary.name))

    def test_the_candidate_holds_the_commit_the_binaries_and_the_notices(self):
        self.fixture.package()
        self.assertEqual(self.fixture.verify(), VERSION)
        output = self.fixture.output
        package_name, sample_name = dist.names(VERSION)
        package = dist.read_tgz(output / package_name)
        self.assertEqual(package[f"Runtime/Plugins/x86_64/{dist.DLL}"], (output / dist.DLL).read_bytes())
        self.assertEqual(package[f"Editor/Sidecar~/{dist.SIDECAR}"], (output / dist.SIDECAR).read_bytes())
        self.assertEqual(package["Documentation~/libvpx-PATENTS.txt"], b"libvpx patents")
        self.assertNotIn("kukuri.identity-key", package)
        sample = dist.read_zip(output / sample_name)
        self.assertEqual(sorted(sample), ["Assets/Game.cs", "Packages/manifest.json"])
        manifest = json.loads(sample["Packages/manifest.json"])
        self.assertEqual(manifest["dependencies"], {"com.kukuri.gamesdk": f"file:../../{package_name}", "b": "2"})
        info = json.loads((output / dist.INFO).read_text(encoding="utf-8"))
        self.assertEqual((info["source"], info["version"], info["libvpx"]), (self.fixture.source, VERSION, dist.LIBVPX))
        self.assertEqual(info["windows_imports"][dist.DLL], sorted(OS_IMPORTS))

    def test_a_binary_from_another_source_or_with_a_runtime_dependency_is_refused(self):
        other = self.fixture.binary("other", b"kukuri-game-sdk-build:1.2.3:unknown:end")
        with self.assertRaisesRegex(ValueError, "was not built from"):
            self.fixture.package(dll=other)
        crt = self.fixture.binary("crt", self.fixture.marker(), OS_IMPORTS + ["VCRUNTIME140.dll"])
        with self.assertRaisesRegex(ValueError, "not part of Windows"):
            self.fixture.package(sidecar=crt)

    def test_a_missing_or_extra_artifact_fails(self):
        for name in (dist.DLL, dist.SIDECAR, dist.names(VERSION)[0], dist.names(VERSION)[1], dist.INFO, dist.SUMS):
            with self.subTest(name):
                self.fixture.output = self.fixture.root / f"dist-{name}"
                self.fixture.package()
                (self.fixture.output / name).unlink()
                with self.assertRaisesRegex(ValueError, "missing or extra"):
                    self.fixture.verify()
        self.fixture.output = self.fixture.root / "dist-extra"
        self.fixture.package()
        (self.fixture.output / "profile.txt").write_text("x", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "missing or extra"):
            self.fixture.verify()

    def test_a_changed_file_fails_the_checksum(self):
        self.fixture.package()
        (self.fixture.output / dist.INFO).write_text("{}", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, dist.SUMS):
            self.fixture.verify()

    def test_an_added_profile_in_the_package_or_the_sample_fails_even_with_matching_checksums(self):
        self.fixture.package()
        output = self.fixture.output
        package_name, sample_name = dist.names(VERSION)
        original = dist.read_tgz(output / package_name)
        dist.write_tgz(output / package_name, original | {"kukuri.identity-key": b"secret"}, 0)
        self.fixture.reseal()
        with self.assertRaisesRegex(ValueError, "differs from the commit"):
            self.fixture.verify()
        dist.write_tgz(output / package_name, original, 0)
        with zipfile.ZipFile(output / sample_name, "a") as archive:
            archive.writestr(f"{dist.SAMPLE_ROOT}/UserSettings/accounts.json", "{}")
        self.fixture.reseal()
        with self.assertRaisesRegex(ValueError, "differs from the commit"):
            self.fixture.verify()

    def test_a_tampered_binary_inside_the_package_fails(self):
        self.fixture.package()
        package_name = dist.names(VERSION)[0]
        path = self.fixture.output / package_name
        files = dist.read_tgz(path)
        files[f"Runtime/Plugins/x86_64/{dist.DLL}"] += b"tampered"
        dist.write_tgz(path, files, 0)
        self.fixture.reseal()
        with self.assertRaisesRegex(ValueError, "differs from the commit"):
            self.fixture.verify()

    def test_a_committed_secret_or_profile_is_refused(self):
        for name, content, message in (
            ("Runtime/leak.txt", "-----BEGIN PRIVATE KEY-----", "contains a secret"),
            ("Runtime/kukuri.db", "x", "forbidden file"),
            ("Runtime/kukuri.nsec", "x", "forbidden file"),
            ("Runtime/kukuri.iroh-store/blobs", "x", "forbidden file"),
            ("Runtime/endpoint-secret.json", "x", "forbidden file"),
        ):
            with self.subTest(name):
                path = self.fixture.repository / dist.PACKAGE / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content, encoding="utf-8")
                self.fixture.git("add", "-f", ".")
                self.fixture.git("-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "-q", "-m", name)
                self.fixture.source = self.fixture.git("rev-parse", "HEAD").strip()
                self.fixture.dll = self.fixture.binary("dll", self.fixture.marker())
                self.fixture.sidecar = self.fixture.binary("exe", self.fixture.marker())
                self.fixture.output = self.fixture.root / f"dist-{path.name}"
                with self.assertRaisesRegex(ValueError, message):
                    self.fixture.package()
                path.unlink()
                self.fixture.git("add", "-A")
                self.fixture.git("-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "-q", "-m", "remove")

    def test_the_package_is_the_same_bytes_for_the_same_input(self):
        self.fixture.package()
        first = (self.fixture.output / dist.names(VERSION)[0]).read_bytes()
        self.fixture.output = self.fixture.root / "dist-again"
        self.fixture.package()
        self.assertEqual((self.fixture.output / dist.names(VERSION)[0]).read_bytes(), first)
        with tarfile.open(self.fixture.output / dist.names(VERSION)[0], "r:gz") as tar:
            self.assertTrue(all(member.name.startswith("package/") for member in tar.getmembers()))


if __name__ == "__main__":
    unittest.main()
