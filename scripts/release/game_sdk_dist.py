"""Game SDK の配布候補を作り、照合する（ADR 0070、#1984 AC-2）。署名と公開はしない。

配布物の中身は、指定した commit の追跡中の file（`git archive`）と、build した DLL・Sidecar と、notice だけにする。作業 dir の
file は読まないので、手元の profile や未追跡の file は入らない。`verify` は配布物を commit から組み立て直した内容と比べ、1 つでも
違えば失敗する（成果物の欠落を成功として扱わない）。
"""
import argparse
import gzip
import hashlib
import io
import json
import pathlib
import re
import struct
import subprocess
import tarfile
import time
import zipfile

from release_assets import source_version

PACKAGE = "sdk/unity/com.kukuri.gamesdk"
SAMPLE = "examples/cyber-disc-arena-unity"
# sample の開発用の試験（配布しない）。
SAMPLE_EXCLUDED = ("test.ps1", "slo.ps1", "slo/")
SAMPLE_ROOT = "cyber-disc-arena-unity"
DLL = "kukuri_game_sdk.dll"
SIDECAR = "kukuri-sdk-sidecar.exe"
# 配置の path は sdk/unity/build-native.ps1 と同じ。
PACKAGE_BINARIES = {f"Runtime/Plugins/x86_64/{DLL}": DLL, f"Editor/Sidecar~/{SIDECAR}": SIDECAR}
# `~` で終わる dir は Unity が取り込まないので、.meta が要らない。
PACKAGE_NOTICES = {
    "Documentation~/LICENSE": "LICENSE",
    "Documentation~/THIRD_PARTY_NOTICES.md": "docs/THIRD_PARTY_NOTICES.md",
    "Documentation~/libvpx-LICENSE.txt": "docs/licenses/libvpx-LICENSE.txt",
    "Documentation~/libvpx-PATENTS.txt": "docs/licenses/libvpx-PATENTS.txt",
}
LIBVPX = "1.17.0"
UNITY = "6000.3"
# 依存は OS の DLL だけ（ADR 0070 §8.1）。VC++ の再頒布 package の DLL を読み込むと、入っていない PC で動かない。
OS_DLLS = re.compile(
    r"(advapi32|bcrypt|bcryptprimitives|combase|crypt32|dbghelp|iphlpapi|kernel32|mmdevapi|ncrypt|ntdll|ole32|oleaut32|"
    r"psapi|secur32|shell32|user32|userenv|winmm|ws2_32|api-ms-win-core-[a-z0-9-]+)\.dll",
    re.IGNORECASE,
)
SUMS = "SHA256SUMS"
INFO = "build-info.json"
FORBIDDEN_NAME = re.compile(
    r"(^|/)(\.env[^/]*|accounts\.json|endpoint-secret\.json|game-account[^/]*\.json|game-player[^/]*\.json|"
    r"kukuri\.(account|app-consent|community-node|db|game-assets|gossip|identity|iroh|nsec|private-channel|public-channel|remote-blobs|subscriptions)[^/]*(/.*)?|"
    r"\.kukuri-profile[^/]*|[^/]*\.(db|sqlite|pem|key|pfx|p12|ulf|alf)|(Library|Logs|Temp|UserSettings)/.*)$",
    re.IGNORECASE,
)
FORBIDDEN_TEXT = re.compile(rb"-----BEGIN [A-Z ]*PRIVATE KEY-----|ghp_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9_]{22}")


def names(version):
    package = f"com.kukuri.gamesdk-{version}.tgz"
    return package, f"{SAMPLE_ROOT}-{version}.zip"


def digest(data):
    return hashlib.sha256(data).hexdigest()


def git(repository, *args):
    return subprocess.run(
        ["git", "-c", "core.autocrlf=false", "-c", "core.eol=lf", *args], cwd=repository, check=True, capture_output=True
    ).stdout


def tracked(repository, source, prefix):
    """commit の `prefix` の下の file（`prefix` からの相対 path と中身）。"""
    with tarfile.open(fileobj=io.BytesIO(git(repository, "archive", "--format=tar", source, "--", prefix))) as tar:
        return {
            member.name.removeprefix(f"{prefix}/"): tar.extractfile(member).read()
            for member in tar.getmembers()
            if member.isfile()
        }


def package_version(repository, source):
    return json.loads(git(repository, "show", f"{source}:{PACKAGE}/package.json"))["version"]


def pe_imports(data):
    """PE の import table の DLL の名前。"""
    if data[:2] != b"MZ":
        raise ValueError("not a Windows binary")
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe : pe + 4] != b"PE\0\0" or struct.unpack_from("<H", data, pe + 24)[0] != 0x20B:
        raise ValueError("not a 64-bit Windows binary")
    count, optional = struct.unpack_from("<H", data, pe + 6)[0], struct.unpack_from("<H", data, pe + 20)[0]
    table = pe + 24 + optional
    sections = [struct.unpack_from("<IIII", data, table + 40 * index + 8) for index in range(count)]

    def offset(rva):
        for virtual_size, address, raw_size, raw in sections:
            if address <= rva < address + max(virtual_size, raw_size):
                return rva - address + raw
        raise ValueError("the import table is outside the sections")

    imports = set()
    entry = offset(struct.unpack_from("<I", data, pe + 24 + 120)[0])
    while name := struct.unpack_from("<I", data, entry + 12)[0]:
        start = offset(name)
        imports.add(data[start : data.index(b"\0", start)].decode("ascii"))
        entry += 20
    return sorted(imports)


def check_binary(data, label, version, source):
    """版と source の印（`crates/game-sdk/src/lib.rs` の `BUILD_ID`）と、依存が OS の DLL だけであることを確かめる。"""
    if f"kukuri-game-sdk-build:{version}:{source}:end".encode() not in data:
        raise ValueError(f"{label} was not built from {version} at {source}")
    imports = pe_imports(data)
    other = [name for name in imports if not OS_DLLS.fullmatch(name)]
    if other:
        raise ValueError(f"{label} depends on DLLs that are not part of Windows: {other}")
    return imports


def package_files(repository, source, binaries):
    files = dict(tracked(repository, source, PACKAGE))
    for path, name in PACKAGE_BINARIES.items():
        if path in files:
            raise ValueError(f"{path} must not be tracked")
        files[path] = binaries[name]
    for path, notice in PACKAGE_NOTICES.items():
        files[path] = git(repository, "show", f"{source}:{notice}")
    return files


def sample_files(repository, source, version):
    files = {
        path: data
        for path, data in tracked(repository, source, SAMPLE).items()
        if not any(path == excluded or (excluded.endswith("/") and path.startswith(excluded)) for excluded in SAMPLE_EXCLUDED)
    }
    # project を配布物と同じ dir に展開すると、隣の package を読む。
    manifest = json.loads(files["Packages/manifest.json"])
    manifest["dependencies"]["com.kukuri.gamesdk"] = f"file:../../{names(version)[0]}"
    files["Packages/manifest.json"] = (json.dumps(manifest, indent=2) + "\n").encode()
    return files


def write_tgz(path, files, timestamp):
    with path.open("wb") as raw, gzip.GzipFile(filename="", fileobj=raw, mode="wb", mtime=timestamp) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as tar:
            for name in sorted(files):
                info = tarfile.TarInfo(f"package/{name}")
                info.size, info.mode, info.mtime = len(files[name]), 0o644, timestamp
                tar.addfile(info, io.BytesIO(files[name]))


def read_tgz(path):
    with tarfile.open(path, "r:gz") as tar:
        members = tar.getmembers()
        if not all(member.isfile() and member.name.startswith("package/") for member in members):
            raise ValueError("the UPM package has an entry outside package/ or a non-file entry")
        return {member.name.removeprefix("package/"): tar.extractfile(member).read() for member in members}


def write_zip(path, files, timestamp):
    stamp = time.gmtime(timestamp)[:6]
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
        for name in sorted(files):
            archive.writestr(zipfile.ZipInfo(f"{SAMPLE_ROOT}/{name}", stamp), files[name], zipfile.ZIP_DEFLATED)


def read_zip(path):
    with zipfile.ZipFile(path) as archive:
        entries = archive.namelist()
        if not all(name.startswith(f"{SAMPLE_ROOT}/") for name in entries):
            raise ValueError(f"the sample has an entry outside {SAMPLE_ROOT}/")
        return {name.removeprefix(f"{SAMPLE_ROOT}/"): archive.read(name) for name in entries}


def check_clean(label, files):
    """秘密・個人の profile が入っていないこと（binary は本体の中の文字列で誤検出するので、名前だけを見る）。"""
    for name, data in files.items():
        if FORBIDDEN_NAME.search(name):
            raise ValueError(f"{label} contains a forbidden file: {name}")
        if not name.endswith((".dll", ".exe")) and FORBIDDEN_TEXT.search(data):
            raise ValueError(f"{label} contains a secret in {name}")


def package(repository, source, dll, sidecar, output, rust):
    version = package_version(repository, source)
    source_version(version, source)
    if output.exists() and any(output.iterdir()):
        raise ValueError("the output directory must be empty")
    output.mkdir(parents=True, exist_ok=True)
    binaries = {DLL: dll.read_bytes(), SIDECAR: sidecar.read_bytes()}
    imports = {name: check_binary(data, name, version, source) for name, data in binaries.items()}
    for name, data in binaries.items():
        (output / name).write_bytes(data)
    timestamp = int(git(repository, "show", "-s", "--format=%ct", source))
    package_name, sample_name = names(version)
    files = package_files(repository, source, binaries)
    sample = sample_files(repository, source, version)
    check_clean(package_name, files)
    check_clean(sample_name, sample)
    write_tgz(output / package_name, files, timestamp)
    write_zip(output / sample_name, sample, timestamp)
    artifacts = [DLL, SIDECAR, package_name, sample_name]
    info = {
        "schema_version": 1,
        "version": version,
        "source": source,
        "rust": rust,
        "libvpx": LIBVPX,
        "unity": UNITY,
        "package_dependencies": json.loads(files["package.json"])["dependencies"],
        "windows_imports": imports,
    }
    (output / INFO).write_text(json.dumps(info, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    write_sums(output, artifacts + [INFO])


def write_sums(output, files):
    (output / SUMS).write_bytes(
        "".join(f"{digest((output / name).read_bytes())}  {name}\n" for name in sorted(files)).encode()
    )


def verify(repository, source, dist):
    version = package_version(repository, source)
    source_version(version, source)
    package_name, sample_name = names(version)
    artifacts = [DLL, SIDECAR, package_name, sample_name]
    found = sorted(path.name for path in dist.iterdir())
    if found != sorted(artifacts + [INFO, SUMS]):
        raise ValueError(f"the distribution has missing or extra files: {found}")
    expected = "".join(
        f"{digest((dist / name).read_bytes())}  {name}\n" for name in sorted(artifacts + [INFO])
    ).encode()
    if (dist / SUMS).read_bytes() != expected:
        raise ValueError(f"{SUMS} does not match the files")
    info = json.loads((dist / INFO).read_text(encoding="utf-8"))
    if info["source"] != source or info["version"] != version:
        raise ValueError(f"{INFO} is not for {version} at {source}")
    binaries = {name: (dist / name).read_bytes() for name in (DLL, SIDECAR)}
    imports = {name: check_binary(data, name, version, source) for name, data in binaries.items()}
    if info["windows_imports"] != imports:
        raise ValueError(f"{INFO} does not list the DLLs the binaries load")
    for label, actual, wanted in (
        (package_name, read_tgz(dist / package_name), package_files(repository, source, binaries)),
        (sample_name, read_zip(dist / sample_name), sample_files(repository, source, version)),
    ):
        different = sorted(name for name in actual.keys() | wanted.keys() if actual.get(name) != wanted.get(name))
        if different:
            raise ValueError(f"{label} differs from the commit: {different[:10]}")
        check_clean(label, actual)
    return version


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", type=pathlib.Path, default=pathlib.Path(__file__).resolve().parents[2])
    commands = parser.add_subparsers(dest="command", required=True)
    build = commands.add_parser("package")
    build.add_argument("--source", required=True)
    build.add_argument("--dll", type=pathlib.Path, required=True)
    build.add_argument("--sidecar", type=pathlib.Path, required=True)
    build.add_argument("--output", type=pathlib.Path, required=True)
    check = commands.add_parser("verify")
    check.add_argument("--source", required=True)
    check.add_argument("--dist", type=pathlib.Path, required=True)
    args = parser.parse_args()
    if args.command == "package":
        rust = subprocess.run(["rustc", "--version"], check=True, capture_output=True, text=True).stdout.strip()
        package(args.repository, args.source, args.dll, args.sidecar, args.output, rust)
    print(f"Game SDK {verify(args.repository, args.source, args.dist if args.command == 'verify' else args.output)} at {args.source}: verified")


if __name__ == "__main__":
    main()
