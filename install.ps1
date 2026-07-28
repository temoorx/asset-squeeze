$ErrorActionPreference = "Stop"

$Repo = if ($env:ASSET_SQUEEZE_REPO) { $env:ASSET_SQUEEZE_REPO } else { "temoorx/asset-squeeze" }
$InstallDir = if ($env:ASSET_SQUEEZE_INSTALL_DIR) { $env:ASSET_SQUEEZE_INSTALL_DIR } else { Join-Path $HOME ".asset-squeeze" }
$BinDir = Join-Path $InstallDir "bin"

function Get-Platform {
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    switch ($arch) {
        "X64" { return "windows-x86_64" }
        "Arm64" {
            Write-Host "Windows ARM64 detected; installing the Windows x64 build for emulation."
            return "windows-x86_64"
        }
        default { throw "Unsupported CPU architecture: $arch" }
    }
}

$Platform = Get-Platform
$Archive = "asset-squeeze-$Platform.zip"
$Url = "https://github.com/$Repo/releases/latest/download/$Archive"
$ChecksumsUrl = "https://github.com/$Repo/releases/latest/download/SHA256SUMS"
$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("asset-squeeze-" + [System.Guid]::NewGuid().ToString())

New-Item -ItemType Directory -Force -Path $TempDir | Out-Null

try {
    Write-Host "Installing asset-squeeze for $Platform"
    Write-Host "Downloading $Url"

    $ArchivePath = Join-Path $TempDir $Archive
    $ChecksumsPath = Join-Path $TempDir "SHA256SUMS"
    Invoke-WebRequest -Uri $Url -OutFile $ArchivePath
    Invoke-WebRequest -Uri $ChecksumsUrl -OutFile $ChecksumsPath

    $ChecksumLine = Get-Content $ChecksumsPath | Where-Object { $_ -match "\s$([regex]::Escape($Archive))$" } | Select-Object -First 1
    if (-not $ChecksumLine) {
        throw "Could not find checksum for $Archive"
    }

    $ExpectedChecksum = ($ChecksumLine -split "\s+")[0].ToLowerInvariant()
    $ActualChecksum = (Get-FileHash -Algorithm SHA256 $ArchivePath).Hash.ToLowerInvariant()
    if ($ActualChecksum -ne $ExpectedChecksum) {
        throw "Checksum verification failed for $Archive"
    }

    Expand-Archive -Path $ArchivePath -DestinationPath $TempDir -Force

    New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
    Copy-Item (Join-Path $TempDir "asset-squeeze/asset-squeeze.exe") (Join-Path $BinDir "asset-squeeze.exe") -Force

    $VendorSource = Join-Path $TempDir "asset-squeeze/vendor"
    $VendorDest = Join-Path $InstallDir "vendor"
    if (Test-Path $VendorSource) {
        if (Test-Path $VendorDest) {
            Remove-Item $VendorDest -Recurse -Force
        }
        Copy-Item $VendorSource $VendorDest -Recurse -Force
    }

    $Notices = Join-Path $TempDir "asset-squeeze/THIRD_PARTY_NOTICES.md"
    if (Test-Path $Notices) {
        Copy-Item $Notices (Join-Path $InstallDir "THIRD_PARTY_NOTICES.md") -Force
    }

    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $PathParts = @()
    if ($UserPath) {
        $PathParts = $UserPath -split ";"
    }

    if ($PathParts -notcontains $BinDir) {
        $NewPath = if ($UserPath) { "$UserPath;$BinDir" } else { $BinDir }
        [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
        $env:Path = "$env:Path;$BinDir"
        Write-Host "Added $BinDir to your user PATH."
    }

    & (Join-Path $BinDir "asset-squeeze.exe") --version

    Write-Host ""
    Write-Host "asset-squeeze installed successfully."
    Write-Host "Try it in a Flutter project:"
    Write-Host "  asset-squeeze doctor"
    Write-Host "  asset-squeeze optimize --dry-run"
}
finally {
    if (Test-Path $TempDir) {
        Remove-Item $TempDir -Recurse -Force
    }
}
