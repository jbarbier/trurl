# The self-update, end to end, on a real Windows machine (CI):
#   1. build version N with a throwaway signing key and a localhost update server;
#   2. install N, add a model the way a learner would;
#   3. build N+1, sign it, publish latest.json on localhost;
#   4. start N, click "Update now" in the real window, let it install;
#   5. check N+1 is installed and runs, and the model is still there;
#   6. a teacher's update with no internet: build N+2 and run its installer over
#      N+1 from a "stick" folder with a models folder next to it. The model
#      already there stays, a new one is copied, N+2 runs.
# If 0.1.0's updater were broken, no later release could fix it remotely: this
# test is what makes the first release safe to ship.
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)
$root = Get-Location
$out = Join-Path $root "target\e2e\windows-update"
New-Item -ItemType Directory -Force $out | Out-Null
$work = Join-Path $env:RUNNER_TEMP "trurl-update-test"
$srv = Join-Path $work "srv"
New-Item -ItemType Directory -Force $srv | Out-Null
function Step($msg) { Write-Host "`n=== $msg" }
# The Tauri CLI pinned by apps/desktop/package-lock.json.
Push-Location apps/desktop
npm ci --no-audit --no-fund
Pop-Location
$tauri = (Resolve-Path "apps/desktop/node_modules/.bin/tauri.cmd").Path

Step "Throwaway signing key and test updater settings"
$key = Join-Path $work "test.key"
& $tauri signer generate --ci -w $key -p test | Out-Null
$pub = (Get-Content "$key.pub" -Raw).Trim()
$conf = Join-Path $work "update-test.conf.json"
@{ plugins = @{ updater = @{ pubkey = $pub; endpoints = @("http://127.0.0.1:8899/latest.json"); dangerousInsecureTransportProtocol = $true } } } |
  ConvertTo-Json -Depth 5 | Set-Content -Encoding utf8 $conf

$vN = (Get-Content pins.json | ConvertFrom-Json).app.version
$parts = $vN.Split(".")
$vNext = "$($parts[0]).$($parts[1]).$([int]$parts[2] + 1)"
$vNext2 = "$($parts[0]).$($parts[1]).$([int]$parts[2] + 2)"

Step "Build version $vN"
Push-Location apps/desktop
& $tauri build --config $conf -- --locked
Pop-Location
$setupN = Join-Path $work "N-setup.exe"
# By exact name: the folder keeps installers of earlier builds.
Copy-Item "target/release/bundle/nsis/Trurl_${vN}_x64-setup.exe" $setupN

Step "Install $vN and add a model"
Start-Process $setupN -ArgumentList "/S" -Wait
$app = Join-Path $env:LOCALAPPDATA "Trurl\trurl.exe"
if (-not (Test-Path $app)) { throw "version $vN did not install" }
$models = Join-Path $env:LOCALAPPDATA "CyberiadLabs\Trurl\models"
New-Item -ItemType Directory -Force $models | Out-Null
Copy-Item (Join-Path $env:TRURL_TEST_MODELS "Qwen_Qwen3-0.6B-Q4_K_M.gguf") $models

Step "Build version $vNext, sign it, publish latest.json on localhost"
python scripts/bump_version.py $vNext
"## $vNext`n`nTest release made by scripts/windows_update_test.ps1." | Add-Content CHANGELOG.md
Push-Location apps/desktop
& $tauri build --config $conf -- --locked
Pop-Location
$setupNext = Join-Path $srv "Trurl_${vNext}_x64-setup.exe"
Copy-Item "target/release/bundle/nsis/Trurl_${vNext}_x64-setup.exe" $setupNext
& $tauri signer sign -f $key -p test --app-version $vNext $setupNext
python scripts/release_notes.py $vNext | Set-Content -Encoding utf8 (Join-Path $work "notes.md")
python scripts/make_manifest.py --version $vNext --notes-file (Join-Path $work "notes.md") --windows $setupNext --base-url "http://127.0.0.1:8899" --out (Join-Path $srv "latest.json")
$server = Start-Process python -ArgumentList "-m", "http.server", "8899", "--bind", "127.0.0.1", "--directory", $srv -PassThru

try {
  Step "Start $vN, accept the update in the real window"
  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=9222"
  $proc = Start-Process $app -PassThru
  Push-Location apps/bridge/e2e
  npm ci --no-audit --no-fund
  node run.mjs --cdp http://127.0.0.1:9222 --suite update --expect-version $vNext --out $out
  if ($LASTEXITCODE -ne 0) { Pop-Location; throw "the update was not offered or not started" }
  Pop-Location

  Step "Wait for the installer to put $vNext in place"
  $ok = $false
  for ($i = 0; $i -lt 300; $i++) {
    Start-Sleep -Seconds 1
    $v = (Get-Item $app -ErrorAction SilentlyContinue).VersionInfo.ProductVersion
    if ($v -and $v.StartsWith($vNext)) { $ok = $true; break }
  }
  if (-not $ok) {
    Write-Host "--- diagnostics"
    Get-Process | Where-Object { $_.Name -match "trurl|setup|llama|msedgewebview|Trurl" } |
      Select-Object Name, Id, Path, StartTime | Format-Table -AutoSize | Out-String -Width 300 | Write-Host
    Get-ChildItem $env:TEMP -Recurse -Include *.exe, *.log -ErrorAction SilentlyContinue |
      Where-Object { $_.LastWriteTime -gt (Get-Date).AddMinutes(-15) } |
      Select-Object FullName, Length, LastWriteTime | Format-Table -AutoSize | Out-String -Width 300 | Write-Host
    Get-ChildItem "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall" -ErrorAction SilentlyContinue |
      ForEach-Object { Get-ItemProperty $_.PSPath } | Where-Object { $_.DisplayName -match "Trurl" } |
      Select-Object DisplayName, DisplayVersion, InstallLocation | Format-List | Out-String | Write-Host
    Get-ChildItem (Split-Path $app) | Select-Object Name, Length, LastWriteTime | Format-Table -AutoSize | Out-String | Write-Host
    Get-WinEvent -LogName Application -MaxEvents 20 -ErrorAction SilentlyContinue |
      Where-Object { $_.TimeCreated -gt (Get-Date).AddMinutes(-15) } |
      Select-Object TimeCreated, ProviderName, Message | Format-List | Out-String -Width 300 | Write-Host
    throw "trurl.exe is still version $v after the update"
  }
  Write-Host "installed: $v"

  Step "Restart $vNext and check it works, with the model kept"
  Start-Sleep -Seconds 5
  Get-Process trurl -ErrorAction SilentlyContinue | Stop-Process -Force
  Start-Sleep -Seconds 3
  if (-not (Test-Path (Join-Path $models "Qwen_Qwen3-0.6B-Q4_K_M.gguf"))) { throw "the update deleted the model" }
  $proc = Start-Process $app -PassThru
  Push-Location apps/bridge/e2e
  node run.mjs --cdp http://127.0.0.1:9222 --suite after-update --expect-version $vNext --out $out
  if ($LASTEXITCODE -ne 0) { Pop-Location; throw "version $vNext does not work after the update" }
  Pop-Location
  Stop-Process -Id $proc.Id -ErrorAction SilentlyContinue

  Step "A class with no internet: install $vNext2 over $vNext from a stick folder"
  python scripts/bump_version.py $vNext2
  "## $vNext2`n`nTest release made by scripts/windows_update_test.ps1." | Add-Content CHANGELOG.md
  Push-Location apps/desktop
  & $tauri build --config $conf -- --locked
  Pop-Location
  $stick = Join-Path $work "stick"
  New-Item -ItemType Directory -Force (Join-Path $stick "models") | Out-Null
  $setup2 = Join-Path $stick "Trurl_${vNext2}_x64-setup.exe"
  Copy-Item "target/release/bundle/nsis/Trurl_${vNext2}_x64-setup.exe" $setup2
  # The model the laptop already has (same size: skipped), and one it does not.
  Copy-Item (Join-Path $env:TRURL_TEST_MODELS "Qwen_Qwen3-0.6B-Q4_K_M.gguf") (Join-Path $stick "models")
  $extra = Join-Path $stick "models\class-extra.gguf"
  [IO.File]::WriteAllBytes($extra, [byte[]](71, 71, 85, 70) + (New-Object byte[] 4092))
  $before = (Get-Item (Join-Path $models "Qwen_Qwen3-0.6B-Q4_K_M.gguf")).LastWriteTimeUtc
  Get-Process trurl -ErrorAction SilentlyContinue | Stop-Process -Force
  Start-Sleep -Seconds 3
  Start-Process $setup2 -ArgumentList "/S" -Wait
  $v = (Get-Item $app).VersionInfo.ProductVersion
  if (-not ($v -and $v.StartsWith($vNext2))) { throw "installing $vNext2 over $vNext left version $v" }
  $kept = Get-Item (Join-Path $models "Qwen_Qwen3-0.6B-Q4_K_M.gguf") -ErrorAction SilentlyContinue
  if (-not $kept) { throw "installing over the old version deleted the model" }
  if ($kept.LastWriteTimeUtc -ne $before) { throw "the model already there was copied again" }
  if (-not (Test-Path (Join-Path $models "class-extra.gguf"))) { throw "the new model on the stick was not copied" }
  if (Get-ChildItem $models -Filter *.part) { throw "a .part file was left in the models folder" }
  $proc = Start-Process $app -PassThru
  Push-Location apps/bridge/e2e
  node run.mjs --cdp http://127.0.0.1:9222 --suite after-update --expect-version $vNext2 --out $out
  if ($LASTEXITCODE -ne 0) { Pop-Location; throw "version $vNext2 does not work after installing it over $vNext" }
  Pop-Location
  Stop-Process -Id $proc.Id -ErrorAction SilentlyContinue
} finally {
  Stop-Process -Id $server.Id -ErrorAction SilentlyContinue
  git checkout -- pins.json Cargo.toml Cargo.lock apps/desktop/src-tauri/tauri.conf.json CHANGELOG.md
}
Write-Host "`nUPDATE TEST PASSED: $vN -> $vNext by the updater, -> $vNext2 by hand from a stick, models kept"
