# Patch a COPY of the contest image with the Keeper OS files and the agent, through WSL.
# Usage: pwsh tools/patch-image.ps1 -Image C:\rcpc-test\ck-os-base.img [-Agent dist\ck-agent]
# The image file is changed in place. Never pass the original image: make a copy first.
# Needs WSL with debugfs, e2fsck and sfdisk (package e2fsprogs and fdisk).
param(
    [Parameter(Mandatory)][string]$Image,
    [string]$Agent
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $Agent) { $Agent = Join-Path $root 'dist\ck-agent' }

foreach ($file in @($Image, $Agent)) {
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "File not found: $file" }
}
$image = (Resolve-Path -LiteralPath $Image).Path
$agent = (Resolve-Path -LiteralPath $Agent).Path
$patch = Join-Path $root 'image\patch'

$wslImage = (wsl -e wslpath -a $image).Trim()
$wslAgent = (wsl -e wslpath -a $agent).Trim()
$wslPatch = (wsl -e wslpath -a $patch).Trim()

# A Windows checkout may have Windows line endings, which break shell scripts and unit files.
# Work from a temporary copy with Unix line endings, so the checkout is never changed.
$script = @'
set -euo pipefail
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
cp -r "$1" "$work/patch"
find "$work/patch" -type f -exec sed -i 's/\r$//' {} +
bash "$work/patch/apply-to-image.sh" --in-place "$2" --agent "$3"
'@
# The script is passed on the command line with Unix line endings.
$script = $script -replace "`r", ''
wsl -e bash -c $script bash $wslPatch $wslImage $wslAgent
if ($LASTEXITCODE -ne 0) { throw "The patch failed or its verification failed (exit code $LASTEXITCODE)" }
