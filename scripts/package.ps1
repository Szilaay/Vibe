[CmdletBinding()]
param([switch]$UseExistingSpeech)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$cargo = Get-Command cargo -ErrorAction SilentlyContinue
if ($cargo) {
    $cargoPath = $cargo.Source
} else {
    $cargoPath = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
    if (-not (Test-Path -LiteralPath $cargoPath -PathType Leaf)) {
        throw 'Cargo not found. Install Rust stable, then run scripts/package.ps1 again.'
    }
}

Push-Location -LiteralPath $projectRoot
try {
    if ($UseExistingSpeech) {
        foreach ($asset in @('whisper-cli.exe', 'whisper.dll', 'ggml.dll', 'ggml-base.dll', 'ggml-cpu.dll', 'ggml-small.bin', 'LICENSE-whisper.txt', 'LICENSE-model.txt')) {
            $assetPath = Join-Path $projectRoot "speech/$asset"
            if (-not (Test-Path -LiteralPath $assetPath -PathType Leaf) -or (Get-Item -LiteralPath $assetPath).Length -eq 0) {
                throw "Missing offline speech asset: $assetPath"
            }
        }
        $modelHash = (Get-FileHash -LiteralPath (Join-Path $projectRoot 'speech/ggml-small.bin') -Algorithm SHA256).Hash
        if ($modelHash -ne '1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b') { throw 'Offline speech model checksum mismatch.' }
    } else {
        & (Join-Path $PSScriptRoot 'package-speech.ps1')
    }
    & $cargoPath build --release --bin vibe
    if ($LASTEXITCODE -ne 0) {
        throw "VIBE build failed (exit $LASTEXITCODE). Existing VIBE.exe was preserved."
    }
    # Cargo metadata respects custom CARGO_TARGET_DIR and .cargo/config.toml settings.
    $metadataJson = & $cargoPath metadata --format-version 1 --no-deps
    if ($LASTEXITCODE -ne 0) {
        throw 'Cannot locate the Cargo output directory. Existing VIBE.exe was preserved.'
    }
    $metadata = $metadataJson | ConvertFrom-Json
    $builtExe = Join-Path $metadata.target_directory 'release\vibe.exe'
    if (-not (Test-Path -LiteralPath $builtExe -PathType Leaf)) {
        throw "Native executable not found: $builtExe"
    }
    $destination = Join-Path $projectRoot 'VIBE.exe'
    try {
        Copy-Item -LiteralPath $builtExe -Destination $destination -Force
    } catch [System.IO.IOException] {
        # Keep the running app and its sessions intact while publishing the update.
        $destination = Join-Path $projectRoot 'VIBE-friss.exe'
        Copy-Item -LiteralPath $builtExe -Destination $destination -Force
        Write-Output 'VIBE.exe is in use; the update was saved as VIBE-friss.exe.'
    }
    $distRoot = Join-Path $projectRoot 'dist'
    New-Item -ItemType Directory -Path $distRoot -Force | Out-Null
    $bundle = Join-Path $distRoot 'VIBE-windows-x64.zip'
    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $partialBundle = Join-Path $distRoot 'VIBE-windows-x64.partial.zip'
    $bundleStream = [IO.File]::Open($partialBundle, [IO.FileMode]::Create)
    $zip = New-Object IO.Compression.ZipArchive($bundleStream, [IO.Compression.ZipArchiveMode]::Create)
    try {
        [void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $destination, 'VIBE.exe')
        [void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, (Join-Path $projectRoot 'assets/providers/LICENSE-openrouter.txt'), 'NOTICE-OpenRouter.txt')
        $readme = $zip.CreateEntry('README.txt')
        $readmeWriter = [IO.StreamWriter]::new($readme.Open(), [Text.UTF8Encoding]::new($false))
        try {
            $readmeWriter.WriteLine('VIBE - Windows x64 portable package')
            $readmeWriter.WriteLine('Extract the entire ZIP into one folder, then launch VIBE.exe.')
            $readmeWriter.WriteLine('Keep the speech folder next to VIBE.exe. Do not launch from inside the ZIP.')
            $readmeWriter.WriteLine('To share VIBE with offline dictation, send this entire ZIP, not only the EXE.')
            $readmeWriter.WriteLine('The speech model is local. No Rust installation or packaging script is required to run the app.')
            $readmeWriter.WriteLine('Codex / Antigravity sign-in and runtime setup are handled separately by the app.')
        } finally { $readmeWriter.Dispose() }

        Get-ChildItem -LiteralPath (Join-Path $projectRoot 'speech') -File | ForEach-Object {
            [void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $_.FullName, "speech/$($_.Name)")
        }
    } finally {
        $zip.Dispose()
        $bundleStream.Dispose()
    }
    Move-Item -LiteralPath $partialBundle -Destination $bundle -Force
    Write-Output "Native desktop application ready: $destination"
    Write-Output "Portable package with offline dictation: $bundle"
} finally {
    Pop-Location
}
