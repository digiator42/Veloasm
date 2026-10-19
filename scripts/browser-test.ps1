#!/usr/bin/env pwsh
#
# Run the browser client end to end: build the example, serve it, drive it
# with headless Chrome, and assert on the verdicts the page writes into its own
# DOM.
#
# The point of doing it this way is that the assertions run *inside* the page,
# against the live DOM, dispatching real events. The browser's verdict is then
# read back out of the rendered markup — so this is a real end-to-end test of
# the client, not a simulation of one.
#
#   .\scripts\browser-test.ps1
#
# Exits non-zero if any assertion failed, so it can gate CI.

param(
    # The port the static server binds to.
    [int]$Port = 8099,
    # Skip the build and test whatever is already in `dist/`.
    [switch]$NoBuild
)

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$example = Join-Path $root 'examples/ast-dashboard'
$dist = Join-Path $example 'dist'

function Find-Chrome {
    $candidates = @(
        "$env:ProgramFiles\Google\Chrome\Application\chrome.exe",
        "${env:ProgramFiles(x86)}\Google\Chrome\Application\chrome.exe",
        "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe",
        "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe"
    )
    foreach ($c in $candidates) {
        if ($c -and (Test-Path $c)) { return $c }
    }
    throw "no Chrome or Edge found; install one or pass -NoBuild with a prebuilt dist/"
}

if (-not $NoBuild) {
    Write-Host "==> emitting the view payload" -ForegroundColor Cyan
    Push-Location $example
    try {
        cargo run --quiet --example emit_view -- --emit-view
        Write-Host "==> building the wasm bundle" -ForegroundColor Cyan
        trunk build --release
    } finally {
        Pop-Location
    }
}

if (-not (Test-Path (Join-Path $dist 'view.json'))) {
    throw "dist/view.json is missing — run without -NoBuild"
}

$browser = Find-Chrome
Write-Host "==> serving $dist on port $Port" -ForegroundColor Cyan
$server = Start-Process -FilePath 'python' `
    -ArgumentList '-m', 'http.server', "$Port", '--bind', '127.0.0.1' `
    -WorkingDirectory $dist -WindowStyle Hidden -PassThru `
    -RedirectStandardError "$env:TEMP\velo-browser-server.log"

$dump = "$env:TEMP\velo-browser-dom.html"
try {
    # Give the server a moment to bind.
    Start-Sleep -Seconds 2

    Write-Host "==> driving headless $browser" -ForegroundColor Cyan
    & $browser --headless --disable-gpu --no-sandbox `
        --virtual-time-budget=20000 --dump-dom "http://127.0.0.1:$Port/index.html" 2>$null |
        Out-File -FilePath $dump -Encoding utf8
} finally {
    Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
}

$dom = Get-Content $dump -Raw
if ($dom -notmatch '(?s)<pre id="test-results">(.*?)</pre>') {
    Write-Host "FAILED: the page produced no test results" -ForegroundColor Red
    Write-Host $dom
    exit 1
}

$results = $matches[1].Trim() -split "`n" | ForEach-Object { $_.Trim() } | Where-Object { $_ }

$failed = @($results | Where-Object { $_ -like 'FAIL*' })
$passed = @($results | Where-Object { $_ -like 'PASS*' })

foreach ($r in $results) {
    if ($r -like 'FAIL*') { Write-Host "  $r" -ForegroundColor Red }
    else { Write-Host "  $r" -ForegroundColor DarkGray }
}

Write-Host ""
if ($failed.Count -gt 0) {
    Write-Host "FAILED: $($failed.Count) of $($results.Count) browser assertions failed" -ForegroundColor Red
    exit 1
}

Write-Host "OK: $($passed.Count) browser assertions passed" -ForegroundColor Green
exit 0
