"""Build receipts and post-build signing for Google Play; never submit releases."""
import argparse
import hashlib
import json
import os
import pathlib
import re
import shutil
import subprocess
import tempfile
import xml.etree.ElementTree as ET
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
ANDROID = ROOT / 'apps/desktop/src-tauri/gen/android/app'
BUNDLETOOL_SHA256 = 'a099cfa1543f55593bc2ed16a70a7c67fe54b1747bb7301f37fdfd6d91028e29'
ANDROID_NS = '{http://schemas.android.com/apk/res/android}'
PACKAGE = 'app.kukuri.android'
FILES = ('kukuri.aab', 'verification.apk', 'native-debug-symbols.zip')


def run(*args, binary=False):
    result = subprocess.run([str(arg) for arg in args], capture_output=True,
                            text=not binary, encoding=None if binary else 'utf-8',
                            errors=None if binary else 'replace')
    if result.returncode:
        # Tool diagnostics can contain certificate subjects or signing input.
        raise ValueError(f'{pathlib.Path(args[0]).stem} failed (exit {result.returncode})')
    return result.stdout


def sha256(path):
    with pathlib.Path(path).open('rb') as handle:
        return hashlib.file_digest(handle, 'sha256').hexdigest()


def production_guard(source, tag):
    if (os.environ.get('GITHUB_EVENT_NAME') not in ('push', 'workflow_dispatch')
            or os.environ.get('GITHUB_REF') != f'refs/tags/{tag}'
            or os.environ.get('GITHUB_WORKFLOW_SHA') != source):
        raise ValueError('Upload signing requires the workflow and source at the same release tag')


def fingerprint(value):
    value = value.replace(':', '').strip().lower()
    if not re.fullmatch(r'[0-9a-f]{64}', value):
        raise ValueError('Upload certificate SHA-256 is missing or invalid')
    return value


def prepare(args):
    run('git', 'diff', '--quiet')
    source = run('git', 'rev-parse', 'HEAD').strip()
    requested = os.environ.get('REQUESTED_SOURCE')
    if requested and requested != source:
        raise ValueError('Requested source mismatch')
    version = json.loads((ROOT / 'apps/desktop/src-tauri/tauri.conf.json').read_text())['version']
    tag = args.tag or f'v{version}-preview.1'
    if args.signing == 'upload':
        production_guard(source, tag)
        fingerprint(os.environ.get('ANDROID_UPLOAD_CERT_SHA256', ''))
        if os.environ.get('UPLOAD_KEY_AVAILABLE') != 'true':
            raise ValueError('Provision the upload keystore and password first')
    gate = run('cargo', 'xtask-lite', 'release-check', tag)
    match = re.search(r'android_version_code=(\d+)', gate)
    if not match:
        raise ValueError('Release gate did not return an Android versionCode')
    code = int(match[1])
    args.directory.mkdir(parents=True, exist_ok=True)
    metadata = {'source': source, 'tag': tag, 'version_name': version,
                'version_code': code, 'signing': args.signing, 'synthetic_tag': not args.tag}
    (args.directory / 'receipt.json').write_text(json.dumps(metadata, indent=2) + '\n')
    (args.directory / 'version-config.json').write_text(json.dumps({'bundle': {'android': {'versionCode': code}}}))
    for name, value in (('source', source), ('tag', tag), ('version_code', code)):
        print(f'{name}={value}')


def tool(name):
    sdk = pathlib.Path(os.environ['ANDROID_HOME'])
    executable = sdk / 'build-tools/36.1.0' / (name + ('.bat' if os.name == 'nt' and name == 'apksigner' else '.exe' if os.name == 'nt' else ''))
    if not executable.is_file():
        raise ValueError('Required Android build-tools 36.1.0 are missing')
    return executable


def java_tool(name):
    return pathlib.Path(os.environ['JAVA_HOME']) / 'bin' / (name + ('.exe' if os.name == 'nt' else ''))


def bundletool():
    path = pathlib.Path(os.environ['BUNDLETOOL_JAR'])
    if sha256(path) != BUNDLETOOL_SHA256:
        raise ValueError('Bundletool checksum mismatch')
    return path


def validate(directory, metadata):
    aab = directory / FILES[0]
    apk = directory / FILES[1]
    xml = run(java_tool('java'), '-jar', bundletool(), 'dump', 'manifest', f'--bundle={aab}', '--module=base')
    manifest = ET.fromstring(xml)
    uses = manifest.find('uses-sdk')
    app = manifest.find('application')
    expected = {'package': PACKAGE, ANDROID_NS + 'versionCode': str(metadata['version_code']),
                ANDROID_NS + 'versionName': metadata['version_name']}
    if any(manifest.get(key) != value for key, value in expected.items()):
        raise ValueError('AAB identity or version mismatch')
    if (uses is None or uses.get(ANDROID_NS + 'minSdkVersion') != '29'
            or uses.get(ANDROID_NS + 'targetSdkVersion') != '36'
            or app is None or app.get(ANDROID_NS + 'debuggable') == 'true'):
        raise ValueError('AAB SDK or release mode mismatch')
    badging = run(tool('aapt2'), 'dump', 'badging', apk)
    for expected_text in (f"name='{PACKAGE}'", f"versionCode='{metadata['version_code']}'",
                          f"versionName='{metadata['version_name']}'", "sdkVersion:'29'", "targetSdkVersion:'36'"):
        if expected_text not in badging:
            raise ValueError('Verification APK identity, SDK or version mismatch')
    for path, prefix in ((aab, 'base/lib/'), (apk, 'lib/')):
        with zipfile.ZipFile(path) as archive:
            abis = {name.split('/')[len(prefix.split('/')) - 1] for name in archive.namelist()
                    if name.startswith(prefix) and name.endswith('.so')}
            if abis != {'arm64-v8a'}:
                raise ValueError('Artifact ABI mismatch')
    with zipfile.ZipFile(directory / FILES[2]) as archive:
        symbols = [name for name in archive.namelist() if name.endswith('.dbg') or name.endswith('.so')]
        if not symbols or any(not name.startswith('arm64-v8a/') for name in symbols):
            raise ValueError('Native debug symbols are missing or have the wrong ABI')


def collect(args):
    metadata = json.loads((args.directory / 'receipt.json').read_text())
    for relative, target in (('bundle/universalRelease/app-universal-release.aab', FILES[0]),
                             ('apk/universal/release/app-universal-release-unsigned.apk', FILES[1]),
                             ('native-debug-symbols/universalRelease/native-debug-symbols.zip', FILES[2])):
        shutil.copyfile(ANDROID / 'build/outputs' / relative, args.directory / target)
    validate(args.directory, metadata)
    metadata['unsigned_sha256'] = {name: sha256(args.directory / name) for name in FILES}
    metadata['toolchain'] = {'rust': run('rustc', '--version').strip(),
                             'tauri_cli': json.loads((ROOT / 'apps/desktop/node_modules/@tauri-apps/cli/package.json').read_text())['version'],
                             'ndk': '29.0.14206865', 'build_tools': '36.1.0',
                             'gradle': '9.6.1', 'agp': '9.3.1',
                             'java': run(java_tool('java'), '--version').splitlines()[0]}
    (args.directory / 'receipt.json').write_text(json.dumps(metadata, indent=2) + '\n')
    (args.directory / 'version-config.json').unlink()


def certificate(keystore, alias):
    der = run(java_tool('keytool'), '-exportcert', '-keystore', keystore, '-alias', alias,
              '-storepass:env', 'ANDROID_UPLOAD_KEYSTORE_PASSWORD', binary=True)
    # Only the hash is published; subject/issuer may contain personal information.
    subject = run(java_tool('keytool'), '-J-Duser.language=en', '-list', '-v', '-keystore', keystore,
                  '-alias', alias, '-storepass:env', 'ANDROID_UPLOAD_KEYSTORE_PASSWORD')
    debug = bool(re.search(r'Owner:.*CN=(Android Debug|kukuri CI test)(?:,|\r?\n|$)', subject))
    return hashlib.sha256(der).hexdigest(), debug


def sign(args):
    metadata = json.loads((args.directory / 'receipt.json').read_text())
    source = run('git', 'rev-parse', 'HEAD').strip()
    if metadata['source'] != source or metadata['signing'] != args.signing:
        raise ValueError('Unsigned artifact source or purpose mismatch')
    if args.signing == 'upload':
        production_guard(source, metadata['tag'])
    for name in FILES:
        if sha256(args.directory / name) != metadata['unsigned_sha256'][name]:
            raise ValueError('Unsigned artifact checksum mismatch')
    validate(args.directory, metadata)
    with tempfile.TemporaryDirectory(prefix='kukuri-android-sign-', dir=os.environ.get('RUNNER_TEMP')) as private:
        keystore = pathlib.Path(private) / 'upload.p12'
        alias = 'upload' if args.signing == 'test' else os.environ.get('ANDROID_UPLOAD_KEY_ALIAS', '')
        if not re.fullmatch(r'[A-Za-z0-9_.-]+', alias):
            raise ValueError('Invalid signing alias')
        if args.signing == 'test':
            os.environ['ANDROID_UPLOAD_KEYSTORE_PASSWORD'] = os.urandom(32).hex()
            run(java_tool('keytool'), '-genkeypair', '-keystore', keystore, '-alias', alias,
                '-storetype', 'PKCS12', '-keyalg', 'RSA', '-keysize', '2048', '-validity', '10000',
                '-dname', 'CN=kukuri CI test', '-storepass:env', 'ANDROID_UPLOAD_KEYSTORE_PASSWORD')
        else:
            import base64
            keystore.write_bytes(base64.b64decode(os.environ.pop('ANDROID_UPLOAD_KEYSTORE_BASE64'), validate=True))
        expected, debug = certificate(keystore, alias)
        if args.signing == 'upload' and (debug or expected != fingerprint(os.environ.get('ANDROID_UPLOAD_CERT_SHA256', ''))):
            raise ValueError('Upload certificate mismatch or debug/test certificate')
        run(java_tool('jarsigner'), '-keystore', keystore, '-storepass:env', 'ANDROID_UPLOAD_KEYSTORE_PASSWORD',
            '-keypass:env', 'ANDROID_UPLOAD_KEYSTORE_PASSWORD', '-sigalg', 'SHA256withRSA', '-digestalg', 'SHA-256',
            args.directory / FILES[0], alias)
        run(java_tool('jarsigner'), '-verify', '-strict', '-keystore', keystore,
            '-storepass:env', 'ANDROID_UPLOAD_KEYSTORE_PASSWORD', args.directory / FILES[0])
        unsigned_apk = args.directory / 'unsigned.apk'
        (args.directory / FILES[1]).rename(unsigned_apk)
        try:
            run(tool('apksigner'), 'sign', '--ks', keystore, '--ks-key-alias', alias,
                '--ks-pass', 'env:ANDROID_UPLOAD_KEYSTORE_PASSWORD', '--key-pass', 'env:ANDROID_UPLOAD_KEYSTORE_PASSWORD',
                '--out', args.directory / FILES[1], unsigned_apk)
            result = run(tool('apksigner'), 'verify', '--print-certs', args.directory / FILES[1])
            if f'Signer #1 certificate SHA-256 digest: {expected}' not in result:
                raise ValueError('Verification APK certificate mismatch')
        finally:
            unsigned_apk.unlink(missing_ok=True)
    os.environ.pop('ANDROID_UPLOAD_KEYSTORE_PASSWORD', None)
    validate(args.directory, metadata)
    metadata.update({'package': PACKAGE, 'min_sdk': 29, 'target_sdk': 36, 'abi': 'arm64-v8a',
                     'certificate_sha256': expected, 'apk_purpose': 'verification-only-not-a-Play-installer'})
    metadata['sha256'] = {name: sha256(args.directory / name) for name in FILES}
    (args.directory / 'receipt.json').write_text(json.dumps(metadata, indent=2) + '\n')
    (args.directory / 'SHA256SUMS').write_text(''.join(f'{value}  {name}\n' for name, value in metadata['sha256'].items()))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('prepare', 'collect', 'sign'))
    parser.add_argument('--directory', type=pathlib.Path, required=True)
    parser.add_argument('--signing', choices=('test', 'upload'), default='test')
    parser.add_argument('--tag', default='')
    args = parser.parse_args()
    try:
        {'prepare': prepare, 'collect': collect, 'sign': sign}[args.action](args)
    except (ValueError, KeyError, OSError, ET.ParseError, zipfile.BadZipFile):
        # Do not leak input, certificate DN, environment or signing diagnostics.
        parser.exit(1, 'Android package validation/signing failed; verify source, identity and provisioning.\n')


if __name__ == '__main__':
    main()
