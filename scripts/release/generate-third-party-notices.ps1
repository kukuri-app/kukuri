param(
  [string]$OutputPath,
  [string[]]$RustMetadataPath,
  [string]$NpmLicensesPath,
  [string]$AssetManifestPath,
  [switch]$Check
)

$ErrorActionPreference = "Stop"

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$desktopDir = Join-Path $repoRoot "apps\desktop"
if ([string]::IsNullOrWhiteSpace($OutputPath)) {
  $OutputPath = Join-Path $repoRoot "docs\THIRD_PARTY_NOTICES.md"
}
if ([string]::IsNullOrWhiteSpace($AssetManifestPath)) {
  $AssetManifestPath = Join-Path $repoRoot "docs\ASSET_MANIFEST.json"
}

function Escape-MarkdownCell {
  param([AllowNull()][string]$Value)

  if ([string]::IsNullOrWhiteSpace($Value)) {
    return "-"
  }
  return ($Value.Trim() -replace "\|", "\|" -replace "`r?`n", " ")
}

# Sort inventory rows with culture-invariant ordinal comparison and drop
# duplicates by (Name, Version, License). Sort-Object uses culture-aware
# collation whose order differs between Windows PowerShell 5.1 and pwsh 7, which
# made the generated file depend on the host. Ordinal comparison keeps the
# output identical across platforms so the CI -Check gate is deterministic.
function Sort-Inventory {
  param([object[]]$Items)

  $list = [System.Collections.Generic.List[object]]::new()
  if ($Items) { $list.AddRange([object[]]$Items) }

  $comparison = [System.Comparison[object]] {
    param($a, $b)
    $c = [string]::CompareOrdinal([string]$a.Name, [string]$b.Name)
    if ($c -ne 0) { return $c }
    $c = [string]::CompareOrdinal([string]$a.Version, [string]$b.Version)
    if ($c -ne 0) { return $c }
    return [string]::CompareOrdinal([string]$a.License, [string]$b.License)
  }
  $list.Sort($comparison)

  $result = [System.Collections.Generic.List[object]]::new()
  $previousKey = $null
  foreach ($item in $list) {
    $key = "$($item.Name)|$($item.Version)|$($item.License)"
    if ($key -ne $previousKey) {
      $result.Add($item)
      $previousKey = $key
    }
  }
  return , $result.ToArray()
}

function Get-CargoMetadata {
  if ($RustMetadataPath.Count -gt 0) {
    foreach ($metadataPath in $RustMetadataPath) {
      Get-Content -LiteralPath $metadataPath -Raw -Encoding UTF8 | ConvertFrom-Json
    }
    return
  }

  Push-Location $repoRoot
  try {
    # Tauri is an independent workspace; root metadata cannot see its GTK,
    # portal, updater, or other desktop-only dependencies.
    foreach ($manifest in @("Cargo.toml", "apps/desktop/src-tauri/Cargo.toml")) {
      $metadataJson = (& cargo metadata --locked --format-version 1 --manifest-path $manifest | Out-String)
      if ($LASTEXITCODE -ne 0) {
        throw "cargo metadata failed for $manifest with exit code $LASTEXITCODE"
      }
      $metadataJson | ConvertFrom-Json
    }
  } finally {
    Pop-Location
  }
}

function Get-NpmLicenses {
  if (-not [string]::IsNullOrWhiteSpace($NpmLicensesPath)) {
    return Get-Content -LiteralPath $NpmLicensesPath -Raw -Encoding UTF8 | ConvertFrom-Json
  }

  $licensesJson = (& npx pnpm@10.16.1 --dir $desktopDir licenses list --prod --json | Out-String)
  if ($LASTEXITCODE -ne 0) {
    throw "pnpm licenses list failed with exit code $LASTEXITCODE"
  }
  return $licensesJson | ConvertFrom-Json
}

function Get-RustInventory {
  $metadata = Get-CargoMetadata
  $packages = @($metadata.packages) |
    Where-Object { $null -ne $_.source } |
    ForEach-Object {
      [pscustomobject]@{
        Name = [string]$_.name
        Version = [string]$_.version
        License = if ([string]::IsNullOrWhiteSpace($_.license)) { "UNKNOWN" } else { [string]$_.license }
        Source = "https://crates.io/crates/$($_.name)"
      }
    }

  return @(Sort-Inventory $packages)
}

function Get-NpmInventory {
  $licenses = Get-NpmLicenses
  $packages = foreach ($licenseGroup in $licenses.PSObject.Properties) {
    foreach ($package in @($licenseGroup.Value)) {
      foreach ($version in @($package.versions)) {
        [pscustomobject]@{
          Name = [string]$package.name
          Version = [string]$version
          License = if ([string]::IsNullOrWhiteSpace($package.license)) { [string]$licenseGroup.Name } else { [string]$package.license }
          Source = if ([string]::IsNullOrWhiteSpace($package.homepage)) { "-" } else { [string]$package.homepage }
        }
      }
    }
  }

  return @(Sort-Inventory $packages)
}

function Get-AssetManifest {
  if (-not (Test-Path -LiteralPath $AssetManifestPath)) {
    throw "Asset manifest does not exist: $AssetManifestPath"
  }
  $manifest = Get-Content -LiteralPath $AssetManifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
  if ([int]$manifest.schema_version -ne 1) {
    throw "Unsupported asset manifest schema version: $($manifest.schema_version)"
  }
  return $manifest
}

function Sort-AssetInventory {
  param([object[]]$Items)

  $list = [System.Collections.Generic.List[object]]::new()
  if ($Items) { $list.AddRange([object[]]$Items) }
  $comparison = [System.Comparison[object]] {
    param($a, $b)
    return [string]::CompareOrdinal([string]$a.Id, [string]$b.Id)
  }
  $list.Sort($comparison)
  return , $list.ToArray()
}

function Get-CommonDirectorySummary {
  param(
    [string[]]$Paths,
    [string]$Distribution
  )

  if ($Paths.Count -eq 1) {
    return "$($Distribution): $($Paths[0])"
  }
  $directories = @($Paths | ForEach-Object {
      $segments = @($_ -split "/")
      if ($segments.Count -le 1) { return "" }
      return ($segments[0..($segments.Count - 2)] -join "/")
    })
  $common = @($directories[0] -split "/")
  foreach ($directory in $directories | Select-Object -Skip 1) {
    $segments = @($directory -split "/")
    $limit = [Math]::Min($common.Count, $segments.Count)
    $matched = 0
    while ($matched -lt $limit -and $common[$matched] -ceq $segments[$matched]) {
      $matched++
    }
    if ($matched -eq 0) {
      $common = @()
      break
    }
    $common = @($common[0..($matched - 1)])
  }
  if ($common.Count -eq 0) {
    return "$($Distribution): $($Paths.Count) files"
  }
  return "$($Distribution): $(($common -join "/"))/** ($($Paths.Count) files)"
}

function Get-AssetFileSummary {
  param([object[]]$Files)

  $parts = [System.Collections.Generic.List[string]]::new()
  foreach ($distribution in @("source-only", "bundled-binary")) {
    $paths = @($Files |
        Where-Object { [string]$_.distribution -eq $distribution } |
        ForEach-Object { [string]$_.path })
    if ($paths.Count -gt 0) {
      $parts.Add((Get-CommonDirectorySummary -Paths $paths -Distribution $distribution)) | Out-Null
    }
  }
  return $parts -join "; "
}

function Get-AssetInventory {
  $manifest = Get-AssetManifest
  $items = foreach ($asset in @($manifest.assets)) {
    $origin = [string]$asset.origin
    $category = switch ($origin) {
      "authored" { "First-party" }
      "third_party" { "Third-party" }
      "generated" { "Generated" }
      "generated_assisted" { "Generated" }
      default { throw "Unsupported asset origin in notice generator: $origin" }
    }
    $sourceDate = if (-not [string]::IsNullOrWhiteSpace([string]$asset.source.acquired_on)) {
      "acquired $($asset.source.acquired_on)"
    } else {
      "created $($asset.source.created_on)"
    }
    $source = if ([string]::IsNullOrWhiteSpace([string]$asset.source.url)) {
      $sourceDate
    } else {
      "$($asset.source.url) ($sourceDate)"
    }
    $licenseReference = if (-not [string]::IsNullOrWhiteSpace([string]$asset.license.text_url)) {
      [string]$asset.license.text_url
    } else {
      [string]$asset.license.text_path
    }
    $modification = if ([bool]$asset.modification.modified) {
      [string]$asset.modification.description
    } else {
      "None"
    }
    $credit = if ([bool]$asset.license.attribution_required) {
      "Required: $($asset.license.attribution)"
    } elseif (-not [string]::IsNullOrWhiteSpace([string]$asset.license.attribution)) {
      "Not required; $($asset.license.attribution)"
    } else {
      "Not required"
    }
    $files = @($asset.files)
    $hasBundledFiles = @($files | Where-Object { [string]$_.distribution -eq "bundled-binary" }).Count -gt 0
    if (-not [bool]$asset.license.commercial_use) {
      throw "Asset is not approved for commercial use: $($asset.id)"
    }
    if (-not [bool]$asset.license.repository_redistribution) {
      throw "Asset is not approved for repository redistribution: $($asset.id)"
    }
    if ($hasBundledFiles -and -not [bool]$asset.license.binary_redistribution) {
      throw "Bundled asset is not approved for binary redistribution: $($asset.id)"
    }
    if ([bool]$asset.license.attribution_required -and [string]::IsNullOrWhiteSpace([string]$asset.license.attribution)) {
      throw "Asset requires attribution text: $($asset.id)"
    }
    $binaryRedistribution = if ([bool]$asset.license.binary_redistribution) { "yes" } else { "no" }
    [pscustomobject]@{
      Id = [string]$asset.id
      Name = [string]$asset.display_name
      Category = $category
      Author = [string]$asset.author
      RightsHolder = [string]$asset.rights_holder
      Source = $source
      License = [string]$asset.license.id
      LicenseReference = $licenseReference
      Modification = $modification
      Distribution = Get-AssetFileSummary -Files $files
      Conditions = "commercial=yes; repository redistribution=yes; binary redistribution=$binaryRedistribution"
      Credit = $credit
      HasBundledFiles = $hasBundledFiles
    }
  }
  return @(Sort-AssetInventory $items)
}

function Add-InventorySection {
  param(
    [System.Collections.Generic.List[string]]$Lines,
    [string]$Title,
    [object[]]$Items
  )

  $Lines.Add("## $Title") | Out-Null
  $Lines.Add("") | Out-Null
  $Lines.Add("Total packages: $($Items.Count)") | Out-Null
  $Lines.Add("") | Out-Null
  $Lines.Add("| Package | Version | License | Source |") | Out-Null
  $Lines.Add("| --- | --- | --- | --- |") | Out-Null
  foreach ($item in $Items) {
    $Lines.Add(
      "| $(Escape-MarkdownCell $item.Name) | $(Escape-MarkdownCell $item.Version) | $(Escape-MarkdownCell $item.License) | $(Escape-MarkdownCell $item.Source) |"
    ) | Out-Null
  }
  $Lines.Add("") | Out-Null
}

function Add-AssetSection {
  param(
    [System.Collections.Generic.List[string]]$Lines,
    [string]$Title,
    [object[]]$Items
  )

  $Lines.Add("### $Title") | Out-Null
  $Lines.Add("") | Out-Null
  if ($Items.Count -eq 0) {
    $Lines.Add("None.") | Out-Null
    $Lines.Add("") | Out-Null
    return
  }
  $Lines.Add("| Asset | Author / rights holder | Source / date | License | Modification | Distribution | Conditions | Credit |") | Out-Null
  $Lines.Add("| --- | --- | --- | --- | --- | --- | --- | --- |") | Out-Null
  foreach ($item in $Items) {
    $Lines.Add(
      "| $(Escape-MarkdownCell $item.Name) | $(Escape-MarkdownCell "$($item.Author) / $($item.RightsHolder)") | $(Escape-MarkdownCell $item.Source) | $(Escape-MarkdownCell "$($item.License) ($($item.LicenseReference))") | $(Escape-MarkdownCell $item.Modification) | $(Escape-MarkdownCell $item.Distribution) | $(Escape-MarkdownCell $item.Conditions) | $(Escape-MarkdownCell $item.Credit) |"
    ) | Out-Null
  }
  $Lines.Add("") | Out-Null
}

$rustInventory = Get-RustInventory
$npmInventory = Get-NpmInventory
$assetInventory = Get-AssetInventory
$unknown = @($rustInventory + $npmInventory | Where-Object { $_.License -eq "UNKNOWN" })
if ($unknown.Count -gt 0) {
  $names = ($unknown | ForEach-Object { "$($_.Name)@$($_.Version)" }) -join ", "
  throw "Third-party license inventory contains UNKNOWN licenses: $names"
}

$lines = [System.Collections.Generic.List[string]]::new()
$lines.Add("# Third-party notices") | Out-Null
$lines.Add("") | Out-Null
$lines.Add("kukuri preview builds include Rust crates, npm packages, Tauri runtime components, and non-code assets.") | Out-Null
$lines.Add("") | Out-Null
$lines.Add("This file is generated from the locked root and Tauri Rust workspaces, desktop npm dependency inventories, and docs/ASSET_MANIFEST.json. It includes dependencies across platforms, not only packages linked into one binary. OS libraries and codecs bundled into an AppImage require a separate package-level inventory.") | Out-Null
$lines.Add("") | Out-Null
$lines.Add("Regenerate it from the repository root with:") | Out-Null
$lines.Add("") | Out-Null
$lines.Add('```powershell') | Out-Null
$lines.Add("./scripts/release/generate-third-party-notices.ps1") | Out-Null
$lines.Add('```') | Out-Null
$lines.Add("") | Out-Null
$lines.Add("Release owners must review these inventories before publishing a preview build and update the manifest or generator if a package or asset requires additional license or attribution text.") | Out-Null
$lines.Add("") | Out-Null
$lines.Add("## Current distribution note") | Out-Null
$lines.Add("") | Out-Null
$lines.Add("Windows preview builds use installer distribution through GitHub Releases. Linux x86_64 AppImage generation and validation are tracked in #889; public release integration remains with #890. A locally validated AppImage does not establish public release readiness. If Windows code signing is not configured, the release notes must state that the preview is unsigned and that SmartScreen warnings are expected.") | Out-Null
$lines.Add("") | Out-Null

$lines.Add("## Non-code asset notices") | Out-Null
$lines.Add("") | Out-Null
$lines.Add("The asset manifest records exact repository paths, SHA-256 digests, provenance, and source-only versus bundled-binary scope. The entries below are the distribution-facing license and credit summary.") | Out-Null
$lines.Add("") | Out-Null

$bundledFirstParty = @($assetInventory | Where-Object { $_.HasBundledFiles -and $_.Category -eq "First-party" })
$bundledThirdParty = @($assetInventory | Where-Object { $_.HasBundledFiles -and $_.Category -eq "Third-party" })
$bundledGenerated = @($assetInventory | Where-Object { $_.HasBundledFiles -and $_.Category -eq "Generated" })
$sourceOnly = @($assetInventory | Where-Object { -not $_.HasBundledFiles })
Add-AssetSection -Lines $lines -Title "Bundled first-party assets" -Items $bundledFirstParty
Add-AssetSection -Lines $lines -Title "Bundled third-party assets" -Items $bundledThirdParty
Add-AssetSection -Lines $lines -Title "Bundled generated or generation-assisted assets" -Items $bundledGenerated
Add-AssetSection -Lines $lines -Title "Source-only non-code assets" -Items $sourceOnly

$lines.AddRange([string[]](Get-Content -LiteralPath (Join-Path $repoRoot "docs/licenses/cn-search.md") -Encoding UTF8))
$lines.Add("") | Out-Null

Add-InventorySection -Lines $lines -Title "Rust crates" -Items $rustInventory
Add-InventorySection -Lines $lines -Title "Desktop npm packages" -Items $npmInventory

$content = ($lines -join "`n").TrimEnd() + "`n"

if ($Check) {
  if (-not (Test-Path -LiteralPath $OutputPath)) {
    throw "Third-party notices file does not exist: $OutputPath"
  }
  $existing = Get-Content -LiteralPath $OutputPath -Raw -Encoding UTF8
  $normalizedExisting = ($existing -replace "`r`n", "`n").TrimEnd()
  $normalizedContent = ($content -replace "`r`n", "`n").TrimEnd()
  if ($normalizedExisting -ne $normalizedContent) {
    throw "Third-party notices are out of date. Run ./scripts/release/generate-third-party-notices.ps1"
  }
  Write-Host "Third-party notices are up to date"
  exit 0
}

$outputDirectory = Split-Path -Parent $OutputPath
New-Item -ItemType Directory -Force -Path $outputDirectory | Out-Null
Set-Content -LiteralPath $OutputPath -Value $content -Encoding UTF8 -NoNewline
Write-Host "Wrote third-party notices to $OutputPath"
