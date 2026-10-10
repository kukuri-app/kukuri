param(
    [string]$KeystorePath = (Join-Path $env:USERPROFILE '.kukuri-signing/android-upload.p12'),
    [string]$Repository = 'kukuri-app/kukuri',
    [switch]$RegisterExisting
)

$ErrorActionPreference = 'Stop'
$signingPath = [IO.Path]::GetFullPath($KeystorePath)
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..'))
if ($signingPath.StartsWith($repositoryRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Keystore must be outside the repository.'
}
if (-not $RegisterExisting -and (Test-Path -LiteralPath $signingPath)) {
    throw 'Keystore already exists; use -RegisterExisting to register it without replacing it.'
}
if ($RegisterExisting -and -not (Test-Path -LiteralPath $signingPath)) { throw 'Existing keystore not found.' }
$keytool = if ($env:JAVA_HOME) { Join-Path $env:JAVA_HOME 'bin/keytool.exe' } else { (Get-Command keytool).Source }
$password = Read-Host 'upload key password (keystore and key use the same password)' -AsSecureString
$confirmation = Read-Host 'Confirm password' -AsSecureString
$plainPassword = [PSCredential]::new('upload', $password).GetNetworkCredential().Password
$plainConfirmation = [PSCredential]::new('upload', $confirmation).GetNetworkCredential().Password
$privateCertificate = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString() + '.cer')
function Set-UploadSecret([string]$Name, [string]$Value) {
    $startInfo = [Diagnostics.ProcessStartInfo]::new('gh')
    foreach ($argument in @('secret', 'set', $Name, '--repo', $Repository)) { $startInfo.ArgumentList.Add($argument) }
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardInput = $true
    $process = [Diagnostics.Process]::Start($startInfo)
    $process.StandardInput.Write($Value)
    $process.StandardInput.Close()
    $process.WaitForExit()
    if ($process.ExitCode -ne 0) { throw 'Secret registration failed; the encrypted keystore is retained.' }
    $process.Dispose()
}
try {
    if ($plainPassword -cne $plainConfirmation -or $plainPassword.Length -lt 6) { throw 'Passwords differ or are too short.' }
    $env:KUKURI_UPLOAD_KEY_PASSWORD = $plainPassword
    if (-not $RegisterExisting) {
        New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($signingPath)) -Force | Out-Null
        $toolOutput = & $keytool -genkeypair -keystore $signingPath -storetype PKCS12 -alias upload `
            -keyalg RSA -keysize 2048 -validity 10000 -dname 'CN=kukuri upload' `
            -storepass:env KUKURI_UPLOAD_KEY_PASSWORD -keypass:env KUKURI_UPLOAD_KEY_PASSWORD 2>&1
        if ($LASTEXITCODE -ne 0) { throw 'Key generation failed.' }
    }
    $toolOutput = & $keytool -exportcert -keystore $signingPath -alias upload `
        -storepass:env KUKURI_UPLOAD_KEY_PASSWORD -file $privateCertificate 2>&1
    if ($LASTEXITCODE -ne 0) { throw 'Could not read upload certificate.' }
    $publicFingerprint = (Get-FileHash -LiteralPath $privateCertificate -Algorithm SHA256).Hash.ToLower()
    $encodedKeystore = [Convert]::ToBase64String([IO.File]::ReadAllBytes($signingPath))
    Set-UploadSecret ANDROID_UPLOAD_KEYSTORE_BASE64 $encodedKeystore
    Set-UploadSecret ANDROID_UPLOAD_KEYSTORE_PASSWORD $plainPassword
    gh variable set ANDROID_UPLOAD_KEY_ALIAS --repo $Repository --body upload
    if ($LASTEXITCODE -ne 0) { throw 'Alias registration failed.' }
    gh variable set ANDROID_UPLOAD_CERT_SHA256 --repo $Repository --body $publicFingerprint
    if ($LASTEXITCODE -ne 0) { throw 'Fingerprint registration failed.' }
    Write-Output "Upload certificate SHA-256: $publicFingerprint"
    Write-Output "Encrypted keystore retained at: $signingPath"
} finally {
    Remove-Item Env:KUKURI_UPLOAD_KEY_PASSWORD -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $privateCertificate) { Remove-Item -LiteralPath $privateCertificate }
    $plainPassword = $null
    $plainConfirmation = $null
    $encodedKeystore = $null
    $toolOutput = $null
    $password.Dispose()
    $confirmation.Dispose()
}
