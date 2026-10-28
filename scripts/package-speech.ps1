[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$speechRoot = Join-Path $projectRoot 'speech'
$cacheRoot = Join-Path $projectRoot '.tools\speech'
New-Item -ItemType Directory -Path $speechRoot, $cacheRoot -Force | Out-Null

function Get-VerifiedDownload {
    param([string]$Url, [string]$Destination, [string]$Sha256)
    if ((Test-Path -LiteralPath $Destination -PathType Leaf) -and
        (Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash -eq $Sha256) {
        return
    }
    $partial = "$Destination.partial"
    Write-Output "Downloading offline dictation asset: $Destination"
    # curl streams large models without retaining them in PowerShell memory.
    & curl.exe --fail --location --retry 3 --silent --show-error --output $partial $Url
    if ($LASTEXITCODE -ne 0) { throw "Download failed: $Url" }
    if ((Get-FileHash -LiteralPath $partial -Algorithm SHA256).Hash -ne $Sha256) {
        throw "Checksum mismatch: $partial"
    }
    Move-Item -LiteralPath $partial -Destination $Destination -Force
}

$archive = Join-Path $cacheRoot 'whisper-bin-x64-v1.8.3.zip'
Get-VerifiedDownload -Url 'https://github.com/ggml-org/whisper.cpp/releases/download/v1.8.3/whisper-bin-x64.zip' `
    -Destination $archive -Sha256 'd824b1e37599f882b396e73f1ee0bfd5d0529f700314c48311dcbd00b803321d'
$expanded = Join-Path $cacheRoot 'v1.8.3'
Expand-Archive -LiteralPath $archive -DestinationPath $expanded -Force
$release = Join-Path $expanded 'Release'
Copy-Item -LiteralPath (Join-Path $release 'whisper-cli.exe') -Destination $speechRoot -Force
Get-ChildItem -LiteralPath $release -Filter '*.dll' | ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination $speechRoot -Force
}
Get-VerifiedDownload `
    -Url 'https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-small.bin' `
    -Destination (Join-Path $speechRoot 'ggml-small.bin') `
    -Sha256 '1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b'

# Preserve the notices for the engine, model, and DLL dependencies we redistribute.
& curl.exe --fail --location --retry 3 --silent --show-error `
    --output (Join-Path $speechRoot 'LICENSE-whisper.txt') `
    'https://raw.githubusercontent.com/ggml-org/whisper.cpp/v1.8.3/LICENSE'
if ($LASTEXITCODE -ne 0) { throw 'Cannot download whisper.cpp license' }
& curl.exe --fail --location --retry 3 --silent --show-error `
    --output (Join-Path $speechRoot 'LICENSE-model.txt') `
    'https://raw.githubusercontent.com/openai/whisper/v20250625/LICENSE'
if ($LASTEXITCODE -ne 0) { throw 'Cannot download Whisper model license' }
# SDL is only used by other upstream examples, not whisper-cli. Do not ship it.
$sdlPath = Join-Path $speechRoot 'SDL2.dll'
if (Test-Path -LiteralPath $sdlPath) { Remove-Item -LiteralPath $sdlPath }
Write-Output "Offline Hungarian dictation ready: $speechRoot"
