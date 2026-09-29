import json
import pathlib
import platform
import subprocess
import tempfile
import unittest
import xml.etree.ElementTree as ET


ROOT = pathlib.Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "apps/desktop/src-tauri/windows/store/Package.appxmanifest"
SCRIPT = ROOT / "scripts/release/build-windows-store-msix.ps1"
WORKFLOW = ROOT / ".github/workflows/kukuri-windows-store-package.yml"
NS = {
    "f": "http://schemas.microsoft.com/appx/manifest/foundation/windows10",
    "uap": "http://schemas.microsoft.com/appx/manifest/uap/windows10",
    "rescap": "http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities",
    "uap10": "http://schemas.microsoft.com/appx/manifest/uap/windows10/10",
}


class WindowsStorePackageContracts(unittest.TestCase):
    def test_windows_checkout_keeps_tauri_manifest_lf(self):
        # Tauri's TOML serializer writes LF; a CRLF checkout can leave Git's
        # status dirty even when diff reports no semantic/content difference.
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run([
                'git', '-c', 'core.autocrlf=true', '-C', str(ROOT),
                'checkout-index', '--prefix=' + pathlib.Path(directory).as_posix() + '/',
                '--', 'apps/desktop/src-tauri/Cargo.toml',
            ], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            content = (pathlib.Path(directory) / 'apps/desktop/src-tauri/Cargo.toml').read_bytes()
            self.assertIn(b'\n', content)
            self.assertNotIn(b'\r\n', content)

    def test_store_version_is_derived_without_a_separate_counter(self):
        with tempfile.TemporaryDirectory() as directory:
            command = r'''
param($repoRoot)
$ErrorActionPreference = 'Stop'
. (Join-Path $repoRoot 'scripts/release/windows-store-version.ps1')
foreach ($case in @(@('0.2.8','1.2.8.0'), @('0.2.9','1.2.9.0'), @('1.0.0','2.0.0.0'), @('65534.65535.65535','65535.65535.65535.0'))) {
    if ((ConvertTo-StoreVersion $case[0]) -ne $case[1]) { throw 'Wrong mapping' }
}
foreach ($invalid in @('65535.0.0','0.65536.0','0.0.65536','01.2.3','0.2.8-preview.1','0.2.8+build.1','0.2','-1.0.0','0.2.8.0','999999999999999999.0.0')) {
    $rejected = $false
    try { ConvertTo-StoreVersion $invalid | Out-Null } catch { $rejected = $true }
    if (-not $rejected) { throw "Accepted unsupported version $invalid" }
}
'''
            path = pathlib.Path(directory) / 'version-test.ps1'
            path.write_text(command, encoding='utf-8')
            result = subprocess.run(['pwsh', '-NoProfile', '-File', str(path), str(ROOT)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    @unittest.skipUnless(platform.system() == "Windows", "System.Drawing shell assets require Windows; covered by Store package CI")
    def test_shell_icons_have_transparent_targetsize_variants(self):
        with tempfile.TemporaryDirectory() as directory:
            command = r'''
param($repoRoot, $output)
$ErrorActionPreference = 'Stop'
. (Join-Path $repoRoot 'scripts/release/windows-store-assets.ps1')
$names = @(New-StoreShellIcons (Join-Path $repoRoot 'apps/desktop/src-tauri/icons/icon.png') $output)
if ($names.Count -ne 42) { throw 'Expected 14 sizes and 3 theme variants' }
foreach ($size in @(16,20,24,30,32,36,40,48,60,64,72,80,96,256)) {
    foreach ($suffix in @('', '_altform-unplated', '_altform-lightunplated')) {
        $name = "Square44x44Logo.targetsize-${size}${suffix}.png"
        if ($name -notin $names) { throw "Missing $name" }
        $bitmap = [Drawing.Bitmap]::new((Join-Path $output $name))
        try {
            if ($bitmap.Width -ne $size -or $bitmap.Height -ne $size) { throw 'Wrong size' }
            if ($bitmap.GetPixel(0,0).A -ne 0) { throw 'Opaque background' }
            $visible = $false
            for ($y=0; $y -lt $size; $y++) {
                for ($x=0; $x -lt $size; $x++) {
                    if ($bitmap.GetPixel($x,$y).A -gt 0) { $visible = $true }
                }
            }
            if (-not $visible) { throw 'Empty icon' }
        } finally { $bitmap.Dispose() }
    }
}
'''
            test_script = pathlib.Path(directory) / 'icons-test.ps1'
            test_script.write_text(command, encoding='utf-8')
            result = subprocess.run(['pwsh', '-NoProfile', '-File', str(test_script), str(ROOT), directory], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_output_guard_rejects_existing_directories_without_deleting_content(self):
        # Execute only the real guard, never the packaging/deletion entrypoint.
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            existing = root / "dist" / "existing"
            existing.mkdir(parents=True)
            marker = existing / "keep.txt"
            marker.write_text("keep", encoding="utf-8")
            command = r'''
param($scriptPath, $repoRoot, $existing)
$ast = [Management.Automation.Language.Parser]::ParseFile($scriptPath, [ref]$null, [ref]$null)
$function = $ast.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'Assert-WorkspaceChild'}, $true)
. ([scriptblock]::Create($function.Extent.Text))
Assert-WorkspaceChild (Join-Path $repoRoot 'dist/new') 'OutputDirectory'
try { Assert-WorkspaceChild (Join-Path $repoRoot '.git/new') 'OutputDirectory'; exit 2 } catch {}
try { Assert-WorkspaceChild $existing 'OutputDirectory' } catch { exit 0 }
exit 1
'''
            test_script = root / "guard-test.ps1"
            test_script.write_text(command, encoding="utf-8")
            result = subprocess.run(["pwsh", "-NoProfile", "-File", str(test_script), str(SCRIPT), str(root), str(existing)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(marker.read_text(encoding="utf-8"), "keep")

    def test_manifest_matches_the_registered_partner_center_identity(self):
        root = ET.parse(MANIFEST).getroot()
        identity = root.find("f:Identity", NS)
        self.assertIsNotNone(identity)
        self.assertEqual(identity.attrib, {
            "Name": "KingYoSun.kukuri",
            "Publisher": "CN=33EB763C-4859-4E44-886F-1784E16DD6D5",
            "Version": "0.0.0.0",  # Template sentinel; build derives the version from package.json.
            "ProcessorArchitecture": "x64",
        })
        properties = root.find("f:Properties", NS)
        self.assertEqual(properties.findtext("f:PublisherDisplayName", namespaces=NS), "KingYoSun")
        integrity = properties.find("uap10:PackageIntegrity/uap10:Content", NS)
        self.assertEqual(integrity.attrib["Enforcement"], "on")

    def test_build_bypass_is_rejected_before_any_tool_or_output(self):
        result = subprocess.run(["pwsh", "-NoProfile", "-File", str(SCRIPT), "-SkipBuild"], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("SkipBuild", result.stderr)
        source = SCRIPT.read_text(encoding="utf-8")
        self.assertIn('$env:CARGO_TARGET_DIR = $storeTargetDir', source)
        self.assertNotIn('Remove-Item', source)

    def test_manifest_exposes_only_the_required_full_trust_surface(self):
        root = ET.parse(MANIFEST).getroot()
        application = root.find("f:Applications/f:Application", NS)
        self.assertEqual(application.attrib["Executable"], "kukuri.exe")
        self.assertEqual(application.attrib["EntryPoint"], "Windows.FullTrustApplication")
        self.assertEqual(application.attrib[f"{{{NS['uap10']}}}TrustLevel"], "mediumIL")
        self.assertEqual(application.attrib[f"{{{NS['uap10']}}}RuntimeBehavior"], "packagedClassicApp")
        protocol = application.find("f:Extensions/uap:Extension/uap:Protocol", NS)
        self.assertEqual(protocol.attrib["Name"], "kukuri")
        capabilities = root.findall("f:Capabilities/*", NS)
        self.assertEqual(len(capabilities), 1)
        self.assertEqual(capabilities[0].tag, f"{{{NS['rescap']}}}Capability")
        self.assertEqual(capabilities[0].attrib["Name"], "runFullTrust")
        self.assertNotRegex(MANIFEST.read_text(encoding="utf-8"), r"\$[A-Za-z].*?\$")

    def test_packaging_produces_only_the_unsigned_store_candidate(self):
        source = SCRIPT.read_text(encoding="utf-8")
        self.assertIn('$requiredWinAppVersion = "0.6.1"', source)
        self.assertRegex(source, r'"pack", \$stagingDir,\s*"--manifest"')
        self.assertNotIn('"--cert"', source)
        self.assertNotIn('"--cert-password"', source)
        self.assertNotIn("code_sign_certificate", source)
        self.assertNotIn("KUKURI_MSIX_CERT_PASSWORD", source)
        self.assertNotIn("SignTool", source)
        self.assertNotIn("Import-PfxCertificate", source)
        self.assertNotIn("Import-Certificate", source)
        self.assertIn('"--features", "microsoft-store"', source)
        self.assertIn('$env:VITE_KUKURI_DISTRIBUTION = "microsoft-store"', source)
        self.assertIn("MSIX payload does not match the fixed allowlist", source)
        self.assertIn("MSIX block map must use SHA-256", source)

    def test_ci_builds_an_unsigned_package_without_distribution_secrets(self):
        source = WORKFLOW.read_text(encoding="utf-8")
        self.assertNotIn("secrets.", source)
        self.assertIn(
            "uses: microsoft/setup-WinAppCli@cc8ea9a08b3ee3db43d5aa6bddda4a0e87d800f7",
            source,
        )
        self.assertIn("version: v0.6.1", source)
        self.assertIn("Expected WinApp CLI 0.6.1", source)
        self.assertIn("run: cargo xtask-lite windows-store-package", source)
        self.assertNotIn("--sign-local", source)


if __name__ == "__main__":
    unittest.main()
