$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

# Set assets path - check if assets folder exists in current directory
if (Test-Path .\assets) {
    $assetRoot = (Resolve-Path .\assets).Path
    $env:VELOREN_ASSETS = $assetRoot
    Write-Host "Assets folder found at: $assetRoot"
} elseif (Test-Path ..\assets) {
    $assetRoot = (Resolve-Path ..\assets).Path
    $env:VELOREN_ASSETS = $assetRoot
    Write-Host "Assets folder found at: $assetRoot"
} else {
    Write-Host "WARNING: Assets folder not found! The server may not work correctly."
    Write-Host "Expected assets folder in: $PSScriptRoot\assets or $PSScriptRoot\..\assets"
}

Get-Content .\.env | ForEach-Object {
    if ($_ -match '^\s*([^#][^=]*)=(.*)$') {
        [Environment]::SetEnvironmentVariable($matches[1].Trim(), $matches[2].Trim(), "Process")
    }
}
$env:VELOREN_ALPHA_ACCESS_PATH = Join-Path $PSScriptRoot "alpha-access.json"
$env:VELOREN_AUTH_MODE = "remote"
if (-not $env:VELOREN_AUTH_SERVER_URL) {
    $env:VELOREN_AUTH_SERVER_URL = "http://127.0.0.1:19253"
}
& .\bin\veloren-server-cli.exe --non-interactive
