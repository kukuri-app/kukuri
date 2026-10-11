param(
    [string]$KeystorePath = (Join-Path $env:USERPROFILE '.kukuri-signing/android-upload.jks'),
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
function Invoke-UploadTool([string]$Executable, [string[]]$ToolArguments, [string]$InputValue = '') {
    $startInfo = [Diagnostics.ProcessStartInfo]::new($Executable)
    $startInfo.Arguments = ($ToolArguments | ForEach-Object { '"' + ($_ -replace '(\\*)"', '$1$1\"' -replace '(\\+)$', '$1$1') + '"' }) -join ' '
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardInput = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $process = [Diagnostics.Process]::Start($startInfo)
    try {
        $output = $process.StandardOutput.ReadToEndAsync()
        $diagnostics = $process.StandardError.ReadToEndAsync()
        $writer = [IO.StreamWriter]::new($process.StandardInput.BaseStream, [Text.UTF8Encoding]::new($false))
        try { $writer.Write($InputValue) } finally { $writer.Dispose() }
        $process.WaitForExit()
        $null = $output.GetAwaiter().GetResult()
        $null = $diagnostics.GetAwaiter().GetResult()
        if ($process.ExitCode -ne 0) { throw "Upload tool failed (exit $($process.ExitCode)); any encrypted keystore is retained." }
    } finally { $process.Dispose() }
}
try {
    if ($plainPassword -cne $plainConfirmation -or $plainPassword.Length -lt 6) { throw 'Passwords differ or are too short.' }
    $env:KUKURI_UPLOAD_KEY_PASSWORD = $plainPassword
    if (-not $RegisterExisting) {
        New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($signingPath)) -Force | Out-Null
        Invoke-UploadTool $keytool @('-genkeypair', '-keystore', $signingPath, '-storetype', 'JKS', '-alias', 'upload',
            '-keyalg', 'RSA', '-keysize', '2048', '-validity', '10000', '-dname', 'CN=kukuri upload',
            '-storepass:env', 'KUKURI_UPLOAD_KEY_PASSWORD', '-keypass:env', 'KUKURI_UPLOAD_KEY_PASSWORD')
    }
    Invoke-UploadTool $keytool @('-exportcert', '-keystore', $signingPath, '-alias', 'upload',
        '-storepass:env', 'KUKURI_UPLOAD_KEY_PASSWORD', '-file', $privateCertificate)
    $publicFingerprint = (Get-FileHash -LiteralPath $privateCertificate -Algorithm SHA256).Hash.ToLower()
    $encodedKeystore = [Convert]::ToBase64String([IO.File]::ReadAllBytes($signingPath))
    $gh = (Get-Command gh).Source
    Invoke-UploadTool $gh @('secret', 'set', 'ANDROID_UPLOAD_KEYSTORE_BASE64', '--repo', $Repository) $encodedKeystore
    Invoke-UploadTool $gh @('secret', 'set', 'ANDROID_UPLOAD_KEYSTORE_PASSWORD', '--repo', $Repository) $plainPassword
    Invoke-UploadTool $gh @('variable', 'set', 'ANDROID_UPLOAD_KEY_ALIAS', '--repo', $Repository, '--body', 'upload')
    Invoke-UploadTool $gh @('variable', 'set', 'ANDROID_UPLOAD_CERT_SHA256', '--repo', $Repository, '--body', $publicFingerprint)
    Write-Output "Upload certificate SHA-256: $publicFingerprint"
    Write-Output "Encrypted keystore retained at: $signingPath"
} finally {
    Remove-Item Env:KUKURI_UPLOAD_KEY_PASSWORD -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $privateCertificate) { Remove-Item -LiteralPath $privateCertificate }
    $plainPassword = $null
    $plainConfirmation = $null
    $encodedKeystore = $null
    $password.Dispose()
    $confirmation.Dispose()
}
