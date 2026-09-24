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
    Write-Host "WARNING: Assets folder not found! The marketplace service may not work correctly."
    Write-Host "Expected assets folder in: $PSScriptRoot\assets or $PSScriptRoot\..\assets"
}

Get-Content .\marketplace.env | ForEach-Object {
    if ($_ -match '^\s*([^#][^=]*)=(.*)$') {
        [Environment]::SetEnvironmentVariable($matches[1].Trim(), $matches[2].Trim(), "Process")
    }
}
& .\bin\veloren-marketplace-service.exe
