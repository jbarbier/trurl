# Evals on Windows: the real core, the real pinned llama-server and real models,
# with Windows Firewall blocking every outbound connection of the test program
# and of llama-server. Loopback (Trurl talking to its own llama-server) is not
# filtered by the firewall, which is exactly the offline situation in a classroom.
#
#   ./scripts/run_evals.ps1
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

$models = $env:TRURL_TEST_MODELS
if (-not $models) { $models = Join-Path $HOME ".cache\trurl\models" }
python scripts/fetch_models.py --dest $models --only gemma-3-1b-it,qwen3-0.6b
if ($LASTEXITCODE -ne 0) { throw "fetch_models failed" }
python scripts/fetch_models.py --dest $models --test-models
if ($LASTEXITCODE -ne 0) { throw "fetch_models --test-models failed" }

$bin = cargo test -p trurl-core --test live --no-run --message-format=json 2>$null |
  ForEach-Object { try { $_ | ConvertFrom-Json } catch { $null } } |
  Where-Object { $_ -and $_.reason -eq "compiler-artifact" -and $_.target.name -eq "live" -and $_.executable } |
  Select-Object -Last 1 -ExpandProperty executable
if (-not $bin) { throw "could not build the live tests" }

$programs = @($bin) + (Get-ChildItem vendor/llama/windows -Recurse -Filter llama-server.exe | ForEach-Object { $_.FullName })
Set-NetFirewallProfile -Profile Domain,Public,Private -Enabled True
$rules = @()
foreach ($p in $programs) {
  $name = "trurl-evals-block-" + [guid]::NewGuid().ToString("N").Substring(0, 8)
  New-NetFirewallRule -DisplayName $name -Direction Outbound -Action Block -Program $p | Out-Null
  $rules += $name
}
try {
  $env:TRURL_TEST_MODELS = $models
  # Proves the block works: the offline test asserts Hugging Face is unreachable.
  $env:TRURL_EXPECT_OFFLINE = "1"
  & $bin --ignored --test-threads 1
  if ($LASTEXITCODE -ne 0) { throw "evals failed" }
} finally {
  foreach ($r in $rules) { Remove-NetFirewallRule -DisplayName $r }
}
