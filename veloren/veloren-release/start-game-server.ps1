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

# Load environment variables from .env.local for local testing, or .env for VPS deployment
if (Test-Path .\.env.local) {
    Get-Content .\.env.local | ForEach-Object {
        if ($_ -match '^\s*([^#][^=]*)=(.*)$') {
            [Environment]::SetEnvironmentVariable($matches[1].Trim(), $matches[2].Trim(), "Process")
        }
    }
    Write-Host "Loaded configuration from .env.local (local testing mode)"
} elseif (Test-Path .\.env) {
    Get-Content .\.env | ForEach-Object {
        if ($_ -match '^\s*([^#][^=]*)=(.*)$') {
            [Environment]::SetEnvironmentVariable($matches[1].Trim(), $matches[2].Trim(), "Process")
        }
    }
    Write-Host "Loaded configuration from .env (VPS deployment mode)"
} else {
    Write-Host "WARNING: No configuration file found (.env or .env.local)"
    # Set defaults
    $env:VELOREN_AUTH_MODE = "local"
}

# Only override auth mode if not already set
if (-not $env:VELOREN_AUTH_MODE) {
    $env:VELOREN_AUTH_MODE = "remote"
}

# Set default auth server URL if not configured
if ($env:VELOREN_AUTH_MODE -eq "remote" -and -not $env:VELOREN_AUTH_SERVER_URL) {
    $env:VELOREN_AUTH_SERVER_URL = "http://127.0.0.1:19253"
    Write-Host "Set default auth server URL: http://127.0.0.1:19253"
}

Write-Host ""
Write-Host "========================================"
Write-Host "Starting Veloren Game Server"
Write-Host "========================================"
Write-Host "Auth Mode: $($env:VELOREN_AUTH_MODE)"
if ($env:VELOREN_AUTH_MODE -eq "remote") {
    Write-Host "Auth Server: $($env:VELOREN_AUTH_SERVER_URL)"
}
Write-Host "========================================"
Write-Host ""

& .\bin\veloren-server-cli.exe --non-interactive
