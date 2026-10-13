# Windows smoke test, run by CI after `tauri build`. It does what a teacher does
# with a USB stick, then checks every promise:
#   1. the installer runs without admin rights and copies the models next to it;
#   2. Trurl starts with the network blocked by the firewall, loads the default
#      model and answers a message (driven through the real WebView2 window);
#   3. closing Trurl stops llama-server;
#   4. uninstalling keeps the models.
# Screenshots of the real window go to target/e2e/windows/.
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)
$root = Get-Location
$out = Join-Path $root "target\e2e\windows"
New-Item -ItemType Directory -Force $out | Out-Null

function Step($msg) { Write-Host "`n=== $msg" }
function Shot($name) {
  Add-Type -AssemblyName System.Windows.Forms, System.Drawing
  $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
  $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
  $bmp.Save((Join-Path $out "$name.png"))
  $g.Dispose(); $bmp.Dispose()
}

Step "USB-style folder: installer + models/"
# By exact name: target/ is cached between CI runs and may hold older installers.
$version = (Get-Content pins.json | ConvertFrom-Json).app.version
$setup = Get-Item "target/release/bundle/nsis/Trurl_${version}_x64-setup.exe"
$usb = Join-Path $env:RUNNER_TEMP "TrurlUSB"
New-Item -ItemType Directory -Force (Join-Path $usb "models") | Out-Null
Copy-Item $setup.FullName $usb
$models = $env:TRURL_TEST_MODELS
Copy-Item (Join-Path $models "Qwen_Qwen3-0.6B-Q4_K_M.gguf") (Join-Path $usb "models")
Copy-Item (Join-Path $models "gemma-3-1b-it-Q4_K_M.gguf") (Join-Path $usb "models")

Step "Silent install (current user, no admin)"
$p = Start-Process (Join-Path $usb $setup.Name) -ArgumentList "/S" -Wait -PassThru
if ($p.ExitCode -ne 0) { throw "installer exit code $($p.ExitCode)" }
$app = Join-Path $env:LOCALAPPDATA "Trurl\trurl.exe"
if (-not (Test-Path $app)) { throw "trurl.exe not installed at $app" }
$data = Join-Path $env:LOCALAPPDATA "CyberiadLabs\Trurl\models"
foreach ($m in "Qwen_Qwen3-0.6B-Q4_K_M.gguf", "gemma-3-1b-it-Q4_K_M.gguf") {
  $f = Join-Path $data $m
  if (-not (Test-Path $f)) { throw "installer did not copy $m" }
  if ((Get-Item $f).Length -ne (Get-Item (Join-Path $usb "models\$m")).Length) { throw "$m copied incompletely" }
}
if (Get-ChildItem $data -Filter *.part) { throw "a .part file was left behind" }
foreach ($v in "avx2", "avx", "ssse3", "compat") {
  if (-not (Test-Path (Join-Path $env:LOCALAPPDATA "Trurl\llama\$v\llama-server.exe"))) { throw "llama-server $v missing from install" }
}
$installed = (Get-ChildItem (Join-Path $env:LOCALAPPDATA "Trurl") -Recurse -File | Measure-Object -Sum Length).Sum
Write-Host ("installed, models copied. Trurl on disk: {0:N1} MB" -f ($installed / 1MB))

Step "Block all outbound traffic of trurl.exe and llama-server.exe"
Set-NetFirewallProfile -Profile Domain,Public,Private -Enabled True
$rules = @()
$progs = @($app) + (Get-ChildItem (Join-Path $env:LOCALAPPDATA "Trurl\llama") -Recurse -Filter llama-server.exe | ForEach-Object { $_.FullName })
foreach ($prog in $progs) {
  $n = "trurl-smoke-" + [guid]::NewGuid().ToString("N").Substring(0, 8)
  New-NetFirewallRule -DisplayName $n -Direction Outbound -Action Block -Program $prog | Out-Null
  $rules += $n
}

try {
  Step "Start Trurl (WebView2 debugging port for the test driver)"
  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=9222"
  $proc = Start-Process $app -PassThru
  Start-Sleep -Seconds 8
  Shot "01-started"

  Step "Drive the real window: wait for Ready, chat, check the answer"
  Push-Location apps/bridge/e2e
  npm ci --no-audit --no-fund
  node run.mjs --cdp http://127.0.0.1:9222 --suite smoke --out $out
  if ($LASTEXITCODE -ne 0) { Pop-Location; throw "UI smoke test failed" }
  Pop-Location
  Shot "02-after-chat"

  Step "Memory of the desktop app after one answer (for docs/BENCHMARKS.md)"
  $all = Get-CimInstance Win32_Process
  $tree = @($proc.Id)
  do {
    $more = $all | Where-Object { $tree -contains $_.ParentProcessId -and $tree -notcontains $_.ProcessId } | ForEach-Object { $_.ProcessId }
    $tree += $more
  } while ($more)
  $rows = Get-Process -Id $tree -ErrorAction SilentlyContinue | Group-Object Name | ForEach-Object {
    [pscustomobject]@{
      Process = $_.Name
      Count = $_.Count
      WorkingSetMB = [math]::Round(($_.Group | Measure-Object WorkingSet64 -Sum).Sum / 1MB)
      PrivateMB = [math]::Round(($_.Group | Measure-Object PrivateMemorySize64 -Sum).Sum / 1MB)
    }
  }
  $rows | Format-Table -AutoSize | Out-String | Write-Host
  $total = ($rows | Measure-Object WorkingSetMB -Sum).Sum
  Write-Host "DESKTOP MEMORY: $total MB working set in all (trurl.exe, WebView2, llama-server with the model mapped)"

  Step "Close Trurl: llama-server must stop with it"
  # Only the installed copies: an earlier CI step may have left its own llama-server behind.
  $ours = Join-Path $env:LOCALAPPDATA "Trurl\llama\"
  function OurServers {
    @(Get-CimInstance Win32_Process -Filter "Name='llama-server.exe'" |
      Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($ours, [StringComparison]::OrdinalIgnoreCase) })
  }
  if ((OurServers).Count -eq 0) { throw "no installed llama-server was running before Trurl closed" }
  Stop-Process -Id $proc.Id
  $t0 = Get-Date
  while ((OurServers).Count -gt 0 -and ((Get-Date) - $t0).TotalSeconds -lt 15) { Start-Sleep -Milliseconds 250 }
  Get-CimInstance Win32_Process -Filter "Name='llama-server.exe'" |
    Format-Table ProcessId, ParentProcessId, CreationDate, ExecutablePath -AutoSize | Out-String | Write-Host
  if ((OurServers).Count -gt 0) { throw "llama-server still running 15 s after Trurl closed" }
  Write-Host ("llama-server stopped {0:N1} s after Trurl closed" -f ((Get-Date) - $t0).TotalSeconds)
} finally {
  foreach ($r in $rules) { Remove-NetFirewallRule -DisplayName $r }
}

Step "Silent uninstall keeps the models"
$un = Join-Path $env:LOCALAPPDATA "Trurl\uninstall.exe"
Start-Process $un -ArgumentList "/S" -Wait | Out-Null
# The NSIS uninstaller re-launches itself from %TEMP%: wait for the files to go.
for ($i = 0; $i -lt 60 -and (Test-Path $app); $i++) { Start-Sleep -Seconds 1 }
if (Test-Path $app) { throw "trurl.exe still there after uninstall" }
if (-not (Test-Path (Join-Path $data "Qwen_Qwen3-0.6B-Q4_K_M.gguf"))) { throw "uninstall deleted the models" }
Write-Host "`nSMOKE TEST PASSED"
