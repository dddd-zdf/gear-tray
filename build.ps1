param([switch]$Test)
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
if (Test-Path "$PSScriptRoot/.tools/cargo/bin/cargo.exe") {
    $env:CARGO_HOME = "$PSScriptRoot/.tools/cargo"
    $env:RUSTUP_HOME = "$PSScriptRoot/.tools/rustup"
    $cargoExe = "$PSScriptRoot/.tools/cargo/bin/cargo.exe"
} else {
    $cargoExe = 'cargo'
}
if ($Test) {
    & $cargoExe test --lib
    if ($LASTEXITCODE -ne 0) { throw 'Tests failed' }
}
& $cargoExe build --release --bins
if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
New-Item -ItemType Directory -Force "$PSScriptRoot/dist" | Out-Null
Copy-Item "$PSScriptRoot/target/release/gear-tray.exe" "$PSScriptRoot/dist/GearTray.exe"
Copy-Item "$PSScriptRoot/target/release/gear-diagnostics.exe" "$PSScriptRoot/dist/GearDiagnostics.exe"
Copy-Item "$PSScriptRoot/LICENSE", "$PSScriptRoot/THIRD-PARTY-NOTICES.md", "$PSScriptRoot/README.md" "$PSScriptRoot/dist"
Write-Host 'Built dist/GearTray.exe and dist/GearDiagnostics.exe'
