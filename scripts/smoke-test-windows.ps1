param(
    [Parameter(Mandatory = $true)]
    [string]$Root,

    [switch]$Installed
)

$ErrorActionPreference = "Stop"

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,

        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$FilePath exited with code $LASTEXITCODE"
    }
}

$Root = (Resolve-Path $Root).Path
$Cli = if ($Installed) {
    Join-Path $Root "bin/asset-squeeze.exe"
} else {
    Join-Path $Root "asset-squeeze.exe"
}
$Vendor = Join-Path $Root "vendor/bin/windows-x86_64"

if (-not (Test-Path $Cli -PathType Leaf)) {
    throw "Missing CLI executable: $Cli"
}
Invoke-Checked -FilePath $Cli -Arguments @("--version")

$Backends = @("jpegtran", "cjpeg", "djpeg", "cwebp", "dwebp")
foreach ($Backend in $Backends) {
    $Tool = Join-Path $Vendor "$Backend.exe"
    if (-not (Test-Path $Tool -PathType Leaf)) {
        throw "Missing backend executable: $Tool"
    }
    Invoke-Checked -FilePath $Tool -Arguments @("-version")
}

$Fixture = Join-Path ([System.IO.Path]::GetTempPath()) ("asset-squeeze-smoke-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $Fixture | Out-Null

try {
    $Ppm = Join-Path $Fixture "source.ppm"
    $Header = [Text.Encoding]::ASCII.GetBytes("P6`n256 256`n255`n")
    $Pixels = New-Object byte[] (256 * 256 * 3)
    $Index = 0
    for ($Y = 0; $Y -lt 256; $Y++) {
        for ($X = 0; $X -lt 256; $X++) {
            $Pixels[$Index] = $X
            $Pixels[$Index + 1] = $Y
            $Pixels[$Index + 2] = ($X * 3 + $Y * 5) % 256
            $Index += 3
        }
    }
    $PpmBytes = New-Object byte[] ($Header.Length + $Pixels.Length)
    [Array]::Copy($Header, 0, $PpmBytes, 0, $Header.Length)
    [Array]::Copy($Pixels, 0, $PpmBytes, $Header.Length, $Pixels.Length)
    [IO.File]::WriteAllBytes($Ppm, $PpmBytes)

    $Jpeg = Join-Path $Fixture "source.jpg"
    $Webp = Join-Path $Fixture "source.webp"
    Invoke-Checked -FilePath (Join-Path $Vendor "cjpeg.exe") -Arguments @(
        "-quality", "95", "-outfile", $Jpeg, $Ppm
    )
    Invoke-Checked -FilePath (Join-Path $Vendor "cwebp.exe") -Arguments @(
        "-quiet", "-lossless", $Jpeg, "-o", $Webp
    )

    $BeforeJpeg = (Get-Item $Jpeg).Length
    $BeforeWebp = (Get-Item $Webp).Length

    Invoke-Checked -FilePath $Cli -Arguments @(
        "optimize", $Jpeg, $Webp, "--quality", "60", "--strip", "all", "--dry-run"
    )
    Invoke-Checked -FilePath $Cli -Arguments @(
        "optimize", $Jpeg, $Webp, "--quality", "60", "--strip", "all"
    )

    $AfterJpeg = (Get-Item $Jpeg).Length
    $AfterWebp = (Get-Item $Webp).Length
    if ($AfterJpeg -ge $BeforeJpeg) {
        throw "JPEG did not shrink: $BeforeJpeg -> $AfterJpeg"
    }
    if ($AfterWebp -ge $BeforeWebp) {
        throw "WebP did not shrink: $BeforeWebp -> $AfterWebp"
    }

    $VerifiedPpm = Join-Path $Fixture "verified.ppm"
    $VerifiedPng = Join-Path $Fixture "verified.png"
    Invoke-Checked -FilePath (Join-Path $Vendor "djpeg.exe") -Arguments @(
        "-outfile", $VerifiedPpm, $Jpeg
    )
    Invoke-Checked -FilePath (Join-Path $Vendor "dwebp.exe") -Arguments @(
        $Webp, "-o", $VerifiedPng
    )
    if ((Get-Item $VerifiedPpm).Length -eq 0 -or (Get-Item $VerifiedPng).Length -eq 0) {
        throw "A decoded verification image is empty"
    }

    $Project = Join-Path $Fixture "flutter"
    $Assets = Join-Path $Project "assets"
    New-Item -ItemType Directory -Force -Path $Assets | Out-Null
    Copy-Item $Jpeg (Join-Path $Assets "photo.jpg")
    Copy-Item $Webp (Join-Path $Assets "photo.webp")
    Set-Content -Path (Join-Path $Project "pubspec.yaml") -Encoding ASCII -Value @"
name: smoke_test
flutter:
  assets:
    - assets/
"@

    $DoctorOutput = & $Cli doctor --project $Project | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "doctor failed with code $LASTEXITCODE"
    }
    Write-Host $DoctorOutput
    if ($DoctorOutput -notmatch "Framework: Flutter") {
        throw "Flutter framework was not detected"
    }
    if ($DoctorOutput -notmatch "jpeg:\s+1" -or $DoctorOutput -notmatch "webp:\s+1") {
        throw "Expected JPEG and WebP assets were not discovered"
    }
    if ($DoctorOutput -notmatch "jpeg lossy:" -or $DoctorOutput -notmatch "webp lossy:") {
        throw "Lossy backends were not reported"
    }

    Invoke-Checked -FilePath $Cli -Arguments @(
        "optimize", "--project", $Project, "--quality", "65", "--dry-run"
    )
    Invoke-Checked -FilePath $Cli -Arguments @("update", "--dry-run")

    & $Cli optimize $Jpeg --quality 0 *> $null
    if ($LASTEXITCODE -eq 0) {
        throw "Invalid --quality 0 unexpectedly succeeded"
    }

    Write-Host "Windows package smoke test passed"
}
finally {
    if (Test-Path $Fixture) {
        Remove-Item $Fixture -Recurse -Force
    }
}
