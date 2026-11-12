# Connect the current Windows account's Tidemark to a local bb installation.
param([string]$ServerUrl = $env:BB_SERVER_URL)
$ErrorActionPreference = 'Stop'
if (-not $ServerUrl) { $ServerUrl = 'http://127.0.0.1:38886' }
$uri = [Uri]$ServerUrl
if ($uri.Scheme -ne 'https' -and -not ($uri.Scheme -eq 'http' -and $uri.IsLoopback)) {
    throw 'Use HTTPS for remote bb servers.'
}
if ($uri.UserInfo -or $uri.Query -or $uri.Fragment) { throw 'Use the bb server address without credentials, query or fragment.' }
if (-not (Get-Command bb -ErrorAction SilentlyContinue)) { throw 'bb CLI was not found. Install bb or enter URL/token in the Tidemark bb panel.' }
$pluginToken = (& bb plugin token tidemark | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or -not $pluginToken) {
    throw 'Install the connector first: bb plugin install git:https://github.com/treyzar/bb-plugin-tidemark.git'
}
Add-Type -AssemblyName System.Security
$protectedToken = [Security.Cryptography.ProtectedData]::Protect(
    [Text.Encoding]::UTF8.GetBytes($pluginToken), $null,
    [Security.Cryptography.DataProtectionScope]::CurrentUser)
$directory = Join-Path $env:APPDATA 'win-island'
New-Item -ItemType Directory -Path $directory -Force | Out-Null
$saved = @{ url = $ServerUrl.TrimEnd('/'); protected_token = @($protectedToken | ForEach-Object { [int]$_ }) }
$json = ConvertTo-Json $saved -Depth 3
[IO.File]::WriteAllText((Join-Path $directory 'bb-connection.json'), $json, [Text.UTF8Encoding]::new($false))
$pluginToken = $null
Write-Host 'Tidemark connected to bb. Open the island menu -> bb to answer or write a message.'
