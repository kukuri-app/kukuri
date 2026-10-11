"""Signing and credential boundaries with real JDK certificates and signed JARs."""
import contextlib
import hashlib
import io
import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest
import zipfile
from unittest import mock

import yaml

import android_package as package


class SourceAndWorkflowTests(unittest.TestCase):
    def test_upload_signing_requires_the_same_tag_and_workflow_source(self):
        source = 'a' * 40
        tag = 'v0.4.3-preview.5'
        valid = {'GITHUB_EVENT_NAME': 'workflow_dispatch', 'GITHUB_REF': 'refs/tags/' + tag,
                 'GITHUB_WORKFLOW_SHA': source}
        for changed in ({}, {'GITHUB_EVENT_NAME': 'pull_request'}, {'GITHUB_REF': 'refs/heads/main'},
                        {'GITHUB_WORKFLOW_SHA': 'b' * 40}):
            with self.subTest(changed=changed), mock.patch.dict(os.environ, {**valid, **changed}, clear=True):
                if changed:
                    with self.assertRaises(ValueError):
                        package.production_guard(source, tag)
                else:
                    package.production_guard(source, tag)

    def test_invalid_fingerprint_is_rejected_without_accepting_a_different_identity(self):
        self.assertEqual(package.fingerprint(':'.join(['AB'] * 32)), 'ab' * 32)
        for invalid in ('', 'a' * 63, 'z' * 64):
            with self.assertRaises(ValueError):
                package.fingerprint(invalid)

    def test_signing_runner_is_separate_and_receives_secrets_only_at_sign(self):
        path = package.ROOT / '.github/workflows/kukuri-android-package.yml'
        workflow = yaml.load(path.read_text(encoding='utf-8'), Loader=yaml.BaseLoader)
        self.assertNotIn('pull_request_target', workflow['on'])
        self.assertEqual(workflow['jobs']['sign']['needs'], 'unsigned')
        for job in workflow['jobs'].values():
            self.assertNotIn('cache', str(job).lower())
        unsigned = workflow['jobs']['unsigned']['steps']
        for step in unsigned:
            for key, value in step.get('env', {}).items():
                if 'secrets.' in value:
                    self.assertEqual(key, 'UPLOAD_KEY_AVAILABLE')
                    self.assertIn("!= ''", value)
        sign = workflow['jobs']['sign']['steps']
        credential_steps = [step for step in sign if 'ANDROID_UPLOAD_KEYSTORE_BASE64' in step.get('env', {})]
        self.assertEqual([step['name'] for step in credential_steps], ['Sign and verify the candidate'])
        for name in ('ANDROID_UPLOAD_KEYSTORE_BASE64', 'ANDROID_UPLOAD_KEYSTORE_PASSWORD'):
            expression = credential_steps[0]['env'][name]
            for gate in ("github.event_name != 'pull_request'", "startsWith(github.ref, 'refs/tags/v')", "inputs.signing == 'upload'"):
                self.assertIn(gate, expression)
        self.assertFalse(any('tauri' in step.get('run', '') or 'pnpm' in step.get('run', '') for step in sign))


@unittest.skipUnless(os.environ.get('JAVA_HOME'), 'JAVA_HOME is required for actual signing tools')
class SigningTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='kukuri-android-sign-contract-')
        self.root = pathlib.Path(self.directory.name)
        self.password = mock.patch.dict(os.environ, {'ANDROID_UPLOAD_KEYSTORE_PASSWORD': 'test-only-password'})
        self.password.start()

    def tearDown(self):
        self.password.stop()
        self.directory.cleanup()

    def key(self, name, subject):
        path = self.root / (name + '.p12')
        package.run(package.java_tool('keytool'), '-genkeypair', '-keystore', path, '-storetype', 'PKCS12',
                    '-alias', 'upload', '-keyalg', 'RSA', '-keysize', '2048', '-validity', '10000',
                    '-dname', subject, '-storepass:env', 'ANDROID_UPLOAD_KEYSTORE_PASSWORD')
        return path

    def test_real_certificates_distinguish_ci_debug_and_upload_identity(self):
        with contextlib.redirect_stdout(io.StringIO()) as output:
            ci = self.key('ci', 'CN=kukuri CI test')
            debug = self.key('debug', 'CN=Android Debug,O=Android,C=US')
            # An ephemeral contract fixture, not a registered production key.
            upload = self.key('upload-fixture', 'CN=kukuri upload')
            ci_fp, ci_debug = package.certificate(ci, 'upload')
            debug_fp, is_debug = package.certificate(debug, 'upload')
            upload_fp, upload_debug = package.certificate(upload, 'upload')
        self.assertEqual(output.getvalue(), '')
        self.assertTrue(ci_debug)
        self.assertTrue(is_debug)
        self.assertFalse(upload_debug)
        self.assertEqual(len({ci_fp, debug_fp, upload_fp}), 3)

    def test_real_signed_jar_rejects_a_modified_payload(self):
        keystore = self.key('jar', 'CN=kukuri CI test')
        jar = self.root / 'candidate.aab'
        with zipfile.ZipFile(jar, 'w') as archive:
            archive.writestr('base/payload.txt', 'original')
        package.run(package.java_tool('jarsigner'), '-keystore', keystore, '-storepass:env',
                    'ANDROID_UPLOAD_KEYSTORE_PASSWORD', jar, 'upload')
        verify = (package.java_tool('jarsigner'), '-verify', '-strict', '-keystore', keystore,
                  '-storepass:env', 'ANDROID_UPLOAD_KEYSTORE_PASSWORD', jar)
        package.run(*verify)
        with zipfile.ZipFile(jar, 'a') as archive:
            archive.writestr('unsigned-added.txt', 'tampered')
        with self.assertRaises(ValueError):
            package.run(*verify)


@unittest.skipUnless(os.name == 'nt' and os.environ.get('JAVA_HOME'), 'Windows and JDK are required')
class UploadOperatorTests(unittest.TestCase):
    def test_create_and_register_existing_in_windows_powershell_and_pwsh(self):
        script = package.ROOT / 'scripts/release/create-android-upload-key.ps1'
        password = 'fixture_日本語 password'
        with tempfile.TemporaryDirectory(prefix='kukuri-upload-contract-') as directory:
            root = pathlib.Path(directory)
            # This executable accepts only a synthetic repository and logs hashes, not inputs.
            source = root / 'main.go'
            source.write_text('''package main
import("os";"io";"crypto/sha256";"encoding/hex";"encoding/json")
func main(){a:=os.Args[1:];ok:=false;for _,v:=range a{if v=="fixture/unused"{ok=true}};if !ok{os.Exit(2)}
b,_:=io.ReadAll(os.Stdin);s:=sha256.Sum256(b);f,e:=os.OpenFile(os.Getenv("KUKURI_FAKE_GH_LOG"),os.O_CREATE|os.O_APPEND|os.O_WRONLY,0600);if e!=nil{os.Exit(3)};defer f.Close()
json.NewEncoder(f).Encode(map[string]any{"args":a,"length":len(b),"sha256":hex.EncodeToString(s[:])})}
''', encoding='utf-8')
            subprocess.run(['go', 'build', '-o', str(root / 'gh.exe'), str(source)], check=True, capture_output=True)
            wrapper = root / 'operator.ps1'
            wrapper.write_text('''param([string]$Script,[string]$Key,[string]$Mode)
$ErrorActionPreference = 'Stop'
[Console]::InputEncoding = [Text.UTF8Encoding]::new($true)
function Read-Host { param([string]$Prompt,[switch]$AsSecureString); ConvertTo-SecureString $env:KUKURI_OPERATOR_FIXTURE_PASSWORD -AsPlainText -Force }
try {
    if ($Mode -eq 'existing') { & $Script -KeystorePath $Key -Repository fixture/unused -RegisterExisting }
    else { & $Script -KeystorePath $Key -Repository fixture/unused }
    if (Test-Path Env:KUKURI_UPLOAD_KEY_PASSWORD) { throw 'Password environment was retained.' }
} catch {
    if (Test-Path Env:KUKURI_UPLOAD_KEY_PASSWORD) { throw 'Failure retained password environment.' }
    Write-Output $_.Exception.Message
    exit 1
}
''', encoding='utf-8-sig')
            for shell in ('powershell.exe', 'pwsh.exe'):
                with self.subTest(shell=shell):
                    executable = shutil.which(shell)
                    self.assertIsNotNone(executable, shell)
                    log = root / (shell + '.jsonl')
                    key = root / ('key with spaces ' + shell + '.jks')
                    env = {**os.environ, 'PATH': str(root) + os.pathsep + os.environ['PATH'],
                           'KUKURI_FAKE_GH_LOG': str(log), 'KUKURI_OPERATOR_FIXTURE_PASSWORD': password}
                    env = {name: value for name, value in env.items() if name.upper() != 'PSMODULEPATH'}
                    def invoke(mode, supplied_password=password):
                        result = subprocess.run([executable, '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File',
                                                 str(wrapper), '-Script', str(script), '-Key', str(key), '-Mode', mode],
                                                capture_output=True, env={**env, 'KUKURI_OPERATOR_FIXTURE_PASSWORD': supplied_password},
                                                timeout=30)
                        self.assertNotIn(password.encode('utf-8'), result.stdout + result.stderr)
                        return result
                    created = invoke('create')
                    self.assertEqual(created.returncode, 0, (created.stdout + created.stderr).decode('utf-8', errors='replace'))
                    key_hash = package.sha256(key)
                    self.assertEqual(invoke('existing').returncode, 0)
                    self.assertEqual(package.sha256(key), key_hash)
                    self.assertNotEqual(invoke('create').returncode, 0)
                    self.assertNotEqual(invoke('existing', 'incorrect-fixture-password').returncode, 0)
                    entries = [json.loads(line) for line in log.read_text().splitlines()]
                    self.assertEqual(len(entries), 8)
                    for group in (entries[:4], entries[4:]):
                        self.assertEqual(group[0]['args'], ['secret', 'set', 'ANDROID_UPLOAD_KEYSTORE_BASE64', '--repo', 'fixture/unused'])
                        import base64
                        encoded = base64.b64encode(key.read_bytes())
                        self.assertEqual(group[0]['length'], len(encoded))
                        self.assertEqual(group[0]['sha256'], hashlib.sha256(encoded).hexdigest())
                        self.assertEqual(group[1]['length'], len(password.encode('utf-8')))
                        self.assertEqual(group[1]['sha256'], hashlib.sha256(password.encode('utf-8')).hexdigest())
                        self.assertEqual(group[2]['args'][-1], 'upload')
                        with mock.patch.dict(os.environ, {'ANDROID_UPLOAD_KEYSTORE_PASSWORD': password}):
                            self.assertEqual(group[3]['args'][-1], package.certificate(key, 'upload')[0])


if __name__ == '__main__':
    unittest.main()
