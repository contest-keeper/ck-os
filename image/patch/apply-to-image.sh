#!/usr/bin/env bash
# Apply the Keeper files and settings to a COPY of the contest image.
# Usage: bash apply-to-image.sh --in-place <image.img> --agent <ck-agent>
#   The image file is modified: pass a copy. <ck-agent> is the static Linux build from
#   tools/build-agent.ps1.
# Requires Bash, debugfs, e2fsck and sfdisk. On Windows, run through WSL.
set -euo pipefail

usage="usage: apply-to-image.sh --in-place <image.img> --agent <ck-agent>"
# The patch changes the image file itself. Never run it on the original image: make a copy
# first, and pass --in-place to confirm that the file you name is that copy.
if [[ "${1:-}" != --in-place ]]; then
  echo "$usage" >&2
  echo "Refusing to run: this changes the image file itself. Work on a COPY of the image" \
    "and pass --in-place to confirm." >&2
  exit 2
fi
shift
image_file=${1:?$usage}
shift
[[ "${1:-}" == --agent ]] || {
  echo "$usage" >&2
  exit 2
}
agent_file=${2:?$usage}
patch_directory=$(cd "$(dirname "$0")" && pwd)

# Keep the low-level filesystem commands separate from the steps below.
# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib/image-tools.sh
source "$patch_directory/lib/image-tools.sh"

# 1. Locate the Linux partition inside the image.
root_sector=$(find_linux_partition "$image_file")
filesystem="$image_file?offset=$((root_sector * 512))"
echo "Root filesystem found at sector $root_sector"

# 2. Prepare a temporary copy of the files and an image command list.
work_directory=$(mktemp -d)
trap 'rm -rf -- "$work_directory"' EXIT
payload_directory="$work_directory/rootfs"
commands_file="$work_directory/commands"
prepare_payload "$patch_directory/rootfs" "$payload_directory"
# Added after prepare_payload, which rewrites line endings and would corrupt a program.
add_agent_binary "$agent_file" "$payload_directory"
: >"$commands_file"

# 3. Plan the changes. The image is not written during these steps.
queue_missing_directories "$payload_directory" "$filesystem" "$commands_file"
queue_payload_files "$payload_directory" "$commands_file"
queue_disabled_services "$patch_directory/mask-units.txt" "$commands_file"
queue_removed_files "$patch_directory/remove-files.txt" "$commands_file"
queue_enabled_service ck-recorder.service graphical.target "$commands_file"
queue_enabled_service ck-config.service multi-user.target "$commands_file"
queue_enabled_service ck-selftest.service multi-user.target "$commands_file"
queue_enabled_service ck-uplink.service multi-user.target "$commands_file"
queue_efi_permissions "$filesystem" "$work_directory" "$commands_file"

# 4. Apply the command list to the image.
apply_image_commands "$filesystem" "$commands_file" "$work_directory"

# 5. Read every change back from the image and check filesystem health.
# All checks run, even if one fails, so the output lists every problem.
verification_failed=0
verify_payload "$payload_directory" "$filesystem" || verification_failed=1
verify_disabled_services "$patch_directory/mask-units.txt" "$filesystem" || verification_failed=1
verify_removed_files "$patch_directory/remove-files.txt" "$filesystem" || verification_failed=1
verify_enabled_service ck-recorder.service graphical.target "$filesystem" || verification_failed=1
verify_enabled_service ck-config.service multi-user.target "$filesystem" || verification_failed=1
verify_enabled_service ck-selftest.service multi-user.target "$filesystem" || verification_failed=1
verify_enabled_service ck-uplink.service multi-user.target "$filesystem" || verification_failed=1
verify_efi_permissions "$filesystem" || verification_failed=1
verify_filesystem "$filesystem" "$work_directory" || verification_failed=1

if [[ "$verification_failed" == 0 ]]; then
  image_version=$(debugfs -R 'cat /etc/ck/image-version' "$filesystem" 2>/dev/null)
  echo "Patch applied and verified: $image_version"
fi
exit "$verification_failed"
