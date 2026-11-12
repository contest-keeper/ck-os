#!/usr/bin/env bash
# Filesystem helpers for apply-to-image.sh. Source this file; do not run it directly.
# COMMANDS blocks contain the actual debugfs instructions added to the patch plan.
# In those blocks, "sif" sets file metadata such as owner, group and permissions.

find_linux_partition() {
  local image_file=$1
  local linux_partition_type=0FC63DAF-8483-4772-8E79-3D69D8477DE4
  local partition_table partition root_sector
  partition_table=$(sfdisk -d "$image_file")

  # A partition line contains: start= 2048, size= ..., type=<partition type>.
  while IFS= read -r partition; do
    if [[ "$partition" != *"type=$linux_partition_type"* || "$partition" != *"start="* ]]; then
      continue
    fi

    root_sector=${partition#*start=}         # Keep the text after start=.
    root_sector=${root_sector%%,*}           # Stop at the next comma.
    root_sector=${root_sector//[[:space:]]/} # Remove padding spaces.
    if [[ "$root_sector" =~ ^[0-9]+$ ]]; then
      printf '%s\n' "$root_sector"
      return 0
    fi
  done <<<"$partition_table"

  echo "No Linux root partition found in $image_file" >&2
  return 1
}

prepare_payload() {
  local source_directory=$1
  local payload_directory=$2
  cp -r "$source_directory" "$payload_directory"
  # The current reference payload contains text files only.
  find "$payload_directory" -type f -exec sed -i 's/\r$//' {} +
}

add_agent_binary() {
  local agent_file=$1
  local payload_directory=$2
  # A Linux program starts with the bytes 7f 'E' 'L' 'F'. This catches a wrong file, such as the
  # Windows build, before anything is written to the image.
  if [[ ! -s "$agent_file" || "$(head -c 4 "$agent_file" | od -An -tx1 | tr -d ' 
')" != 7f454c46 ]]; then
    echo "$agent_file is not a Linux program. Build it with tools/build-agent.ps1." >&2
    return 1
  fi
  mkdir -p "$payload_directory/usr/local/bin"
  cp "$agent_file" "$payload_directory/usr/local/bin/ck-agent"
}

list_payload_paths() {
  local payload_directory=$1
  local file_type=$2
  (
    cd "$payload_directory" || exit 1
    # Paths are relative to the image root, for example /etc/ck/image-version.
    find . -mindepth 1 -type "$file_type" -printf '/%P\n' | sort
  )
}

read_manifest() {
  local manifest_file=$1
  # Remove Windows line endings, comments, trailing spaces, then blank lines.
  sed \
    -e 's/\r$//' \
    -e 's/#.*//' \
    -e 's/[[:space:]]*$//' \
    "$manifest_file" | grep -v '^$' || true
}

image_path_exists() {
  local filesystem=$1
  local image_path=$2
  # debugfs reports an inode when the requested image path exists.
  debugfs -R "stat $image_path" "$filesystem" 2>/dev/null | grep -q '^Inode:'
}

queue_missing_directories() {
  local payload_directory=$1
  local filesystem=$2
  local commands_file=$3
  local image_path

  while IFS= read -r image_path; do
    # Recreating an existing directory with debugfs can corrupt the filesystem.
    if image_path_exists "$filesystem" "$image_path"; then
      continue
    fi

    # sif means "set inode field"; user 0 and group 0 are root.
    cat >>"$commands_file" <<COMMANDS
mkdir $image_path
sif $image_path uid 0
sif $image_path gid 0
COMMANDS
  done < <(list_payload_paths "$payload_directory" d)
}

queue_image_file() {
  local source_file=$1
  local image_path=$2
  local mode=$3
  local commands_file=$4
  local image_directory image_name
  image_directory=$(dirname "$image_path")
  image_name=$(basename "$image_path")

  # debugfs writes into its current directory, so select the destination first.
  cat >>"$commands_file" <<COMMANDS
cd $image_directory
rm $image_name
write $source_file $image_name
sif $image_path uid 0
sif $image_path gid 0
sif $image_path mode $mode
cd /
COMMANDS
}

payload_file_mode() {
  local image_path=$1
  # Mode 0644 allows only the owner to write; 0755 also allows execution.
  case "$image_path" in
    /usr/local/bin/ck-recorder | /usr/local/bin/ck-agent | /usr/local/bin/ck-welcome)
      printf '0100755\n'
      ;;
    *)
      printf '0100644\n'
      ;;
  esac
}

queue_payload_files() {
  local payload_directory=$1
  local commands_file=$2
  local image_path source_file mode

  while IFS= read -r image_path; do
    source_file="$payload_directory$image_path"
    mode=$(payload_file_mode "$image_path")
    queue_image_file "$source_file" "$image_path" "$mode" "$commands_file"
  done < <(list_payload_paths "$payload_directory" f)
}

queue_disabled_services() {
  local manifest_file=$1
  local commands_file=$2
  local service_name

  while IFS= read -r service_name; do
    queue_disabled_service "$service_name" "$commands_file"
  done < <(read_manifest "$manifest_file")
}

queue_disabled_service() {
  local service_name=$1
  local commands_file=$2
  local wants_directory
  # Each folder contains links that start services for a particular boot stage.
  local wants_directories=(
    multi-user.target.wants
    graphical.target.wants
    timers.target.wants
    sysinit.target.wants
    sockets.target.wants
    paths.target.wants
    network-online.target.wants
  )

  for wants_directory in "${wants_directories[@]}"; do
    printf 'rm /etc/systemd/system/%s/%s\n' "$wants_directory" "$service_name" >>"$commands_file"
  done
  # A service linked to /dev/null is masked: systemd cannot start it.
  cat >>"$commands_file" <<COMMANDS
rm /etc/systemd/system/$service_name
symlink /etc/systemd/system/$service_name /dev/null
COMMANDS
}

queue_removed_files() {
  local manifest_file=$1
  local commands_file=$2
  local image_path
  while IFS= read -r image_path; do
    printf 'rm %s\n' "$image_path" >>"$commands_file"
  done < <(read_manifest "$manifest_file")
}

queue_enabled_service() {
  local service_name=$1
  local target=$2
  local commands_file=$3
  local service_link="/etc/systemd/system/$target.wants/$service_name"
  cat >>"$commands_file" <<COMMANDS
rm $service_link
symlink $service_link /etc/systemd/system/$service_name
COMMANDS
}

queue_efi_permissions() {
  local filesystem=$1
  local work_directory=$2
  local commands_file=$3
  local fstab_file="$work_directory/fstab"
  debugfs -R "dump /etc/fstab $fstab_file" "$filesystem" 2>/dev/null

  # Match the EFI mount entry and replace its default permissions with umask=0077.
  # The capture group preserves the mount path, filesystem type and spacing.
  # This makes the EFI settings and stick key readable only by root.
  sed -i 's#\(/boot/efi[[:space:]]\+vfat[[:space:]]\+\)defaults#\1umask=0077#' "$fstab_file"
  if grep -q '/boot/efi.*umask=0077' "$fstab_file"; then
    queue_image_file "$fstab_file" /etc/fstab 0100644 "$commands_file"
  else
    # Fail closed: without this the stick key would stay readable by every user on the machine.
    echo "ERROR: no '/boot/efi vfat defaults' line in /etc/fstab, so the EFI partition cannot be" \
      "made root-only; /etc/fstab left unchanged and nothing was applied" >&2
    return 1
  fi
}

apply_image_commands() {
  local filesystem=$1
  local commands_file=$2
  local work_directory=$3
  local debugfs_log="$work_directory/debugfs.log"
  local error_pattern='error|denied|invalid|no space|corrupt'
  local expected_pattern='File not found by ext2_lookup|already exists|Ext2 inode is not a directory'

  echo "Applying $(wc -l <"$commands_file") changes..."
  debugfs -w -f "$commands_file" "$filesystem" >"$debugfs_log" 2>&1 || true
  # Missing old files are expected when applying the reference patch again.
  # Read-back verification and the filesystem check determine the final result.
  grep -iE "$error_pattern" "$debugfs_log" | grep -viE "$expected_pattern" || true
}

verify_payload() {
  local payload_directory=$1
  local filesystem=$2
  local image_path source_file
  local verification_failed=0

  while IFS= read -r image_path; do
    source_file="$payload_directory$image_path"
    # Compare the file read from the image with the local file we installed.
    if ! cmp -s <(debugfs -R "cat $image_path" "$filesystem" 2>/dev/null) "$source_file"; then
      echo "MISMATCH $image_path"
      verification_failed=1
    fi
    verify_file_permissions "$filesystem" "$image_path" || verification_failed=1
  done < <(list_payload_paths "$payload_directory" f)
  return "$verification_failed"
}

verify_file_permissions() {
  local filesystem=$1
  local image_path=$2
  local expected_mode details actual_mode actual_owner
  expected_mode=$(payload_file_mode "$image_path")
  expected_mode=${expected_mode: -4} # debugfs shows 0755, not 0100755.

  # debugfs prints "Mode:  0755" and "User:     0   Group:     0" for an inode.
  details=$(debugfs -R "stat $image_path" "$filesystem" 2>/dev/null)
  actual_mode=$(sed -n 's/.*Mode: *\([0-7]*\).*/\1/p' <<<"$details")
  actual_owner=$(sed -n 's/^User: *\([0-9]*\) *Group: *\([0-9]*\).*/\1:\2/p' <<<"$details")
  if [[ "$actual_mode" != "$expected_mode" || "$actual_owner" != "0:0" ]]; then
    echo "WRONG PERMISSIONS $image_path (mode ${actual_mode:-?}, owner ${actual_owner:-?})"
    return 1
  fi
}

verify_link() {
  local filesystem=$1
  local link_path=$2
  local expected_target=$3
  local actual_target
  # Short symbolic links are shown by debugfs as: Fast link dest: "<target>".
  actual_target=$(debugfs -R "stat $link_path" "$filesystem" 2>/dev/null |
    sed -n 's/.*Fast link dest: "\(.*\)".*/\1/p')
  if [[ "$actual_target" != "$expected_target" ]]; then
    echo "MISSING LINK $link_path -> $expected_target"
    return 1
  fi
}

verify_disabled_services() {
  local manifest_file=$1
  local filesystem=$2
  local service_name
  local verification_failed=0

  while IFS= read -r service_name; do
    verify_link "$filesystem" "/etc/systemd/system/$service_name" /dev/null || verification_failed=1
  done < <(read_manifest "$manifest_file")
  return "$verification_failed"
}

verify_enabled_service() {
  local service_name=$1
  local target=$2
  local filesystem=$3
  verify_link "$filesystem" "/etc/systemd/system/$target.wants/$service_name" \
    "/etc/systemd/system/$service_name"
}

verify_removed_files() {
  local manifest_file=$1
  local filesystem=$2
  local image_path
  local verification_failed=0

  while IFS= read -r image_path; do
    if image_path_exists "$filesystem" "$image_path"; then
      echo "STILL PRESENT $image_path"
      verification_failed=1
    fi
  done < <(read_manifest "$manifest_file")
  return "$verification_failed"
}

verify_efi_permissions() {
  local filesystem=$1
  local efi_entry
  # The EFI partition holds the stick key, so its mount entry must be root-only.
  efi_entry=$(debugfs -R "cat /etc/fstab" "$filesystem" 2>/dev/null |
    grep -E '^[^#]*[[:space:]]/boot/efi[[:space:]]' || true)
  # A missing entry is as bad as a wrong one: the image would not mount the settings root-only.
  if [[ -z "$efi_entry" ]]; then
    echo "INSECURE /boot/efi has no entry in /etc/fstab"
    return 1
  fi
  if [[ "$efi_entry" != *umask=0077* ]]; then
    echo "INSECURE /boot/efi is not root-only in /etc/fstab"
    return 1
  fi
}

verify_filesystem() {
  local filesystem=$1
  local work_directory=$2
  local fsck_log="$work_directory/fsck.log"
  echo "Checking the filesystem (read-only)..."
  if ! e2fsck -fn "$filesystem" >"$fsck_log" 2>&1; then
    tail -20 "$fsck_log"
    return 1
  fi
}
