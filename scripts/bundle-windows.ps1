$ErrorActionPreference = "Stop"

$RootDir = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$DistDir = Join-Path $RootDir "dist\CottonData-windows-x64"
$ZipPath = Join-Path $RootDir "dist\CottonData-windows-x64.zip"

Push-Location $RootDir
try {
    cargo build --release -p cottondata-desktop -p cottondata-cli

    if (Test-Path $DistDir) { Remove-Item -Recurse -Force $DistDir }
    if (Test-Path $ZipPath) { Remove-Item -Force $ZipPath }
    New-Item -ItemType Directory -Path $DistDir -Force | Out-Null

    Copy-Item "target\release\CottonData.exe" (Join-Path $DistDir "CottonData.exe")
    Copy-Item "target\release\cottondata-cli.exe" (Join-Path $DistDir "cottondata-cli.exe")
    Copy-Item "README.md" (Join-Path $DistDir "README.md")
    Copy-Item "LICENSE" (Join-Path $DistDir "LICENSE")

    Compress-Archive -Path "$DistDir\*" -DestinationPath $ZipPath -Force
    Write-Host "Packaged Windows portable archive: $ZipPath"
    Get-Item $ZipPath, (Join-Path $DistDir "CottonData.exe"), (Join-Path $DistDir "cottondata-cli.exe") | Format-Table Name, Length
}
finally {
    Pop-Location
}
