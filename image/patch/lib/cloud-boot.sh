#!/usr/bin/env bash
# Resolve the stock Ubuntu cloud image's EFI GRUB forwarder before planning any writes.

cloud_boot_error() {
  echo "ERROR: $1; nothing was applied" >&2
}

# Cloud images have stock Squid and Firefox packages. Use the same proxy and CA policy as Keeper OS.
prepare_cloud_payload() {
  local patch_directory=$1 payload_directory=$2 source file destination
  source="$patch_directory/keeper-rootfs"
  if [[ ! -d $source ]]; then
    source="$patch_directory/../keeper/rootfs"
  fi
  for file in \
    etc/squid/acl_allowed_domains.txt \
    etc/squid/squid.conf \
    etc/systemd/system/ck-squid-ca.service \
    etc/systemd/system/squid.service.d/10-ck.conf \
    usr/lib/firefox/distribution/policies.json \
    usr/local/sbin/ck-squid-ca; do
    if [[ ! -f "$source/$file" ]]; then
      cloud_boot_error "missing cloud image payload $file"
      return 1
    fi
    destination="$payload_directory/$file"
    mkdir -p "$(dirname "$destination")"
    sed 's/\r$//' "$source/$file" >"$destination"
  done
}

# A verified sfdisk dump must contain one partition of the requested type.
cloud_partition_geometry() {
  local table=$1 type=$2
  awk -v type="$type" '
    $0 ~ ", type=" type "(,|$)" {
      count++
      if (match($0, /: start= *[0-9]+, size= *[0-9]+,/)) {
        geometry = substr($0, RSTART, RLENGTH)
        gsub(/[^0-9]+/, " ", geometry)
        split(geometry, fields)
        start = fields[1]; size = fields[2]
      }
    }
    END {
      if (count != 1 || !start || !size || length(start) > 15 || length(size) > 15) exit 1
      print start, size
    }
  ' <<<"$table"
}

cloud_filesystem_stats() {
  local filesystem=$1 partition_sectors=$2 stats
  stats=$(LC_ALL=C debugfs -R stats "$filesystem" 2>/dev/null) || return 1
  awk -v sectors="$partition_sectors" '
    /^Block count:/ { count = $NF }
    /^Block size:/ { size = $NF }
    END { exit !(count ~ /^[0-9]+$/ && count > 0 &&
      (size == 1024 || size == 2048 || size == 4096) && count <= sectors * 512 / size) }
  ' <<<"$stats" || return 1
  printf '%s\n' "$stats"
}

resolve_cloud_boot() {
  local image_file=$1 filesystem=$2 root_sector=$3
  local fstab boot_mount table root boot esp root_start root_size boot_start boot_size esp_start
  local boot_fs boot_stats boot_uuid esp_image efi_boot efi_ubuntu forwarder expected
  fstab=$(debugfs -R 'cat /etc/fstab' "$filesystem" 2>/dev/null) || return 1
  boot_mount=$(awk '$1 !~ /^#/ && $2 == "/boot" { print $1, $3 }' <<<"$fstab")
  # Images with /boot on the root partition retain the original patch path.
  [[ -n "$boot_mount" ]] || return 0
  if [[ "$boot_mount" != 'LABEL=BOOT ext4' ]]; then
    cloud_boot_error 'unsupported separate /boot mount'
    return 1
  fi
  if ! command -v mtype >/dev/null || ! command -v mdir >/dev/null; then
    cloud_boot_error 'mtools is required to check the cloud image EFI partition'
    return 1
  fi

  table=$(LC_ALL=C sfdisk -d "$image_file") || return 1
  if ! grep -qxF 'label: gpt' <<<"$table" || ! grep -qxF 'sector-size: 512' <<<"$table" ||
    ! LC_ALL=C sfdisk --verify "$image_file" >/dev/null ||
    ! root=$(cloud_partition_geometry "$table" 0FC63DAF-8483-4772-8E79-3D69D8477DE4) ||
    ! boot=$(cloud_partition_geometry "$table" BC13C2FF-59E6-4262-A352-B275FD6F7172) ||
    ! esp=$(cloud_partition_geometry "$table" C12A7328-F81F-11D2-BA4B-00A0C93EC93B); then
    cloud_boot_error 'expected unique cloud image root, BOOT and EFI GPT partitions'
    return 1
  fi
  read -r root_start root_size <<<"$root"
  read -r boot_start boot_size <<<"$boot"
  esp_start=${esp%% *}
  boot_fs="$image_file?offset=$((boot_start * 512))"
  esp_image="$image_file@@$((esp_start * 512))"
  if [[ "$root_start" != "$root_sector" ]] || ! cloud_filesystem_stats "$filesystem" "$root_size" >/dev/null ||
    ! boot_stats=$(cloud_filesystem_stats "$boot_fs" "$boot_size") ||
    ! grep -qE '^Filesystem volume name:[[:space:]]+BOOT$' <<<"$boot_stats"; then
    cloud_boot_error 'root or BOOT filesystem does not match the cloud image partition table'
    return 1
  fi
  boot_uuid=$(awk '/^Filesystem UUID:/ { print $NF }' <<<"$boot_stats")
  if [[ ! "$boot_uuid" =~ ^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$ ]] ||
    ! efi_boot=$(mdir -b -i "$esp_image" ::/EFI/BOOT) ||
    ! efi_ubuntu=$(mdir -b -i "$esp_image" ::/EFI/ubuntu) ||
    ! grep -qixF '::/EFI/BOOT/BOOTX64.EFI' <<<"$efi_boot" ||
    grep -qiE '/(grub|custom)\.cfg(/|$)' <<<"$efi_boot" ||
    grep -qiE '/custom\.cfg(/|$)' <<<"$efi_ubuntu" ||
    ! forwarder=$(mtype -i "$esp_image" ::/EFI/ubuntu/grub.cfg | sed 's/[[:space:]]*$//'); then
    cloud_boot_error 'expected the stock Ubuntu EFI loader and forwarder'
    return 1
  fi
  expected=$(printf 'search.fs_uuid %s root\n%s\n%s' "$boot_uuid" "set prefix=(\$root)'/grub'" "configfile \$prefix/grub.cfg")
  if [[ "$forwarder" != "$expected" ]]; then
    cloud_boot_error 'EFI GRUB forwarder does not identify the BOOT filesystem'
    return 1
  fi
  printf '%s\n' "$boot_fs"
}

prepare_cloud_grub() {
  local filesystem=$1 root_payload=$2 boot_payload=$3 grub_file="$3/grub/grub.cfg"
  mkdir -p "$boot_payload/grub"
  debugfs -R "dump /grub/grub.cfg $grub_file" "$filesystem" 2>/dev/null
  # This branch handles the generated Ubuntu menu and its single supported include block.
  # Treat other menu/include layouts as unsupported so their access rules cannot be weakened.
  if ! awk '
    /^[[:space:]]*#/ { next }
    /^menuentry (--unrestricted )?\047Ubuntu\047 / {
      ubuntu++
      if (gsub(/--unrestricted/, "&") > 1) unsupported = 1
      next
    }
    /^menuentry[[:space:]]/ || /--unrestricted|(^|[[:space:];])(configfile|password|password_pbkdf2)([[:space:];]|$)/ ||
      /^[[:space:]]*set[[:space:]]+superusers=/ { unsupported = 1 }
    $0 == "if [ -f  ${config_directory}/custom.cfg ]; then" { include_start = NR }
    /(^|[[:space:];])source[[:space:]]/ {
      if (include_start && NR == include_start + 1 && $0 == "  source ${config_directory}/custom.cfg") source_directory = NR
      else if (source_directory == NR - 2 && previous == "elif [ -z \"${config_directory}\" -a -f  $prefix/custom.cfg ]; then" &&
        $0 == "  source $prefix/custom.cfg") source_prefix = NR
      else unsupported = 1
    }
    source_prefix == NR - 1 && $0 == "fi" { include++ }
    { previous = $0 }
    END { exit !(ubuntu == 1 && include == 1 && !unsupported) }
  ' "$grub_file"; then
    cloud_boot_error 'unsupported Ubuntu BOOT GRUB menu or custom.cfg include'
    return 1
  fi
  sed -i '/^menuentry --unrestricted /!s/^menuentry /menuentry --unrestricted /' "$grub_file"
  mv "$root_payload/boot/grub/custom.cfg" "$boot_payload/grub/custom.cfg"
  rmdir "$root_payload/boot/grub" "$root_payload/boot"
}
