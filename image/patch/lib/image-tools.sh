#!/usr/bin/env bash
# Filesystem helpers for apply-to-image.sh. Source this file; do not run it directly.
# COMMANDS blocks contain the actual debugfs instructions added to the patch plan.
# In those blocks, "sif" sets file metadata such as owner, group and permissions.

find_linux_partition() {
  local image_file=$1
  local linux_partition_type=0FC63DAF-8483-4772-8E79-3D69D8477DE4
  local partition_table partition root_sector
  partition_table=$(sfdisk -d "$image_file")

  # A partition line looks like: start= 2048, size= ..., type=<partition type>.
  while IFS= read -r partition; do
    if [[ "$partition" != *"type=$linux_partition_type"* || "$partition" != *"start="* ]]; then
      continue
    fi

    root_sector=${partition#*start=}
    root_sector=${root_sector%%,*}
    root_sector=${root_sector//[[:space:]]/}
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
  # Only text files are here at this point; the agent binary is added afterwards.
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
    find . -mindepth 1 -type "$file_type" -printf '/%P\n' | sort
  )
}

read_manifest() {
  local manifest_file=$1
  sed \
    -e 's/\r$//' \
    -e 's/#.*//' \
    -e 's/[[:space:]]*$//' \
    "$manifest_file" | grep -v '^$' || true
}

image_path_exists() {
  local filesystem=$1
  local image_path=$2
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
  case "$image_path" in
    /grub/grub.cfg | /grub/custom.cfg)
      printf '0100600\n'
      ;;
    /usr/local/bin/ck-recorder | /usr/local/bin/ck-agent | /usr/local/bin/ck-welcome | /usr/local/sbin/ck-squid-ca)
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

# The stub resolver at 127.0.0.53 sends lookups for anyone, so it is not allowed to use the
# network (see netlock.nft). Pointing resolv.conf at the upstream servers lets Squid ask them itself.
queue_direct_dns() {
  local commands_file=$1
  cat >>"$commands_file" <<COMMANDS
rm /etc/resolv.conf
symlink /etc/resolv.conf /run/systemd/resolve/resolv.conf
COMMANDS
}

queue_efi_permissions() {
  local filesystem=$1
  local work_directory=$2
  local commands_file=$3
  local fstab_file="$work_directory/fstab"
  debugfs -R "dump /etc/fstab $fstab_file" "$filesystem" 2>/dev/null

  # umask=0077 makes the EFI partition, which holds the settings and the stick key, root-only.
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

SQUID_INCLUDE_LINE='include /etc/squid/ck-capture.conf'

# Squid does not read a folder of extra files, so one line is added at the end of its settings. It
# is added once: applying the patch again leaves the file as it is.
queue_squid_include() {
  local filesystem=$1
  local work_directory=$2
  local commands_file=$3
  local squid_file="$work_directory/squid.conf"
  debugfs -R "dump /etc/squid/squid.conf $squid_file" "$filesystem" 2>/dev/null
  if [[ ! -s "$squid_file" ]]; then
    echo "ERROR: no /etc/squid/squid.conf in the image; nothing was applied" >&2
    return 1
  fi
  if ! grep -qxF "$SQUID_INCLUDE_LINE" "$squid_file"; then
    printf '
%s
' "$SQUID_INCLUDE_LINE" >>"$squid_file"
  fi
  queue_image_file "$squid_file" /etc/squid/squid.conf 0100640 "$commands_file"
  # Squid's own group (proxy, 13) reads this file; queue_image_file made it root's.
  printf 'sif /etc/squid/squid.conf gid 13
' >>"$commands_file"
}

verify_squid_include() {
  local filesystem=$1
  if ! debugfs -R 'cat /etc/squid/squid.conf' "$filesystem" 2>/dev/null | grep -qxF "$SQUID_INCLUDE_LINE"; then
    echo "MISSING $SQUID_INCLUDE_LINE in /etc/squid/squid.conf"
    return 1
  fi
}

GRUB_CONFIG=/boot/grub/grub.cfg

# custom.cfg (a payload file) sets a superuser whose password nobody knows. That also closes the
# normal boot unless every entry at the top of the menu is marked --unrestricted, so this marks them.
# The submenu "Advanced options" (its recovery mode is a root shell) and the entries inside it stay
# locked. Marking is skipped for an entry that already has it, so applying the patch again is safe.
queue_grub_lock() {
  local filesystem=$1
  local work_directory=$2
  local commands_file=$3
  local grub_file="$work_directory/grub.cfg"
  debugfs -R "dump $GRUB_CONFIG $grub_file" "$filesystem" 2>/dev/null
  if ! grep -q '^menuentry ' "$grub_file" 2>/dev/null; then
    echo "ERROR: no menu entries in $GRUB_CONFIG, so GRUB cannot be locked; nothing was applied" >&2
    return 1
  fi
  # Without the line that reads custom.cfg the superuser is never set and the lock does nothing.
  if ! grep -q 'custom\.cfg' "$grub_file"; then
    echo "ERROR: $GRUB_CONFIG does not read custom.cfg, so GRUB cannot be locked; nothing was applied" >&2
    return 1
  fi
  sed -i '/^menuentry .*--unrestricted/!s/^menuentry /menuentry --unrestricted /' "$grub_file"
  queue_image_file "$grub_file" "$GRUB_CONFIG" 0100600 "$commands_file"
}

verify_grub_lock() {
  local filesystem=$1
  local grub_config entry details actual_mode actual_owner
  local verification_failed=0
  grub_config=$(debugfs -R "cat $GRUB_CONFIG" "$filesystem" 2>/dev/null)
  if ! grep -q '^menuentry ' <<<"$grub_config"; then
    echo "MISSING menu entries in $GRUB_CONFIG"
    return 1
  fi
  # An entry without --unrestricted would ask for the superuser's password, and the stick would
  # not boot.
  while IFS= read -r entry; do
    if [[ "$entry" != *--unrestricted* ]]; then
      echo "NOT UNRESTRICTED ${entry:0:70}"
      verification_failed=1
    fi
  done < <(grep '^menuentry ' <<<"$grub_config")
  details=$(debugfs -R "stat $GRUB_CONFIG" "$filesystem" 2>/dev/null)
  actual_mode=$(sed -n 's/.*Mode: *\([0-7]*\).*/\1/p' <<<"$details")
  actual_owner=$(sed -n 's/^User: *\([0-9]*\) *Group: *\([0-9]*\).*/\1:\2/p' <<<"$details")
  if [[ "$actual_mode" != 0600 || "$actual_owner" != 0:0 ]]; then
    echo "WRONG PERMISSIONS $GRUB_CONFIG (mode ${actual_mode:-?}, owner ${actual_owner:-?})"
    verification_failed=1
  fi
  return "$verification_failed"
}

# Prints a group file without one member in one group. Only that group's line is touched. The member
# list is the last field in /etc/group and in /etc/gshadow.
remove_group_member() {
  local group_name=$1
  local member=$2
  local file=$3
  awk -F: -v OFS=: -v group_name="$group_name" -v member="$member" '
    $1 == group_name {
      count = split($NF, members, ",")
      kept = ""
      for (i = 1; i <= count; i++) {
        if (members[i] != member && members[i] != "") {
          kept = kept (kept == "" ? "" : ",") members[i]
        }
      }
      $NF = kept
    }
    { print }
  ' "$file"
}

# lpadmin lets its members administer CUPS, which runs as root, and root may use the network (see
# netlock.nft). A printer the contestant added at any address would carry data past Squid. Printing
# a file needs no group, so the contestant leaves it.
queue_group_cleanup() {
  local filesystem=$1
  local work_directory=$2
  local commands_file=$3
  local group_file="$work_directory/group"
  local gshadow_file="$work_directory/gshadow"
  debugfs -R "dump /etc/group $group_file.old" "$filesystem" 2>/dev/null
  debugfs -R "dump /etc/gshadow $gshadow_file.old" "$filesystem" 2>/dev/null
  if [[ ! -s "$group_file.old" || ! -s "$gshadow_file.old" ]]; then
    echo "ERROR: no /etc/group or /etc/gshadow in the image; nothing was applied" >&2
    return 1
  fi
  remove_group_member lpadmin contestant "$group_file.old" >"$group_file"
  remove_group_member lpadmin contestant "$gshadow_file.old" >"$gshadow_file"
  queue_image_file "$group_file" /etc/group 0100644 "$commands_file"
  queue_image_file "$gshadow_file" /etc/gshadow 0100640 "$commands_file"
  # Group shadow (42) reads this file; queue_image_file made it root's.
  printf 'sif /etc/gshadow gid 42\n' >>"$commands_file"
}

verify_group_cleanup() {
  local filesystem=$1
  local file details actual_mode actual_owner expected_mode expected_owner
  local verification_failed=0
  for file in /etc/group /etc/gshadow; do
    if debugfs -R "cat $file" "$filesystem" 2>/dev/null |
      awk -F: '$1 == "lpadmin" { n = split($NF, m, ","); for (i = 1; i <= n; i++) if (m[i] == "contestant") found = 1 } END { exit !found }'; then
      echo "STILL IN lpadmin: contestant, in $file"
      verification_failed=1
    fi
    if [[ "$file" == /etc/gshadow ]]; then
      expected_mode=0640
      expected_owner=0:42
    else
      expected_mode=0644
      expected_owner=0:0
    fi
    details=$(debugfs -R "stat $file" "$filesystem" 2>/dev/null)
    actual_mode=$(sed -n 's/.*Mode: *\([0-7]*\).*/\1/p' <<<"$details")
    actual_owner=$(sed -n 's/^User: *\([0-9]*\) *Group: *\([0-9]*\).*/\1:\2/p' <<<"$details")
    if [[ "$actual_mode" != "$expected_mode" || "$actual_owner" != "$expected_owner" ]]; then
      echo "WRONG PERMISSIONS $file (mode ${actual_mode:-?}, owner ${actual_owner:-?})"
      verification_failed=1
    fi
  done
  return "$verification_failed"
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

verify_direct_dns() {
  verify_link "$1" /etc/resolv.conf /run/systemd/resolve/resolv.conf
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
