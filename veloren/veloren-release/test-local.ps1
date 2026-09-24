$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

Write-Host "========================================"
Write-Host "Veloren Local Test Script"
Write-Host "========================================"
Write-Host ""

# Check if assets folder exists
if (-not (Test-Path .\assets)) {
    Write-Host "ERROR: Assets folder not found!" -ForegroundColor Red
    Write-Host "Expected location: $PSScriptRoot\assets"
    exit 1
}

Write-Host "✓ Assets folder found" -ForegroundColor Green

# Set assets path
$assetRoot = (Resolve-Path .\assets).Path
$env:VELOREN_ASSETS = $assetRoot
Write-Host "✓ Assets path set to: $assetRoot" -ForegroundColor Green

# Load environment variables
if (Test-Path .\.env.local) {
    Get-Content .\.env.local | ForEach-Object {
        if ($_ -match '^\s*([^#][^=]*)=(.*)$') {
            [Environment]::SetEnvironmentVariable($matches[1].Trim(), $matches[2].Trim(), "Process")
        }
    }
    Write-Host "✓ Environment variables loaded from .env.local" -ForegroundColor Green
} elseif (Test-Path .\.env) {
    Get-Content .\.env | ForEach-Object {
        if ($_ -match '^\s*([^#][^=]*)=(.*)$') {
            [Environment]::SetEnvironmentVariable($matches[1].Trim(), $matches[2].Trim(), "Process")
        }
    }
    Write-Host "✓ Environment variables loaded from .env" -ForegroundColor Green
} else {
    Write-Host "WARNING: No .env file found, using defaults" -ForegroundColor Yellow
    # Set defaults for local testing
    $env:VELOREN_AUTH_MODE = "local"
    $env:VELOREN_SERVER_NAME = "Runehaven Local Test Server"
    Write-Host "✓ Set to local authentication mode (no auth server required)" -ForegroundColor Green
}

Write-Host ""
Write-Host "========================================"
Write-Host "Starting game server in local mode..."
Write-Host "========================================"
Write-Host ""
Write-Host "Server will be available at: localhost:14004"
Write-Host "Press Ctrl+C to stop the server"
Write-Host ""

& .\bin\veloren-server-cli.exe