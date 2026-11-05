# Builds the release exe into dist\ (MuPDF is built from source on the first run,
# which takes a few minutes).
# Pass -Clean to also delete the cargo build cache afterwards.
param([switch]$Clean)
# Errors are detected via $LASTEXITCODE: with "Stop", PowerShell 5.1 aborts on
# cargo's ordinary progress output to stderr whenever output is redirected.
Set-Location $PSScriptRoot

# Release exes embed source file paths (for panic messages). Map the cargo and project
# folders to neutral names so no personal path, such as the Windows user name, ends up
# in the exe. This replaces the rustflags in .cargo\config.toml, so crt-static is repeated.
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE ".cargo" }
$env:CARGO_ENCODED_RUSTFLAGS = @(
    "-Ctarget-feature=+crt-static",
    "--remap-path-prefix=$cargoHome=cargo",
    "--remap-path-prefix=$PSScriptRoot=src"
) -join [char]0x1f

cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item target\release\recto.exe dist\recto.exe -Force
# MuPDF is linked into the exe; its license (AGPL-3.0) travels with it.
Copy-Item MUPDF-LICENSE.txt dist\MUPDF-LICENSE.txt -Force
Write-Host "Built dist\recto.exe"

if ($Clean) {
    cargo clean
    Write-Host "Removed build cache (target\)"
}
