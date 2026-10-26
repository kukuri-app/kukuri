"""Run the pinned native audio contracts without changing Cargo's shared source cache."""

import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import tomllib


def toml_value(value):
    if isinstance(value, dict):
        return "{ " + ", ".join(json.dumps(key) + " = " + toml_value(item) for key, item in value.items()) + " }"
    if isinstance(value, list):
        return "[" + ", ".join(toml_value(item) for item in value) + "]"
    return json.dumps(value)


def isolated_manifest(source, consumer):
    workspace = tomllib.loads((source / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]
    manifest = tomllib.loads((source / "rs/moq-audio/Cargo.toml").read_text(encoding="utf-8"))
    for key, value in list(manifest["package"].items()):
        if isinstance(value, dict) and value.get("workspace"):
            manifest["package"][key] = workspace["package"][key]

    def dependencies(table):
        for name, value in list(table.items()):
            if isinstance(value, dict) and value.get("workspace"):
                base = workspace["dependencies"][name]
                base = {"version": base} if isinstance(base, str) else dict(base)
                features = sorted(set(base.get("features", []) + value.get("features", [])))
                base.update({key: item for key, item in value.items() if key != "workspace"})
                if features:
                    base["features"] = features
                table[name] = base

    for section in ("dependencies", "dev-dependencies", "build-dependencies"):
        dependencies(manifest.get(section, {}))
    for target in manifest.get("target", {}).values():
        for section in ("dependencies", "dev-dependencies", "build-dependencies"):
            dependencies(target.get(section, {}))
    # These are the actual consumer's DSP/device versions, not the latest
    # versions allowed by the upstream workspace's semver ranges.
    locked = tomllib.loads((consumer / "Cargo.lock").read_text(encoding="utf-8"))["package"]
    for name in ("cpal", "sonora", "fixed-resample"):
        versions = {package["version"] for package in locked if package["name"] == name}
        if len(versions) != 1:
            raise ValueError(f"Review the audio contract adapter for {name}: {versions}")
        manifest["dependencies"][name]["version"] = "=" + versions.pop()
    patches = tomllib.loads((consumer / "Cargo.toml").read_text(encoding="utf-8"))["patch"]["crates-io"]
    manifest["patch"] = {"crates-io": {"moq-mux": patches["moq-mux"]}}
    manifest["lib"] = {"path": (source / "rs/moq-audio/src/lib.rs").as_posix(), "name": "moq_audio"}
    manifest["workspace"] = {}
    for field in ("test", "bench", "bin", "example"):
        manifest.pop(field, None)
    for field in ("autotests", "autobenches", "autobins", "autoexamples"):
        manifest["package"][field] = False
    return "\n".join(json.dumps(key) + " = " + toml_value(value) for key, value in manifest.items())


def main():
    root = Path(__file__).resolve().parents[1]
    patch = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["patch"]["crates-io"]["moq-audio"]
    repository = "https://github.com/KingYoSun/moq"
    if patch["git"] != repository or not re.fullmatch(r"[0-9a-f]{40}", patch["rev"]):
        raise ValueError("Expected the pinned moq fork and a full commit SHA")
    parent = (root / "target/upstream-contracts").resolve()
    parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="audio-", dir=parent, ignore_cleanup_errors=True) as temporary:
        work = Path(temporary).resolve()
        work.relative_to(parent)  # Keep recursive cleanup inside this task's build cache.
        source = work / "source"
        source.mkdir()

        def run(*command, cwd=source, env=None):
            subprocess.run(command, cwd=cwd, env=env, check=True)

        run("git", "init", "--quiet")
        run("git", "remote", "add", "origin", repository)
        run("git", "sparse-checkout", "set", "rs/moq-audio")
        run("git", "fetch", "--quiet", "--depth", "1", "origin", patch["rev"])
        run("git", "checkout", "--quiet", "--detach", "FETCH_HEAD")
        aec = (source / "rs/moq-audio/src/aec.rs").read_text(encoding="utf-8")
        if aec.count("fn double_talk_survives_a_changing_device_buffer_delay()") != 1:
            raise ValueError("Pinned source has no reviewed device-delay regression contract")
        contract = work / "contract"
        contract.mkdir()
        (contract / "Cargo.toml").write_text(isolated_manifest(source, root), encoding="utf-8", newline="\n")
        shutil.copyfile(root / "Cargo.lock", contract / "Cargo.lock")
        environment = os.environ.copy()
        environment["CARGO_TARGET_DIR"] = str(parent / "audio-build")
        print(f"Checking native audio {patch['rev']} in an isolated manifest", flush=True)
        run("cargo", "nextest", "run", "--manifest-path", str(contract / "Cargo.toml"), "--release",
            "--no-default-features", "--features", "aec", "--lib", "--no-fail-fast",
            "-E", "test(aec::) or test(capture::) or test(playback::)", "--success-output", "immediate", env=environment)


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as error:
        raise SystemExit(error.returncode) from None
