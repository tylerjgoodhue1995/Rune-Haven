$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot
Get-Content .\.env | ForEach-Object {
    if ($_ -match '^\s*([^#][^=]*)=(.*)$') {
        [Environment]::SetEnvironmentVariable($matches[1].Trim(), $matches[2].Trim(), "Process")
    }
}
$env:VELOREN_AUTH_MODE = "remote"
if (-not $env:VELOREN_AUTH_SERVER_URL) {
    $env:VELOREN_AUTH_SERVER_URL = "http://127.0.0.1:19253"
}
& .\bin\veloren-server-cli.exe --non-interactive
