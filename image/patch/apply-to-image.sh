#!/usr/bin/env bash
# Apply the Keeper files and settings to a COPY of the contest image.
# Usage: bash apply-to-image.sh --in-place <image.img> --agent <ck-agent>
#   The image file is modified: pass a copy. <ck-agent> is the static Linux build from
#   tools/build-agent.ps1.
# Requires Bash, debugfs, e2fsck and sfdisk. Ubuntu cloud images also need mtools.
# On Windows, run through WSL.
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

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib/image-tools.sh
source "$patch_directory/lib/image-tools.sh"
# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib/cloud-boot.sh
source "$patch_directory/lib/cloud-boot.sh"

# 1. Locate the Linux partition inside the image.
root_sector=$(find_linux_partition "$image_file")
filesystem="$image_file?offset=$((root_sector * 512))"
echo "Root filesystem found at sector $root_sector"
# A failure here must stop the script: set -e aborts on the failed command substitution.
# Do not add "|| true" to this line.
boot_filesystem=$(resolve_cloud_boot "$image_file" "$filesystem" "$root_sector")

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
prepare_legacy_reset "$filesystem" "$work_directory" "$payload_directory"
if [[ -n "$boot_filesystem" ]]; then
  prepare_cloud_payload "$patch_directory" "$payload_directory"
  boot_payload="$work_directory/bootfs"
  boot_commands="$work_directory/boot-commands"
  prepare_cloud_grub "$boot_filesystem" "$payload_directory" "$boot_payload"
  : >"$boot_commands"
  queue_payload_files "$boot_payload" "$boot_commands"
  verify_filesystem "$filesystem" "$work_directory"
  verify_filesystem "$boot_filesystem" "$work_directory"
fi
queue_missing_directories "$payload_directory" "$filesystem" "$commands_file"
queue_payload_files "$payload_directory" "$commands_file"
queue_disabled_services "$patch_directory/mask-units.txt" "$commands_file"
queue_removed_files "$patch_directory/remove-files.txt" "$commands_file"
queue_enabled_service ck-recorder.service graphical.target "$commands_file"
if ! image_path_exists "$filesystem" /etc/systemd/system/timers.target.wants; then
  printf 'mkdir /etc/systemd/system/timers.target.wants\n' >>"$commands_file"
fi
queue_enabled_service ck-recording-upload.timer timers.target "$commands_file"
queue_enabled_service ck-config.service multi-user.target "$commands_file"
queue_enabled_service ck-netlock.service sysinit.target "$commands_file"
queue_direct_dns "$commands_file"
queue_enabled_service ck-selftest.service multi-user.target "$commands_file"
queue_enabled_service ck-uplink.service multi-user.target "$commands_file"
queue_enabled_service ck-capture.service multi-user.target "$commands_file"
queue_efi_permissions "$filesystem" "$work_directory" "$commands_file"
if [[ -z "$boot_filesystem" ]]; then
  queue_squid_include "$filesystem" "$work_directory" "$commands_file"
  queue_grub_lock "$filesystem" "$work_directory" "$commands_file"
fi
queue_group_cleanup "$filesystem" "$work_directory" "$commands_file"

# 4. Apply the command list to the image.
if [[ -n "$boot_filesystem" ]]; then
  apply_image_commands "$boot_filesystem" "$boot_commands" "$work_directory"
  verify_payload "$boot_payload" "$boot_filesystem"
  verify_filesystem "$boot_filesystem" "$work_directory"
fi
apply_image_commands "$filesystem" "$commands_file" "$work_directory"

# 5. Read every change back from the image and check filesystem health.
# All checks run, even if one fails, so the output lists every problem.
verification_failed=0
verify_payload "$payload_directory" "$filesystem" || verification_failed=1
verify_disabled_services "$patch_directory/mask-units.txt" "$filesystem" || verification_failed=1
verify_removed_files "$patch_directory/remove-files.txt" "$filesystem" || verification_failed=1
verify_enabled_service ck-recorder.service graphical.target "$filesystem" || verification_failed=1
verify_enabled_service ck-recording-upload.timer timers.target "$filesystem" || verification_failed=1
verify_enabled_service ck-config.service multi-user.target "$filesystem" || verification_failed=1
verify_enabled_service ck-netlock.service sysinit.target "$filesystem" || verification_failed=1
verify_direct_dns "$filesystem" || verification_failed=1
verify_enabled_service ck-selftest.service multi-user.target "$filesystem" || verification_failed=1
verify_enabled_service ck-uplink.service multi-user.target "$filesystem" || verification_failed=1
verify_enabled_service ck-capture.service multi-user.target "$filesystem" || verification_failed=1
verify_squid_include "$filesystem" || verification_failed=1
verify_efi_permissions "$filesystem" || verification_failed=1
if [[ -z "$boot_filesystem" ]]; then
  verify_grub_lock "$filesystem" || verification_failed=1
fi
verify_group_cleanup "$filesystem" || verification_failed=1
verify_filesystem "$filesystem" "$work_directory" || verification_failed=1

if [[ "$verification_failed" == 0 ]]; then
  image_version=$(debugfs -R 'cat /etc/ck/image-version' "$filesystem" 2>/dev/null)
  echo "Patch applied and verified: $image_version"
fi
exit "$verification_failed"
