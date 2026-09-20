#!/usr/bin/env python3
"""Check/load the four production OCI images, then smoke their entrypoints.

Usage: python scripts/ci/cn_image_check.py <bake OUT_DIR>
Uses only the Python standard library and Docker. No registry writes.
"""

import gzip
import hashlib
import io
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import uuid


IMAGES = ("user-api", "iroh-relay", "cli", "indexer")
# ac9f93ceef95 linux/amd64, sum of gzip layer descriptor sizes (not docker SIZE).
BASELINES = dict(zip(IMAGES, (648220591, 239963153, 733553685, 748691783)))


def run(*args):
    result = subprocess.run(args, text=True, encoding="utf-8", stdout=subprocess.PIPE)
    if result.returncode:
        print(result.stdout, file=sys.stderr)
        result.check_returncode()
    return result.stdout.strip()


def blob(root, descriptor):
    algorithm, digest = descriptor["digest"].split(":")
    if algorithm != "sha256":
        raise ValueError(f"Unsupported digest: {algorithm}")
    path = root / "blobs" / algorithm / digest
    with path.open("rb") as stream:
        actual = hashlib.file_digest(stream, algorithm).hexdigest()
    if actual != digest or path.stat().st_size != descriptor["size"]:
        raise ValueError(f"Invalid OCI blob: {path}")
    return path


def add_bytes(archive, name, data):
    info = tarfile.TarInfo(name)
    info.size = len(data)
    archive.addfile(info, io.BytesIO(data))


def check_image(root, name, tag):
    index = json.loads((root / "index.json").read_text())
    # Buildx's index may wrap the platform index in one descriptor.
    while "manifests" in index:
        candidates = [d for d in index["manifests"] if d.get("platform", {}).get(
            "architecture") == "amd64" and d.get("platform", {}).get("os") == "linux"]
        if len(candidates) == 1:
            descriptor = candidates[0]
        elif len(index["manifests"]) == 1:
            descriptor = index["manifests"][0]
        else:
            raise ValueError(f"{name}: expected one linux/amd64 manifest")
        index = json.loads(blob(root, descriptor).read_text())
    manifest = index
    config_path = blob(root, manifest["config"])
    config = json.loads(config_path.read_text())
    assert (config["os"], config["architecture"]) == ("linux", "amd64")
    assert config["config"]["Entrypoint"] == ["/usr/local/bin/app"]
    size = sum(layer["size"] for layer in manifest["layers"])
    assert size <= BASELINES[name], f"{name}: {size} > baseline {BASELINES[name]}"
    app_layers = []
    app_hash = None
    with tempfile.TemporaryDirectory(prefix="cn-image-check-") as directory:
        archive_path = Path(directory) / "image.tar"
        layers = []
        with tarfile.open(archive_path, "w") as archive:
            add_bytes(archive, "config.json", config_path.read_bytes())
            for i, layer in enumerate(manifest["layers"]):
                assert layer["mediaType"].endswith("tar+gzip"), layer["mediaType"]
                compressed = blob(root, layer)
                with tarfile.open(compressed, "r:gz") as contents:
                    for member in contents:
                        path = member.name.removeprefix("./").rstrip("/")
                        assert not (path == "tmp/release" or path.startswith("tmp/release/")), path
                        assert not path.startswith("workspace/target/"), path
                        assert not path.endswith((".rlib", ".rmeta")), path
                        assert not re.search(r"(^|/)cn-(user-api|iroh-relay|cli|indexer)(\.d)?$", path), path
                        if path == "usr/local/bin/app":
                            assert member.isfile() and member.mode & 0o111, "app must be executable"
                            app_hash = hashlib.file_digest(contents.extractfile(member), "sha256").hexdigest()
                            app_layers.append(layer["size"])
                # Import the exact OCI layer bytes as a Docker archive, preserving diff_ids.
                raw_path = Path(directory) / "layer.tar"
                with gzip.open(compressed, "rb") as source, raw_path.open("wb") as destination:
                    shutil.copyfileobj(source, destination)
                with raw_path.open("rb") as stream:
                    diff_id = "sha256:" + hashlib.file_digest(stream, "sha256").hexdigest()
                assert diff_id == config["rootfs"]["diff_ids"][i]
                layer_name = f"layer-{i}.tar"
                archive.add(raw_path, arcname=layer_name)
                layers.append(layer_name)
            assert len(app_layers) == 1, f"{name}: expected one app layer"
            add_bytes(archive, "manifest.json", json.dumps([
                {"Config": "config.json", "RepoTags": [tag], "Layers": layers}
            ]).encode())
        run("docker", "load", "--input", str(archive_path))
    loaded = json.loads(run("docker", "image", "inspect", tag))[0]
    # containerd image stores may report the manifest digest as Id. Compare the
    # actual runtime configuration and uncompressed layer identities instead.
    assert loaded["RootFS"]["Layers"] == config["rootfs"]["diff_ids"]
    for key in ("Entrypoint", "Cmd", "Env", "User", "WorkingDir", "Labels"):
        assert loaded["Config"].get(key) == config["config"].get(key), key
    actual_app_hash = run("docker", "run", "--rm", "--entrypoint", "sha256sum", tag,
                          "/usr/local/bin/app").split()[0]
    assert actual_app_hash == app_hash
    run("docker", "run", "--rm", "--entrypoint", "sh", tag, "-ec",
        "test ! -e /tmp/release; test -x /usr/local/bin/app")
    if name in ("cli", "indexer"):
        run("docker", "run", "--rm", "--entrypoint", "sh", tag, "-ec",
            "ffmpeg -version >/dev/null; ffprobe -version >/dev/null; test -s /usr/local/share/kukuri/decoder-build-id")
    result = {"image": name, "digest": descriptor["digest"], "bytes": size,
              "baseline_bytes": BASELINES[name], "binary_layer_bytes": app_layers[0],
              "app_sha256": app_hash, "tag": tag}
    print(json.dumps(result), flush=True)
    return result


def wait_ready(container, *command):
    for _ in range(60):
        if subprocess.run(["docker", "exec", container, *command],
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
            return
        if run("docker", "inspect", container, "--format", "{{.State.Running}}") != "true":
            break
        time.sleep(1)
    raise RuntimeError(f"{container}: startup failed\n{run('docker', 'logs', container)}")


def smoke(tags):
    prefix = "cn-image-smoke-" + uuid.uuid4().hex[:10]
    containers = []
    network = run("docker", "network", "create", "--internal", prefix)
    try:
        def start(suffix, *args):
            name = prefix + "-" + suffix
            # Record before run so a failed create/start is also cleaned up.
            containers.append(name)
            run("docker", "run", "-d", "--name", name, "--network", network, *args)
            return name

        postgres = start("db", "--network-alias", "db", "-e", "POSTGRES_PASSWORD=smoke",
                         "-e", "POSTGRES_DB=cn", "postgres:17-bookworm")
        wait_ready(postgres, "pg_isready", "-U", "postgres", "-d", "cn")
        redis = start("redis", "--network-alias", "redis", "valkey/valkey:8-alpine")
        wait_ready(redis, "valkey-cli", "ping")
        assert "Usage:" in run("docker", "run", "--rm", "--network", "none", tags["cli"], "--help")
        run("docker", "run", "--rm", "--network", network, tags["cli"],
            "--database-url", "postgres://postgres:smoke@db/cn", "prepare")
        # Use the canonical sample rather than maintaining a second operator schema.
        source = (Path(__file__).resolve().parents[2] / "crates/cn-operator/src/lib.rs").read_text(encoding="utf-8")
        sample = source.split('pub const SAMPLE_CONFIG: &str = r#"', 1)[1].split('"#;', 1)[0]
        with tempfile.TemporaryDirectory(prefix="cn-smoke-config-") as directory:
            config = Path(directory) / "operator.yaml"
            config.write_text(sample, encoding="utf-8")
            api = start("api", "-v", f"{config.resolve()}:/operator.yaml:ro",
                        "-e", "COMMUNITY_NODE_OPERATOR_CONFIG=/operator.yaml",
                        "-e", "COMMUNITY_NODE_DATABASE_URL=postgres://postgres:smoke@db/cn",
                        "-e", "COMMUNITY_NODE_RENDEZVOUS_REDIS_URL=redis://redis:6379/",
                        "-e", "COMMUNITY_NODE_BASE_URL=http://127.0.0.1:8080",
                        "-e", "COMMUNITY_NODE_JWT_ISSUER=smoke",
                        "-e", "COMMUNITY_NODE_JWT_SECRET=image-smoke-jwt-secret-not-production",
                        "-e", "COMMUNITY_NODE_LEGAL_DATA_KEY=image-smoke-legal-key-not-production",
                        tags["user-api"])
            wait_ready(api, "curl", "--fail", "--silent", "http://127.0.0.1:8080/healthz")
        relay = start("relay", tags["iroh-relay"])
        wait_ready(relay, "curl", "--fail", "--silent", "http://127.0.0.1:3340/generate_204")
        # The complete positive/negative indexer contract is run by cn_indexer_smoke.sh.
        print("cli --help/prepare, user-api /healthz, relay /generate_204: PASS", flush=True)
    finally:
        for container in reversed(containers):
            subprocess.run(["docker", "rm", "-fv", container], check=False, stdout=subprocess.DEVNULL)
        subprocess.run(["docker", "network", "rm", network], check=False, stdout=subprocess.DEVNULL)


def main():
    output = Path(sys.argv[1]).resolve()
    prefix = "cn-checked-" + uuid.uuid4().hex[:10]
    results = []
    tags = {name: f"{prefix}-{name}:local" for name in IMAGES}
    try:
        for name in IMAGES:
            results.append(check_image(output / f"kukuri-cn-{name}", name, tags[name]))
        smoke(tags)
        # Pass the exact checked production indexer to the existing contract script.
        # Resolve PATH explicitly: Windows otherwise finds System32/WSL bash
        # before Git Bash and can silently switch Docker daemons.
        subprocess.run([shutil.which("bash") or "bash", "scripts/ci/cn_indexer_smoke.sh",
                        tags["indexer"]], check=True)
        (output / "image-check-results.json").write_text(json.dumps(results, indent=2) + "\n")
    finally:
        for tag in tags.values():
            subprocess.run(["docker", "image", "rm", tag], check=False, stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL)


if __name__ == "__main__":
    main()
