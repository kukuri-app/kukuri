# Game SDK の性能（ADR 0070 §8.4）を Cyber Disc Arena で測る（#1984 AC-1）。
# 指定の PC 1 台で、対戦者 2 人（1920×1080）と視聴者 5 人（1280×720）の 7 つの Player（FFI）、スキンを発行する開発用の
# Sidecar、計測用の UDP proxy（kukuri-sdk-net-proxy）を動かす。全 process の packet は proxy を通り、片道 22.5〜27.5 ms の
# 遅延と方向ごとに 1% の損失がかかる（relay と DHT は使わないので、経路は Direct P2P だけ）。
# 1 回: host が試合を配信し、視聴者 5 人が見始めてから 10 秒の準備の後に 3 分を測る（直接の枠は 4 人なので、5 人目は転送で
# 受ける）。測り始めて 60 秒で作者が 5 MiB の glb のスキンの新しい版を発行し、対戦者 2 人が適用する。測った後に proxy で全
# packet を 3 秒落とし、対戦と視聴の復帰を測る。これを 5 回くり返し、別に同じ負荷を 10 分続けて memory の増加を測る。
# 試合は 3 分で終わるので、測る間は host の probe が残り時間と得点を戻して続ける（負荷は変えない）。
# 結果は <ProjectDir>\runs\report.md と標準出力に書く。先に sdk/unity/build-native.ps1 で native と proxy を置く。
param(
    [string]$Unity = "C:\Program Files\Unity\Hub\Editor\6000.3.27f1\Editor\Unity.exe",
    [string]$ProjectDir = (Join-Path $env:TEMP "kcda-slo"),
    [ValidateSet("Mono2x", "IL2CPP")] [string]$Backend = "IL2CPP",
    [string]$Proxy = (Join-Path $PSScriptRoot "..\..\target\x86_64-pc-windows-msvc\release\kukuri-sdk-net-proxy.exe"),
    [int]$Runs = 5,
    [int]$MeasureSeconds = 180,
    [int]$MemorySeconds = 600,
    [switch]$SkipBuild,
    # 終わった回（exits のある回）は回し直さずに集計だけする。
    [switch]$Resume
)
$ErrorActionPreference = "Stop"
$package = (Resolve-Path (Join-Path $PSScriptRoot "..\..\sdk\unity\com.kukuri.gamesdk")).Path
if (-not (Test-Path (Join-Path $package "Runtime\Plugins\x86_64\kukuri_game_sdk.dll"))) { throw "run sdk/unity/build-native.ps1 first" }
if (-not (Test-Path $Proxy)) { throw "build kukuri-sdk-net-proxy first (sdk/unity/build-native.ps1)" }
$exe = Join-Path $ProjectDir "Builds\$Backend\CyberDiscArena.exe"

if (-not $SkipBuild) {
    if (Test-Path $ProjectDir) { Remove-Item -Recurse -Force $ProjectDir }
    New-Item -ItemType Directory -Force $ProjectDir | Out-Null
    foreach ($part in "Assets", "Packages", "ProjectSettings") { Copy-Item -Recurse (Join-Path $PSScriptRoot $part) $ProjectDir }
    $manifestPath = Join-Path $ProjectDir "Packages\manifest.json"
    $manifest = Get-Content $manifestPath -Raw | ConvertFrom-Json
    $manifest.dependencies."com.kukuri.gamesdk" = "file:" + ($package -replace '\\', '/')
    [IO.File]::WriteAllText($manifestPath, ($manifest | ConvertTo-Json -Depth 8), (New-Object System.Text.UTF8Encoding $false))
    $probeDir = Join-Path $ProjectDir "Assets\SloProbe"
    New-Item -ItemType Directory -Force (Join-Path $probeDir "Editor") | Out-Null
    Copy-Item (Join-Path $PSScriptRoot "slo\SloProbe.cs") $probeDir
    Copy-Item (Join-Path $PSScriptRoot "slo\SloProbeBuild.cs") (Join-Path $probeDir "Editor")
    (Get-Content (Join-Path $probeDir "Editor\SloProbeBuild.cs") -Raw) -replace "__BACKEND__", $Backend |
        Set-Content (Join-Path $probeDir "Editor\SloProbeBuild.cs") -Encoding utf8
    & $Unity -batchmode -nographics -quit -projectPath $ProjectDir -executeMethod SloProbeBuild.Build -logFile (Join-Path $ProjectDir "build.log") | Out-Null
    if (-not (Test-Path $exe)) { throw "the Player was not built; see $ProjectDir\build.log" }
}

$env:KUKURI_DISABLE_KEYRING = "1"
$env:KUKURI_DISCOVERY_MODE = "static_peer"
$env:KUKURI_ADVERTISE_HOST = "127.0.0.1"
foreach ($name in "KUKURI_APP_DATA_DIR", "KUKURI_INSTANCE", "KUKURI_DISCOVERY_SEEDS", "KUKURI_BIND_ADDR") { Remove-Item "env:$name" -ErrorAction SilentlyContinue }
$sidecarExe = Join-Path $package "Editor\Sidecar~\kukuri-sdk-sidecar.exe"
$roles = [ordered]@{ "host" = 1080; "client" = 1080; "viewer1" = 720; "viewer2" = 720; "viewer3" = 720; "viewer4" = 720; "viewer5" = 720 }

# 1 回分の process を動かし、その作業 dir を返す。
function Invoke-Run([string]$name, [int]$seconds, [bool]$blackout) {
    $work = Join-Path $ProjectDir "runs\$name"
    if ($Resume -and (Test-Path (Join-Path $work "exits"))) { return $work }
    if (Test-Path $work) { Remove-Item -Recurse -Force $work }
    New-Item -ItemType Directory -Force $work | Out-Null
    $port = Get-Random -Minimum 40000 -Maximum 60000
    $env:KUKURI_TEST_UDP_PROXY = "127.0.0.1:$port"
    $env:KUKURI_ADVERTISE_PORT = "$port"
    $info = New-Object System.Diagnostics.ProcessStartInfo $Proxy, "--listen 127.0.0.1:$port --stats `"$(Join-Path $work "proxy.stats")`""
    $info.UseShellExecute = $false
    $info.RedirectStandardInput = $true
    $info.RedirectStandardOutput = $true
    $proxyProcess = [System.Diagnostics.Process]::Start($info)
    $frequency = $proxyProcess.StandardOutput.ReadLine()
    Set-Content (Join-Path $work "frequency") ($frequency -replace "frequency ", "")
    $publisherDir = Join-Path $work "publisher"
    $sidecar = Start-Process -FilePath $sidecarExe -ArgumentList "run --profile publisher --profile-dir `"$publisherDir`"" -PassThru -RedirectStandardError (Join-Path $work "sidecar.err")
    for ($i = 0; $i -lt 120 -and -not (Select-String -Path (Join-Path $work "sidecar.err") -Pattern "^ready" -Quiet -ErrorAction SilentlyContinue); $i++) { Start-Sleep -Milliseconds 500 }
    $topic = "kukuri:topic:test-slo-" + [guid]::NewGuid().ToString("N").Substring(0, 8)
    $players = @{}
    foreach ($role in $roles.Keys) {
        $height = $roles[$role]
        $width = if ($height -eq 1080) { 1920 } else { 1280 }
        $scene = if ($role -like "viewer*") { "-slo-scene ArenaStreamViewer " } else { "" }
        $arguments = "-screen-fullscreen 0 -screen-width $width -screen-height $height -slo-role $role $scene-slo-profile-dir `"$(Join-Path $work $role)`" " +
            "-slo-topic $topic -slo-work `"$work`" -slo-publisher-dir `"$publisherDir`" -slo-seconds $seconds -slo-blackout $([int]$blackout) -logFile `"$(Join-Path $work "$role.log")`""
        $players[$role] = Start-Process -FilePath $exe -ArgumentList $arguments -PassThru
    }
    # host が測り終えたら、proxy で全 packet を 3 秒落とす。
    $deadline = (Get-Date).AddSeconds($seconds + 900)
    while ($blackout -and -not (Test-Path (Join-Path $work "blackout-request")) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 200 }
    if ($blackout -and (Test-Path (Join-Path $work "blackout-request"))) {
        $proxyProcess.StandardInput.WriteLine("blackout 3000")
        $line = $proxyProcess.StandardOutput.ReadLine()
        Set-Content (Join-Path $work "blackout") ($line -replace "blackout ", "")
    }
    $exits = @{}
    foreach ($role in $roles.Keys) {
        $remaining = [Math]::Max(1000, ($deadline - (Get-Date)).TotalMilliseconds)
        $exits[$role] = if ($players[$role].WaitForExit([int]$remaining)) { $players[$role].ExitCode } else { Stop-Process -Id $players[$role].Id -Force; "timeout" }
    }
    Start-Process -FilePath $sidecarExe -ArgumentList "stop --profile publisher --profile-dir `"$publisherDir`"" -Wait | Out-Null
    if (-not $sidecar.WaitForExit(15000)) { Stop-Process -Id $sidecar.Id -Force }
    $proxyProcess.Kill()
    Set-Content (Join-Path $work "exits") (($roles.Keys | ForEach-Object { "$_ $($exits[$_])" }) -join "`n")
    return $work
}

# --- 集計 ---
function Percentile([double[]]$values, [double]$p) {
    if ($values.Count -eq 0) { return [double]::NaN }
    $sorted = [double[]]$values.Clone()
    [Array]::Sort($sorted)
    $sorted[[Math]::Min($sorted.Count - 1, [Math]::Ceiling($p * $sorted.Count) - 1)]
}

# `<kind> <QPC> <値...>` の行を種類ごとに読む。
function Read-Samples([string]$path) {
    $table = @{}
    foreach ($line in [IO.File]::ReadLines($path)) {
        $fields = $line.Split(' ')
        if (-not $table.ContainsKey($fields[0])) { $table[$fields[0]] = New-Object System.Collections.Generic.List[object] }
        $table[$fields[0]].Add($fields)
    }
    $table
}

# 種類の行（無ければ空）。
function Rows($table, [string]$kind) { if ($table.ContainsKey($kind)) { , $table[$kind].ToArray() } else { , @() } }

function Measure-Run([string]$work) {
    # 測り始めなかった回（準備で失敗した回）は、すべての項目を未達にする。
    if (-not (Test-Path (Join-Path $work "window"))) { return [ordered]@{ measured = "no" } }
    $frequency = [double](Get-Content (Join-Path $work "frequency"))
    $ms = { param($ticks) [double]$ticks * 1000 / $frequency }
    $window = (Get-Content (Join-Path $work "window")).Split(' ') | ForEach-Object { [long]$_ }
    $inWindow = { param($t) [long]$t -ge $window[0] -and [long]$t -le $window[1] }
    $samples = @{}
    foreach ($role in $roles.Keys) { $samples[$role] = Read-Samples (Join-Path $work "$role.samples") }
    $result = [ordered]@{}
    foreach ($role in "host", "client") {
        $frames = [double[]]@((Rows $samples[$role] "f") | Where-Object { & $inWindow $_[1] } | ForEach-Object { & $ms $_[2] })
        $result["frame_p95_$role"] = Percentile $frames 0.95
        $result["frame_p99_$role"] = Percentile $frames 0.99
        $inputs = [double[]]@((Rows $samples[$role] "i") | Where-Object { & $inWindow $_[1] } | ForEach-Object { & $ms $_[2] })
        $result["input_p95_$role"] = Percentile $inputs 0.95
        $result["input_count_$role"] = $inputs.Count
    }
    # 映像: 配信者が frame に埋めた番号の時刻から、視聴者がその frame を受けた時刻まで。
    $stamps = @{}
    foreach ($fields in (Rows $samples["host"] "s")) { $stamps[$fields[2]] = [long]$fields[1] }
    $direct = New-Object System.Collections.Generic.List[double]
    $relayed = New-Object System.Collections.Generic.List[double]
    foreach ($role in $roles.Keys | Where-Object { $_ -like "viewer*" }) {
        $own = @((Rows $samples[$role] "v") | Where-Object { (& $inWindow $_[1]) -and $stamps.ContainsKey($_[2]) })
        $latency = [double[]]@($own | ForEach-Object { & $ms ([long]$_[1] - $stamps[$_[2]]) })
        $hops = @($own | ForEach-Object { [int]$_[3] })
        for ($i = 0; $i -lt $latency.Count; $i++) { if ($hops[$i] -le 1) { $direct.Add($latency[$i]) } else { $relayed.Add($latency[$i]) } }
        $result["video_p95_$role"] = Percentile $latency 0.95
        $result["video_hops_$role"] = ($hops | Sort-Object -Unique) -join "/"
    }
    $result["video_p95_direct"] = Percentile $direct.ToArray() 0.95
    $result["video_p95_relayed"] = Percentile $relayed.ToArray() 0.95
    # スキン: 発行の要求を送った時点から、各対戦者が新しい版を適用した frame まで。読み込み中の frame time の最大。
    $published = (Rows $samples["host"] "u") | Select-Object -First 1
    # 発行まで進まなかった回は、どの適用とも組にしない。
    if (-not $published) { $published = @("u", [long]::MaxValue, "none") }
    foreach ($role in "host", "client") {
        $applied = (Rows $samples[$role] "a") | Where-Object { [long]$_[1] -ge [long]$published[1] -and $_[2] -eq $published[2] } | Select-Object -First 1
        $result["asset_s_$role"] = if ($applied) { (& $ms ([long]$applied[1] - [long]$published[1])) / 1000 } else { [double]::NaN }
        $answered = (Rows $samples["host"] "w") | Select-Object -First 1
        $result["asset_publish_s"] = if ($answered) { (& $ms ([long]$answered[1] - [long]$published[1])) / 1000 } else { [double]::NaN }
        $hinted = (Rows $samples[$role] "h") | Where-Object { [long]$_[1] -ge [long]$published[1] -and $_[2] -eq $published[2] } | Select-Object -First 1
        $result["asset_hint_s_$role"] = if ($hinted) { (& $ms ([long]$hinted[1] - [long]$published[1])) / 1000 } else { [double]::NaN }
        # 適用中は、更新の知らせ（取得済みの file の path）を受けて読み込みを始めてから、適用した frame まで（2026-11-02 のユーザー判断）。
        $during = if ($hinted -and $applied) { [double[]]@((Rows $samples[$role] "f") | Where-Object { [long]$_[1] -ge [long]$hinted[1] -and [long]$_[1] -le [long]$applied[1] } | ForEach-Object { & $ms $_[2] }) } else { @() }
        $result["asset_frame_max_$role"] = if ($during.Count) { [double]($during | Measure-Object -Maximum).Maximum } else { [double]::NaN }
    }
    # 送信量（3 分の平均）: proxy が数えた、送り手と宛先の組ごとの bytes の窓の始めと終わりの差。
    $ids = @{}
    foreach ($role in $roles.Keys) { $ids[$role] = ((Rows $samples[$role] "id") | Select-Object -First 1)[2] }
    $stats = [IO.File]::ReadAllLines((Join-Path $work "proxy.stats")) | ForEach-Object { , $_.Split(' ') }
    $bytesAt = { param($source, $destination, $at)
        $last = $stats | Where-Object { $_[1] -eq $source -and $_[2] -eq $destination -and [long]$_[0] -le $at } | Select-Object -Last 1
        if ($last) { [double]$last[3] } else { 0.0 } }
    $span = (& $ms ($window[1] - $window[0])) / 1000
    $pairs = $stats | ForEach-Object { "$($_[1]) $($_[2])" } | Sort-Object -Unique
    $rate = { param($source, $destination) ((& $bytesAt $source $destination $window[1]) - (& $bytesAt $source $destination $window[0])) * 8 / $span / 1000 }
    $result["match_kbps_host"] = & $rate $ids["host"] $ids["client"]
    $result["match_kbps_client"] = & $rate $ids["client"] $ids["host"]
    $upload = 0.0
    foreach ($pair in $pairs | Where-Object { $_.StartsWith($ids["host"]) }) { $upload += & $rate $ids["host"] ($pair.Split(' ')[1]) }
    $result["broadcaster_mbps"] = $upload / 1000
    $ids["publisher"] = @((Rows $samples["host"] "pid") | ForEach-Object { $_[2] })[0]
    $result["unmapped_pairs"] = @($pairs | Where-Object { $p = $_.Split(' '); -not ($ids.Values -contains $p[0]) }).Count
    $result["publisher_upload_mbps"] = ((@($pairs | Where-Object { $_.StartsWith($ids["publisher"]) }) | ForEach-Object { & $rate $ids["publisher"] ($_.Split(' ')[1]) }) | Measure-Object -Sum).Sum / 1000
    # 復帰: 全 packet を落とした 3 秒が明けてから、対戦の packet が両方向で届くまでと、各視聴者に frame が届くまで。
    if (Test-Path (Join-Path $work "blackout")) {
        $end = [long]((Get-Content (Join-Path $work "blackout")).Split(' ')[1])
        $first = { param($role, $kind) $hit = (Rows $samples[$role] $kind) | Where-Object { [long]$_[1] -gt $end } | Select-Object -First 1
            if ($hit) { (& $ms ([long]$hit[1] - $end)) / 1000 } else { [double]::NaN } }
        $result["recover_match_s"] = [Math]::Max((& $first "host" "k"), (& $first "client" "k"))
        $result["recover_video_s"] = (@($roles.Keys | Where-Object { $_ -like "viewer*" } | ForEach-Object { & $first $_ "v" }) | Measure-Object -Maximum).Maximum
    }
    foreach ($role in $roles.Keys) {
        $memory = [double[]]@((Rows $samples[$role] "m") | Where-Object { & $inWindow $_[1] } | ForEach-Object { [double]$_[2] / 1MB })
        $result["memory_mib_$role"] = ($memory | Measure-Object -Maximum).Maximum
        $poll = [double[]]@((Rows $samples[$role] "p") | Where-Object { & $inWindow $_[1] } | ForEach-Object { [double]$_[2] })
        $result["poll_p99_$role"] = Percentile $poll 0.99
    }
    $result
}

$report = New-Object System.Collections.Generic.List[string]
$report.Add("# Game SDK の性能の計測（ADR 0070 §8.4）")
$report.Add("")
$report.Add("Backend: $Backend / Runs: $Runs × $MeasureSeconds s / memory: $MemorySeconds s / commit: $(git -C $PSScriptRoot rev-parse HEAD)")
$results = @()
for ($run = 1; $run -le $Runs; $run++) {
    $work = Invoke-Run "run$run" $MeasureSeconds $true
    $results += , (Measure-Run $work)
    Write-Output "run $run"
    $results[-1].GetEnumerator() | ForEach-Object { Write-Output ("  {0} = {1}" -f $_.Key, $_.Value) }
}
$memoryWork = Invoke-Run "memory" $MemorySeconds $false
$growth = [ordered]@{}
foreach ($role in $roles.Keys | Where-Object { Test-Path (Join-Path $memoryWork "window") }) {
    $table = Read-Samples (Join-Path $memoryWork "$role.samples")
    $window = (Get-Content (Join-Path $memoryWork "window")).Split(' ') | ForEach-Object { [long]$_ }
    $frequency = [double](Get-Content (Join-Path $memoryWork "frequency"))
    $baseAt = $window[0] + [long](60 * $frequency)
    $after = @((Rows $table "m") | Where-Object { [long]$_[1] -ge $baseAt -and [long]$_[1] -le $window[1] })
    # 増加は、1 分後からの sample に当てた直線の傾きから求めた測定の間の増え方を、平均に対する割合で表す（映像の cache の
    # 入れ替わりの上下を増加に数えない。2026-11-03 のユーザー判断）。
    $growth[$role] = if ($after.Count -gt 1) {
        $x = @($after | ForEach-Object { ([long]$_[1] - $baseAt) / $frequency }); $y = @($after | ForEach-Object { [double]$_[2] })
        $mx = ($x | Measure-Object -Average).Average; $my = ($y | Measure-Object -Average).Average
        $sxy = 0.0; $sxx = 0.0
        for ($i = 0; $i -lt $x.Count; $i++) { $sxy += ($x[$i] - $mx) * ($y[$i] - $my); $sxx += ($x[$i] - $mx) * ($x[$i] - $mx) }
        ($sxy / $sxx) * ($x[-1] - $x[0]) / $my * 100
    } else { [double]::NaN }
}

# 閾値（5 回すべてで満たす）。値が無い・NaN なら満たさない（PowerShell では $null -le 1 が真になる）。
function Le($value, [double]$limit) { $value -is [double] -and -not [double]::IsNaN($value) -and $value -le $limit }
$checks = [ordered]@{
    "frame time p95 ≤ 16.7 ms" = { param($r) (Le $r.frame_p95_host 16.7) -and (Le $r.frame_p95_client 16.7) }
    "frame time p99 ≤ 25 ms" = { param($r) (Le $r.frame_p99_host 25) -and (Le $r.frame_p99_client 25) }
    "入力の確定 p95 ≤ 100 ms" = { param($r) (Le $r.input_p95_host 100) -and (Le $r.input_p95_client 100) }
    "映像遅延 p95 直接 ≤ 500 ms" = { param($r) (Le $r.video_p95_direct 500) }
    "映像遅延 p95 転送 1 段 ≤ 800 ms" = { param($r) (Le $r.video_p95_relayed 800) }
    "適用中の frame time の最大 ≤ 33 ms" = { param($r) (Le $r.asset_frame_max_host 33) -and (Le $r.asset_frame_max_client 33) }
    "対戦 ≤ 200 kbps / 人" = { param($r) (Le $r.match_kbps_host 200) -and (Le $r.match_kbps_client 200) }
    "配信者の上り ≤ 10 Mbps" = { param($r) (Le $r.broadcaster_mbps 10) }
    "復帰 対戦 ≤ 5 s" = { param($r) (Le $r.recover_match_s 5) }
    "復帰 視聴 ≤ 20 s" = { param($r) (Le $r.recover_video_s 20) }
    "SDK の memory ≤ 256 MiB" = { param($r) @($roles.Keys | Where-Object { -not ((Le $r["memory_mib_$_"] 256)) }).Count -eq 0 }
    "poll p99 ≤ 1 ms" = { param($r) @($roles.Keys | Where-Object { -not ((Le $r["poll_p99_$_"] 1)) }).Count -eq 0 }
}
$report.Add("")
$report.Add("| 項目 | " + ((1..$Runs | ForEach-Object { "run $_" }) -join " | ") + " | 判定 |")
$report.Add("| --- | " + ((1..$Runs | ForEach-Object { "---" }) -join " | ") + " | --- |")
$allPass = $true
foreach ($name in $checks.Keys) {
    $passes = @($results | ForEach-Object { [bool](& $checks[$name] $_) })
    $verdict = if ($passes -notcontains $false) { "PASS" } else { "FAIL" }
    if ($verdict -ne "PASS") { $allPass = $false }
    $report.Add("| $name | " + (($passes | ForEach-Object { if ($_) { "○" } else { "×" } }) -join " | ") + " | $verdict |")
}
$assetSeconds = [double[]]@($results | ForEach-Object { $_.asset_s_host; $_.asset_s_client })
$assetP95 = Percentile $assetSeconds 0.95
$assetVerdict = if ((Le $assetP95 10) -and @($assetSeconds | Where-Object { [double]::IsNaN($_) }).Count -eq 0) { "PASS" } else { "FAIL" }
if ($assetVerdict -ne "PASS") { $allPass = $false }
$report.Add("| asset の反映 p95 ≤ 10 s（5 回 × 2 人） | " + (($results | ForEach-Object { "{0:F1}/{1:F1}" -f $_.asset_s_host, $_.asset_s_client }) -join " | ") + " | $assetVerdict ($("{0:F1}" -f $assetP95) s) |")
$memoryVerdict = if ($growth.Count -eq $roles.Count -and @($growth.Values | Where-Object { -not (Le $_ 5) }).Count -eq 0) { "PASS" } else { "FAIL" }
if ($memoryVerdict -ne "PASS") { $allPass = $false }
$report.Add("| memory の増加 ≤ 5%（$MemorySeconds 秒、開始 1 分後から） | " + (($growth.Keys | ForEach-Object { "{0} {1:F1}%" -f $_, $growth[$_] }) -join ", ") + " | $memoryVerdict |")
$report.Add("")
$report.Add("経路: relay と DHT を使わないので、送信量はすべて Direct P2P（Relay Supported P2P・Relay Fallback は 0）。")
$report.Add("")
$report.Add("## 値")
for ($run = 0; $run -lt $Runs; $run++) {
    $report.Add("")
    $report.Add("### run $($run + 1)")
    $results[$run].GetEnumerator() | ForEach-Object { $report.Add(("- {0}: {1}" -f $_.Key, $(if ($_.Value -is [double]) { "{0:F2}" -f $_.Value } else { $_.Value }))) }
}
$path = Join-Path $ProjectDir "runs\report.md"
[IO.File]::WriteAllLines($path, $report, (New-Object System.Text.UTF8Encoding $false))
$report | Write-Output
Write-Output "SLO ($Backend): pass=$allPass report=$path"
if (-not $allPass) { exit 1 }
