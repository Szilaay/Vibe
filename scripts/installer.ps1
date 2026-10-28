[CmdletBinding()]
param([string]$Compiler, [string]$Notes = 'Installer and signed update support. Existing conversations and account settings are preserved.')
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$cargoPath = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
if (-not $Compiler) { $Compiler = Join-Path $projectRoot '.tools\nsis\nsis-3.13\makensis.exe' }
if (-not (Test-Path -LiteralPath $Compiler -PathType Leaf)) { throw 'Install the official NSIS compiler, or pass -Compiler with its path.' }
if (-not (Test-Path -LiteralPath (Join-Path $projectRoot 'release/update-public-key.txt') -PathType Leaf)) { throw 'Initialize the signing key with cargo run --bin vibe-release -- init-key first.' }
Push-Location -LiteralPath $projectRoot
try {
    foreach ($asset in @('whisper-cli.exe','whisper.dll','ggml.dll','ggml-base.dll','ggml-cpu.dll','ggml-small.bin','LICENSE-model.txt','LICENSE-whisper.txt')) {
        if (-not (Test-Path -LiteralPath (Join-Path $projectRoot "speech/$asset") -PathType Leaf)) { throw "Missing speech asset: $asset" }
    }
    $modelHash = (Get-FileHash -LiteralPath (Join-Path $projectRoot 'speech/ggml-small.bin') -Algorithm SHA256).Hash
    if ($modelHash -ne '1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b') { throw 'Speech model checksum mismatch.' }
    $versionMatch = [regex]::Match((Get-Content Cargo.toml -Raw), '(?m)^version = "(\d+\.\d+\.\d+)"')
    if (-not $versionMatch.Success) { throw 'Cargo.toml must contain a stable x.y.z version.' }
    $version = $versionMatch.Groups[1].Value
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $visualStudio = & $vswhere -latest -products '*' -property installationPath
    $redist = Get-ChildItem -LiteralPath (Join-Path $visualStudio 'VC/Redist/MSVC') -Directory |
        Where-Object { $_.Name -match '^\d+\.\d+\.\d+$' } | Sort-Object { [version]$_.Name } -Descending | Select-Object -First 1
    if (-not $redist) { throw 'Visual Studio redistributable libraries are required for the installer.' }
    $runtime = Join-Path $projectRoot '.tools/runtime'
    New-Item -ItemType Directory -Path $runtime -Force | Out-Null
    Get-ChildItem -LiteralPath (Join-Path $redist.FullName 'x64') -Directory |
        Where-Object { $_.Name -match '\.(CRT|OpenMP)$' } | ForEach-Object {
            Get-ChildItem -LiteralPath $_.FullName -Filter '*.dll' -File | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $runtime -Force }
        }
    foreach ($required in @('msvcp140.dll','vcruntime140.dll','vcruntime140_1.dll','vcomp140.dll')) {
        if (-not (Test-Path -LiteralPath (Join-Path $runtime $required))) { throw "Missing redistributable: $required" }
    }
    @'
Microsoft Visual C++ Runtime - app-local deployment.
These runtime libraries are redistributed from the Visual Studio redistributable directory.
Microsoft copyright and license terms apply.
https://learn.microsoft.com/en-us/cpp/windows/redistributing-visual-cpp-files
https://visualstudio.microsoft.com/license-terms/
'@ | Set-Content -LiteralPath (Join-Path $runtime 'NOTICE.txt') -Encoding utf8
    & $cargoPath build --release --bin vibe --bin vibe-release
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
    New-Item -ItemType Directory -Path (Join-Path $projectRoot 'dist') -Force | Out-Null
    & $Compiler /V2 "/DPROJECT_ROOT=$projectRoot" "/DVERSION=$version" (Join-Path $projectRoot 'release/installer.nsi')
    if ($LASTEXITCODE -ne 0) { throw 'Installer compilation failed.' }
    $installer = Join-Path $projectRoot "dist/VIBE-Setup-$version.exe"
    & (Join-Path $projectRoot 'target/release/vibe-release.exe') sign $installer $version $Notes
    if ($LASTEXITCODE -ne 0) { throw 'Release signing failed.' }
    Write-Output "Installer ready: $installer"
    Write-Output 'Signed update metadata: dist/latest.json'
} finally { Pop-Location }
