# Build the static Linux ck-agent and copy it to dist/.
# Usage: pwsh tools/build-agent.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    $target = 'x86_64-unknown-linux-musl'
    cargo zigbuild --release --target $target -p ck-agent
    if ($LASTEXITCODE -ne 0) { throw 'cargo zigbuild failed' }
    New-Item -ItemType Directory -Force dist | Out-Null
    Copy-Item "target/$target/release/ck-agent" dist/ck-agent -Force
    Write-Host 'Built dist/ck-agent'
}
finally {
    Pop-Location
}
