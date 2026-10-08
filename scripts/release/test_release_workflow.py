"""Check the actual workflow DAG and secret/source boundaries; no GitHub writes."""
import json
import pathlib
import shlex
import subprocess
import unittest

import yaml

ROOT = pathlib.Path(__file__).resolve().parents[2]
# 配布物を build する runner。Linux は配布物の glibc の下限の Ubuntu 22.04（#1180 / #1413）。
LINUX_RELEASE_RUNNER = "ubuntu-22.04"
WINDOWS_RELEASE_RUNNER = "windows-2022"


def workflow(name):
    # GitHub uses YAML 1.2; BaseLoader also preserves the literal `on` key.
    return yaml.load((ROOT / ".github/workflows" / name).read_text(encoding="utf-8"), Loader=yaml.BaseLoader)


class WorkflowTests(unittest.TestCase):
    def test_asset_smoke_reports_success_to_the_ci_pwsh_wrapper(self):
        result = subprocess.run([
            "pwsh", "-NoProfile", "-Command",
            "$ErrorActionPreference = 'Stop'; "
            "& ./scripts/release/test-create-preview-assets.ps1; "
            "& ./scripts/release/test-create-preview-assets-linux.ps1; "
            "if (Test-Path variable:\\LASTEXITCODE) { exit $LASTEXITCODE }",
        ], cwd=ROOT, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=120)
        self.assertIn("Linux + Windows assembly contracts passed", result.stdout)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_linux_refreshes_preinstalled_bundled_libraries_before_bundling(self):
        steps = workflow("kukuri-linux-package.yml")["jobs"]["linux-appimage"]["steps"]
        dependencies = next(step for step in steps if step.get("name") == "Linux build dependencies")
        bundle = next(step for step in steps if step.get("name") == "Build and verify AppImage and Deb")
        self.assertLess(steps.index(dependencies), steps.index(bundle))
        commands = [shlex.split(command) for command in dependencies["run"].replace("\\\n", " ").splitlines()]
        # Runner images keep superseded versions whose exact source leaves the APT index
        # (#907, #1094, #1384), so every preinstalled package is refreshed after the index update.
        update = commands.index(["sudo", "apt-get", "update"])
        upgrade = next(i for i, command in enumerate(commands) if "apt-get" in command and "upgrade" in command)
        self.assertLess(update, upgrade)
        self.assertIn("APT::Get::Always-Include-Phased-Updates=true", commands[upgrade])

    def test_release_verify_installs_every_fast_cn_system_dependency(self):
        # linux-verify reruns the CN tests, including real ffmpeg video extraction (#1060).
        def packages(workflow_name, job_name):
            steps = workflow(workflow_name)["jobs"][job_name]["steps"]
            step = next(step for step in steps if step.get("name") == "Install Linux system dependencies")
            words = shlex.split(step["run"])
            return set(words[words.index("install") + 1:]) - {"-y"}

        missing = packages("kukuri-fast.yml", "linux-cn") - packages("kukuri-release-verify.yml", "linux-verify")
        self.assertEqual(missing, set(), "release linux-verify must install the Fast CN test dependencies")

    def test_publish_requires_every_platform_and_validation(self):
        jobs = workflow("kukuri-release.yml")["jobs"]

        def ancestors(name):
            needs = jobs[name].get("needs", [])
            if isinstance(needs, str): needs = [needs]
            return set(needs).union(*(ancestors(parent) for parent in needs))

        self.assertTrue({"validate-release-inputs", "linux-verify", "windows-package", "linux-package",
                         "cli-package", "release-assets"}.issubset(ancestors("publish-draft")))
        for name in ancestors("publish-draft") | {"publish-draft"}:
            self.assertNotEqual(jobs[name].get("continue-on-error"), "true")
        events = workflow("kukuri-release.yml")["on"]
        self.assertEqual(set(events), {"push", "workflow_dispatch"})

    def test_user_inputs_are_not_interpolated_into_shell(self):
        jobs = workflow("kukuri-release.yml")["jobs"]
        for job in jobs.values():
            for step in job.get("steps", []):
                script = step.get("run", "")
                self.assertNotIn("${{ github.event.inputs", script)
                self.assertNotIn("${{ inputs.", script)
        initial = jobs["validate-release-inputs"]
        self.assertIn("release_source", initial["outputs"])
        for name in ("windows-package", "release-assets"):
            checkout = next(s for s in jobs[name]["steps"] if s.get("uses", "").startswith("actions/checkout"))
            self.assertEqual(checkout["with"]["ref"], "${{ needs.validate-release-inputs.outputs.release_source }}")
        # linux-verify は reusable workflow（#1180）。固定した source と tag を渡し、呼ばれる側で照合する。
        verify = jobs["linux-verify"]
        self.assertEqual(verify["uses"], "./.github/workflows/kukuri-release-verify.yml")
        self.assertEqual(verify["with"]["source_ref"], "${{ needs.validate-release-inputs.outputs.release_source }}")
        self.assertEqual(verify["with"]["release_tag"], "${{ needs.validate-release-inputs.outputs.release_tag }}")
        steps = workflow("kukuri-release-verify.yml")["jobs"]["linux-verify"]["steps"]
        self.assertEqual(steps[0]["with"]["ref"], "${{ inputs.source_ref || github.sha }}")
        self.assertIn("test \"$(git rev-parse HEAD)\" = \"$REQUESTED_SOURCE\"", steps[1]["run"])
        for step in steps:
            self.assertNotIn("${{ inputs.", step.get("run", ""))
        gate = next(step for step in steps if step.get("name") == "Release version gate")
        self.assertEqual(gate["if"], "${{ github.event_name != 'pull_request' }}")
        # PR でも動く file なので、secrets を参照せず、pull_request_target 等の trigger も持たない。
        source = (ROOT / ".github/workflows/kukuri-release-verify.yml").read_text(encoding="utf-8")
        self.assertNotIn("secrets", source)
        self.assertEqual(set(workflow("kukuri-release-verify.yml")["on"]), {"pull_request", "workflow_call", "workflow_dispatch"})

    def test_linux_pr_does_not_receive_distribution_secrets(self):
        steps = workflow("kukuri-linux-package.yml")["jobs"]["linux-appimage"]["steps"]
        signing = next(step for step in steps if step.get("name") == "Build and verify AppImage and Deb")
        for key in ("TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD", "TAURI_UPDATER_PUBLIC_KEY"):
            expression = signing["env"][key]
            self.assertIn("inputs.signing == 'distribution'", expression)
            self.assertIn("github.event_name == 'workflow_dispatch'", expression)
            self.assertIn("startsWith(github.ref, 'refs/tags/v')", expression)
            self.assertNotIn("pull_request", expression)
        cli = workflow("kukuri-cli-package.yml")
        self.assertEqual(cli["jobs"]["cli-package"]["strategy"]["matrix"]["arch"], ["x86_64", "aarch64"])
        self.assertNotIn("secrets", cli["on"].get("workflow_call", {}))

    def test_linux_package_distribution_builds_on_ubuntu_22_04_without_cache(self):
        # 検証の run は Ubuntu 24.04 で cache を使う。配布鍵を渡す run は Ubuntu 22.04 で cache を使わず、
        # PR の run が書いた cache を配布物へ持ち込まない（#1180 / #1413）。
        job = workflow("kukuri-linux-package.yml")["jobs"]["linux-appimage"]
        signing = next(step for step in job["steps"] if step.get("name") == "Build and verify AppImage and Deb")
        distribution = signing["env"]["SIGNING_MODE"].removesuffix(" && 'distribution' || 'test' }}")
        self.assertEqual(job["runs-on"], distribution + " && '" + LINUX_RELEASE_RUNNER + "' || 'ubuntu-24.04' }}")
        not_distribution = "${{ !(" + distribution.removeprefix("${{ ") + ") }}"
        cached = [step for step in job["steps"] if "cache" in step.get("uses", "")]
        self.assertEqual([step["uses"] for step in cached], ["Swatinem/rust-cache@v2"])
        self.assertEqual(cached[0]["if"], not_distribution)
        # `A && '' || B` は空文字が偽のため常に B になる。配布の run で空文字になる形だけを許す。
        node = next(step for step in job["steps"] if step.get("uses", "").startswith("actions/setup-node@"))
        self.assertEqual(node["with"]["cache"], not_distribution.removesuffix(" }}") + " && 'pnpm' || '' }}")

    def test_release_build_jobs_run_on_the_release_runners(self):
        # #1180 / #1413: build / verify と署名する job は、配布物の build 基盤の runner で動く。
        jobs = workflow("kukuri-release.yml")["jobs"]
        self.assertEqual(jobs["validate-release-inputs"]["runs-on"], LINUX_RELEASE_RUNNER)
        verify = workflow("kukuri-release-verify.yml")["jobs"]["linux-verify"]
        self.assertEqual(verify["runs-on"], LINUX_RELEASE_RUNNER)
        self.assertEqual(jobs["windows-package"]["runs-on"], WINDOWS_RELEASE_RUNNER)
        # CLI も配布物なので、Ubuntu 22.04 の glibc で build する（ADR 0049）。
        cli = workflow("kukuri-cli-package.yml")["jobs"]["cli-package"]
        self.assertEqual(cli["runs-on"], LINUX_RELEASE_RUNNER)

    def test_release_path_uses_no_build_cache(self):
        # release は不定期で他 run の cache を読めず、保存と復元の時間だけかかっていた（#1180）。
        release = workflow("kukuri-release.yml")
        self.assertFalse({"RUSTC_WRAPPER", "SCCACHE_GHA_ENABLED", "SCCACHE_GHA_VERSION"} & set(release.get("env", {})))
        jobs = list(release["jobs"].items()) + [
            ("cli-package.yml", workflow("kukuri-cli-package.yml")["jobs"]["cli-package"]),
            ("release-verify.yml", workflow("kukuri-release-verify.yml")["jobs"]["linux-verify"]),
        ]
        for name, job in jobs:
            self.assertFalse({"RUSTC_WRAPPER", "SCCACHE_GHA_ENABLED"} & set(job.get("env", {})), name)
            for step in job.get("steps", []):
                uses = step.get("uses", "")
                for action in ("sccache", "rust-cache", "actions/cache"):
                    self.assertNotIn(action, uses, f"{name}: {uses}")
                if uses.startswith(("actions/setup-node@", "actions/setup-python@")):
                    self.assertNotIn("cache", step.get("with", {}), name)

    def test_changelog_starts_from_the_previous_published_release(self):
        # #1186: Release の無い tag（失敗した release）を起点にしない。git describe に任せず、
        # 公開済み（draft でない）Release から選んだ起点を必ず渡す。
        steps = workflow("kukuri-release.yml")["jobs"]["changelog"]["steps"]
        names = [step.get("name") for step in steps]
        previous = steps[names.index("Resolve previous published release")]
        generate = steps[names.index("Generate changelog section")]
        self.assertLess(names.index("Resolve previous published release"), names.index("Generate changelog section"))
        self.assertEqual(previous["id"], "previous")
        self.assertIn("select(.draft == false)", previous["run"])
        self.assertIn("release_assets.py previous-release", previous["run"])
        self.assertEqual(generate["env"]["PREVIOUS_TAG"], "${{ steps.previous.outputs.previous_tag }}")
        self.assertIn("-PreviousTag $env:PREVIOUS_TAG", generate["run"])

    def test_windows_signing_key_reaches_only_the_package_build_step(self):
        # #1180: 外部 runner では依存 package の install script や setup 系 action から鍵を見せない。
        job = workflow("kukuri-release.yml")["jobs"]["windows-package"]
        self.assertNotIn("env", job)
        # 名前の無い step や `with:` 経由の参照も含め、秘密鍵の secret を参照する step を数える。
        holders = [step.get("name", step.get("uses", "")) for step in job["steps"]
                   if "secrets.TAURI_SIGNING_PRIVATE_KEY" in json.dumps(step)]
        self.assertEqual(holders, ["Build Windows package"])
        build = next(step for step in job["steps"] if step.get("name") == "Build Windows package")
        # 鍵が無いと xtask は updater 成果物なしで成功してしまうため、step 内で空を拒否する。
        self.assertIn("IsNullOrWhiteSpace($env:TAURI_SIGNING_PRIVATE_KEY)", build["run"])

    def test_platform_packages_start_without_waiting_for_linux_verify(self):
        # 公開は publish-draft の祖先（changelog 経由の linux-verify を含む）で担保する。
        jobs = workflow("kukuri-release.yml")["jobs"]
        for name in ("windows-package", "linux-package"):
            needs = jobs[name]["needs"]
            self.assertEqual([needs] if isinstance(needs, str) else needs, ["validate-release-inputs"], name)

    def test_updater_endpoint_is_the_canonical_repository_stable_url(self):
        # 2026-09-16 移管後の正本。旧 owner の URL は GitHub redirect に依存するため設定へ保存しない。
        config = json.loads((ROOT / "apps/desktop/src-tauri/tauri.conf.json").read_text(encoding="utf-8"))
        updater = config["plugins"]["updater"]
        self.assertEqual(updater["endpoints"],
                         ["https://github.com/kukuri-app/kukuri/releases/latest/download/latest-preview.json"])
        self.assertTrue(updater["pubkey"].startswith("dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEI4QzVBN0U3NEIyOEQyM0YK"),
                        "the updater public key must not change with the repository move")


if __name__ == "__main__":
    unittest.main()
