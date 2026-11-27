# shellcheck shell=bash
# apt trusts a .deb that is already in its archive folder by name and size and does not hash it again, so the files
# tools/build-image keeps between builds are checked here before apt sees them.

# verify_apt_cache CACHE DEST KEYRING SOURCE...: copies into the new folder DEST the files of CACHE whose name and
# SHA-256 are in a signed package index of the sources. CACHE is only read, so another build may share it, and each
# copy is what gets hashed. KEYRING is a file or a folder; empty means the build machine's own apt keys. A keyring
# also switches off the expiry date check, as the Debian build does for its dated snapshot.
# The body is a subshell, so its trap removes the downloaded indexes however it ends.
verify_apt_cache() (
  cache=${1:?} dest=${2:?} keyring=$3 kept=0 skipped=0
  shift 3
  mkdir "$dest"
  shopt -s nullglob
  entries=("$cache"/*.deb)
  ((${#entries[@]})) || exit 0
  dir=$(mktemp -d)
  trap 'rm -rf "$dir"' EXIT
  chmod 0755 "$dir" # apt downloads as the user _apt
  mkdir -p "$dir/lists/partial" "$dir/archives/partial" "$dir/none"
  : >"$dir/status"
  printf '%s\n' "$@" >"$dir/sources.list"
  # The build machine's apt settings and update hooks stay out of this.
  printf 'Dir::Etc::Parts "%s";\nDir::Etc::Main "/dev/null";\n' "$dir/none" >"$dir/apt.conf"
  apt=(env "APT_CONFIG=$dir/apt.conf" apt-get -o "Dir::Etc::SourceList=$dir/sources.list"
    -o Dir::Etc::SourceParts=- -o "Dir::State=$dir" -o "Dir::State::Lists=$dir/lists" -o "Dir::State::status=$dir/status"
    -o "Dir::Cache=$dir" -o APT::Architecture=amd64 -o APT::Architectures=amd64 -o Acquire::Languages=none
    -o APT::Update::Error-Mode=any -o Acquire::Retries=8 -o Acquire::http::Timeout=60 -o Acquire::https::Timeout=60)
  if [[ -n $keyring ]]; then
    option=Dir::Etc::Trusted
    [[ ! -d $keyring ]] || option=Dir::Etc::TrustedParts
    apt+=(-o "$option=$keyring" -o Acquire::Check-Valid-Until=false)
  fi
  "${apt[@]}" -q update || exit 1
  # One "SHA256  file name" line per package, with the name apt gives the file in its archive folder.
  # shellcheck disable=SC2016 # $(FILENAME) is apt's own placeholder
  "${apt[@]}" indextargets --format '$(FILENAME)' 'Identifier: Packages' |
    while IFS= read -r list; do /usr/lib/apt/apt-helper cat-file "$list"; done |
    awk '$1 == "Package:" { p = $2 } $1 == "Version:" { v = $2; gsub(":", "%3a", v) } $1 == "Architecture:" { a = $2 }
      $1 == "SHA256:" { s = $2 } /^$/ { if (s != "") print s "  " p "_" v "_" a ".deb"; s = "" }
      END { if (s != "") print s "  " p "_" v "_" a ".deb" }' >"$dir/good"
  [[ -s $dir/good ]] || {
    echo "ERROR: the package index lists no files, so the apt cache cannot be checked" >&2
    exit 1
  }
  for entry in "${entries[@]}"; do
    name=${entry##*/}
    if [[ -f $entry && ! -L $entry ]]; then
      cp -- "$entry" "$dest/$name"
      sum=$(sha256sum <"$dest/$name")
      if grep -qxF "${sum%% *}  $name" "$dir/good"; then
        kept=$((kept + 1))
        continue
      fi
      rm -- "$dest/$name"
    fi
    skipped=$((skipped + 1))
  done
  echo "== apt cache: $kept files match the signed index, $skipped skipped"
)
