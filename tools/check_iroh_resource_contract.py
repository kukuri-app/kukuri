"""Run a private upstream resource contract without editing Cargo's shared cache."""

import argparse
import os
from pathlib import Path
import re
import subprocess
import tempfile
import tomllib


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--revision", help="Override with a full upstream SHA for before/after checks")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    manifest = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    patch = manifest["patch"]["crates-io"]["iroh"]
    repository = "https://github.com/KingYoSun/iroh"
    if patch["git"] != repository:
        raise ValueError("Unexpected upstream repository")
    revision = args.revision or patch["rev"]
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("A complete upstream commit SHA is required")
    parent = (root / "target" / "upstream-contracts").resolve()
    parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="source-", dir=parent, ignore_cleanup_errors=True) as temporary:
        source = Path(temporary).resolve()
        source.relative_to(parent)  # Verify the recursive cleanup stays inside our own target directory.

        def run(*command, env=None):
            subprocess.run(command, cwd=source, env=env, check=True)

        run("git", "init", "--quiet")
        run("git", "remote", "add", "origin", repository)
        run("git", "fetch", "--quiet", "--depth", "1", "origin", revision)
        run("git", "checkout", "--quiet", "--detach", "FETCH_HEAD")
        file = source / "iroh/src/socket/remote_map.rs"
        content = file.read_text(encoding="utf-8")
        marker = "    fn make_remote_map()"
        if content.count(marker) != 1:
            raise ValueError("Upstream fixture changed; review the test adapter")
        content = content.replace(marker, "    pub(super) fn make_remote_map()", 1)
        content += '\n#[cfg(test)]\nmod kukuri_resource_contract {\n    include!(env!("KUKURI_UPSTREAM_RESOURCE_CONTRACT"));\n}\n'
        file.write_text(content, encoding="utf-8", newline="\n")
        environment = os.environ.copy()
        environment["KUKURI_UPSTREAM_RESOURCE_CONTRACT"] = str(root / "harness/upstream/iroh_mapped_address_contract.rs")
        environment["CARGO_TARGET_DIR"] = str(parent / "build" / revision)
        print(f"Checking upstream iroh {revision}; test-only adapter in {source}", flush=True)
        for contract in ("kukuri_mapped_address_history_stays_bounded", "poll_cleanup_preserves_restarted_sender"):
            run("cargo", "test", "--locked", "--package", "iroh", "--lib", contract, env=environment)


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as error:
        raise SystemExit(error.returncode) from None
