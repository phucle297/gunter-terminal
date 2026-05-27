#Requires -Version 5.1
# install.ps1 — Install gunter terminal emulator on Windows
# Usage: irm https://raw.githubusercontent.com/phucle297/gunter-terminal/develop/install.ps1 | iex
# Set $env:GUNTER_VERSION to pin a specific tag, e.g.: $env:GUNTER_VERSION = "v0.2.0"

$ErrorActionPreference = 'Stop'

$Repo       = 'phucle297/gunter-terminal'
$InstallDir = "$env:LOCALAPPDATA\Programs\gunter"
$Version    = if ($env:GUNTER_VERSION) { $env:GUNTER_VERSION } else { 'latest' }

# Resolve latest tag via GitHub API
if ($Version -eq 'latest') {
    try {
        $release = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest"
        $Version = $release.tag_name
    } catch {
        Write-Error "Failed to resolve latest release: $_"
        exit 1
    }
}

$asset  = "gunter-windows-x86_64.zip"
$url    = "https://github.com/$Repo/releases/download/$Version/$asset"
$sha256Url = "$url.sha256"
$tmp    = Join-Path $env:TEMP "gunter-$Version.zip"

Write-Host "Downloading gunter $Version..."
try {
    Invoke-WebRequest -Uri $url -OutFile $tmp -UseBasicParsing
} catch {
    Write-Error "Download failed: $_"
    exit 1
}

# Verify SHA256 — .Content is byte[] in PS5, decode explicitly
try {
    $resp     = Invoke-WebRequest -Uri $sha256Url -UseBasicParsing
    $text     = [System.Text.Encoding]::UTF8.GetString($resp.Content).Trim()
    $expected = $text.Split(' ')[0].ToLower()
    $actual   = (Get-FileHash $tmp -Algorithm SHA256).Hash.ToLower()
    if ($expected -ne $actual) {
        Remove-Item $tmp -Force
        Write-Error "SHA256 mismatch. Expected: $expected  Got: $actual"
        exit 1
    }
    Write-Host "SHA256 verified."
} catch {
    Write-Warning "Could not verify SHA256 (non-fatal): $_"
}

# Extract to temp staging dir, then copy into install dir
$stage = Join-Path $env:TEMP "gunter-stage-$Version"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
Expand-Archive -Path $tmp -DestinationPath $stage -Force
Remove-Item $tmp -Force

# Unwrap single nested subdirectory if present (zip contains gunter-windows-x86_64\*)
$children = @(Get-ChildItem $stage)
$src = if ($children.Count -eq 1 -and $children[0].PSIsContainer) { $children[0].FullName } else { $stage }

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
# Copy-Item -Recurse -Force handles existing dirs correctly
Copy-Item -Path "$src\*" -Destination $InstallDir -Recurse -Force
Remove-Item $stage -Recurse -Force

# Add to user PATH if missing
$userPath = [Environment]::GetEnvironmentVariable('PATH', 'User')
if ($userPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable('PATH', "$userPath;$InstallDir", 'User')
    Write-Host "Added $InstallDir to PATH. Restart your terminal for PATH to take effect."
}

Write-Host ""
Write-Host "gunter $Version installed to $InstallDir\gunter.exe"
Write-Host "Run: gunter"
