"""Inspect a Deb without installing/extracting it; emit its actual payload inventory."""
import argparse
import configparser
import email.parser
import hashlib
import io
import json
import pathlib
import re
import subprocess
import tarfile

REQUIRED_DEPENDS = {"libwebkit2gtk-4.1-0", "libgtk-3-0", "libayatana-appindicator3-1", "libssl3", "policykit-1", "libasound2t64"}
EXECUTABLE = "usr/bin/kukuri-desktop-tauri"


def tar_entries(data):
    result = {}
    with tarfile.open(fileobj=io.BytesIO(data)) as tar:
        for entry in tar:
            name = entry.name.removeprefix("./").rstrip("/")
            if not name or name == ".":
                continue
            path = pathlib.PurePosixPath(name)
            if path.is_absolute() or ".." in path.parts or str(path) != name or name in result:
                raise ValueError("Unsafe or duplicate Deb payload path")
            if entry.uid != 0 or entry.gid != 0 or entry.mode & 0o6022:
                raise ValueError("Unexpected Deb ownership or writable/privileged mode")
            if entry.isdir():
                continue
            if not entry.isfile():
                raise ValueError("Indirect or special Deb payload entry")
            result[name] = (tar.extractfile(entry).read(), entry.mode)
    return result


def inspect_payload(metadata, data_tar, control_tar, version):
    if (metadata.get("Package"), metadata.get("Version"), metadata.get("Architecture")) != ("kukuri", version, "amd64"):
        raise ValueError("Deb Package/Version/Architecture mismatch")
    depends = {re.split(r"[ (]", item.strip())[0] for item in metadata.get("Depends", "").split(",")}
    if not REQUIRED_DEPENDS.issubset(depends):
        raise ValueError("Deb system dependencies are incomplete")
    control = tar_entries(control_tar)
    if set(control) - {"control", "md5sums"}:
        # No maintainer scripts are needed: dpkg triggers update desktop/icon caches.
        raise ValueError("Unreviewed Deb control script or extra entry")
    payload = tar_entries(data_tar)
    for name in payload:
        if not (name == EXECUTABLE or name == "usr/share/applications/kukuri.desktop"
                or name.startswith("usr/share/doc/kukuri/")
                or re.fullmatch(r"usr/share/icons/hicolor/[A-Za-z0-9@]+/apps/[A-Za-z0-9_-]+\.png", name)):
            raise ValueError(f"Unreviewed Deb payload path: {name}")
    executable, mode = payload.get(EXECUTABLE, (b"", 0))
    if (len(executable) < 64 or executable[:6] != b"\x7fELF\x02\x01"
            or int.from_bytes(executable[18:20], "little") != 62 or mode & 0o111 != 0o111):
        raise ValueError("Deb executable is not executable x86_64 ELF")
    elfs = sorted(name for name, (data, _) in payload.items() if data.startswith(b"\x7fELF"))
    if elfs != [EXECUTABLE]:
        raise ValueError("Deb includes unclassified native ELF payload")
    for name in ("copyright", "THIRD_PARTY_NOTICES.md"):
        if not payload.get("usr/share/doc/kukuri/" + name, (b"",))[0]:
            raise ValueError("Deb notice is missing")
    if not any(name.startswith("usr/share/doc/kukuri/native-licenses/") for name in payload):
        raise ValueError("Deb native notices are missing")
    desktop = configparser.ConfigParser(interpolation=None)
    desktop.read_string(payload.get("usr/share/applications/kukuri.desktop", (b"",))[0].decode())
    if not desktop.has_section("Desktop Entry"):
        raise ValueError("Deb desktop entry is missing")
    entry = desktop["Desktop Entry"]
    if (entry.get("Type") != "Application" or entry.get("Exec") not in
            {"kukuri-desktop-tauri %U", "kukuri-desktop-tauri %u", "/usr/bin/kukuri-desktop-tauri %U"}
            or "x-scheme-handler/kukuri" not in entry.get("MimeType", "").split(";")):
        raise ValueError("Deb desktop executable/deep-link mismatch")
    if not any(name.endswith("/apps/" + entry.get("Icon", "") + ".png")
               and data.startswith(b"\x89PNG\r\n\x1a\n") for name, (data, _) in payload.items()):
        raise ValueError("Deb desktop icon is missing")
    return {"schema_version": 1, "package": "kukuri", "version": version, "architecture": "amd64",
            "system_dependencies": sorted(depends), "elf_paths": elfs,
            "payload": [{"path": name, "sha256": hashlib.sha256(data).hexdigest(), "mode": mode}
                        for name, (data, mode) in sorted(payload.items())],
            "maintainer_scripts": []}


def inspect(deb, version):
    def command(*args):
        return subprocess.run(["dpkg-deb", *args, str(deb)], check=True, capture_output=True).stdout
    metadata = dict(email.parser.Parser().parsestr(command("--field").decode()))
    report = inspect_payload(metadata, command("--fsys-tarfile"), command("--ctrl-tarfile"), version)
    report["deb_sha256"] = hashlib.sha256(deb.read_bytes()).hexdigest()
    return report


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("deb", type=pathlib.Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--output", type=pathlib.Path)
    parser.add_argument("--source")
    args = parser.parse_args()
    report = inspect(args.deb, args.version)
    if args.source:
        if not re.fullmatch(r"[0-9a-f]{40}", args.source):
            raise ValueError("Deb source must be a full commit")
        report["source_commit"] = args.source
    if args.output:
        with args.output.open("x", encoding="utf-8") as output:
            json.dump(report, output, indent=2)
            output.write("\n")
    print(json.dumps({"deb_sha256": report["deb_sha256"], "verified_payload_files": len(report["payload"])}))
