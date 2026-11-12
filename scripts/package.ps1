# Builds dist\tidemark-<version>-win-x64.zip and, with Inno Setup,
# dist\tidemark-<version>-setup.exe.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches.Groups[1].Value
cargo build --release
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

$stage = [IO.Path]::GetFullPath((Join-Path $root 'dist\tidemark'))
$distRoot = [IO.Path]::GetFullPath((Join-Path $root 'dist')) + [IO.Path]::DirectorySeparatorChar
if (-not $stage.StartsWith($distRoot, [StringComparison]::OrdinalIgnoreCase)) { throw 'Stage must be inside dist' }
if (Test-Path $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
New-Item -ItemType Directory -Force "$stage\integrations\claude-code", "$stage\integrations\opencode" | Out-Null

Copy-Item target\release\win-island.exe, target\release\island-hook.exe $stage
Copy-Item integrations\opencode\win-island.js "$stage\integrations\opencode"
New-Item -ItemType Directory -Force "$stage\integrations\bb" | Out-Null
Copy-Item integrations\bb\* "$stage\integrations\bb"
Copy-Item README.md $stage
Copy-Item CHANGELOG.md $stage
New-Item -ItemType Directory -Force "$stage\docs\design" | Out-Null
Copy-Item docs\design\tidemark.md "$stage\docs\design"
New-Item -ItemType Directory -Force "$stage\wallpapers" | Out-Null
Copy-Item assets\wallpapers\coast.png, assets\wallpapers\night.png "$stage\wallpapers"

# Claude Code hooks: the development path becomes a placeholder the installer
# (or the user, for the portable zip) replaces with the real island-hook.exe.
$hooks = Get-Content integrations\claude-code\hooks.json -Raw
$hooks = $hooks -replace '"command":\s*"[^"]*island-hook\.exe"', '"command": "@HOOK@"'
Set-Content "$stage\integrations\claude-code\hooks.template.json" $hooks -Encoding utf8

$zip = Join-Path $root "dist\tidemark-$version-win-x64.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path "$stage\*" -DestinationPath $zip
Write-Host "Portable: $zip"

$iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source
if (-not $iscc) { $iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1 }
if ($iscc) {
    & $iscc "/DAppVersion=$version" "/DStageDir=$stage" "/O$root\dist" installer\win-island.iss
    if ($LASTEXITCODE -ne 0) { throw 'iscc failed' }
    Write-Host "Installer: dist\tidemark-$version-setup.exe"
} else {
    Write-Warning 'Inno Setup not found - installer skipped (winget install JRSoftware.InnoSetup).'
}
