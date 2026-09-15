$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot
Get-Content .\marketplace.env | ForEach-Object {
    if ($_ -match '^\s*([^#][^=]*)=(.*)$') {
        [Environment]::SetEnvironmentVariable($matches[1].Trim(), $matches[2].Trim(), "Process")
    }
}
& .\bin\veloren-marketplace-service.exe
