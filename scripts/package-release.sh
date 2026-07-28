#!/usr/bin/env bash
set -euo pipefail

platform="${1:?usage: scripts/package-release.sh <platform>}"
version="${2:?usage: scripts/package-release.sh <platform> <version>}"
binary_name="${3:-asset-squeeze}"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dist="$root/dist"
package_dir="$dist/asset-squeeze"
vendor_dir="$package_dir/vendor/bin/$platform"

rm -rf "$package_dir"
mkdir -p "$vendor_dir"

if [[ "$platform" == windows-* ]]; then
  cp "$root/target/release/${binary_name}.exe" "$package_dir/asset-squeeze.exe"
else
  cp "$root/target/release/$binary_name" "$package_dir/asset-squeeze"
  chmod +x "$package_dir/asset-squeeze"
fi

cp "$root/README.md" "$package_dir/README.md"
cp "$root/LICENSE" "$package_dir/LICENSE"
cp "$root/THIRD_PARTY_NOTICES.md" "$package_dir/THIRD_PARTY_NOTICES.md"

find_jpegtran() {
  if command -v jpegtran >/dev/null 2>&1; then
    command -v jpegtran
    return 0
  fi

  local candidates=(
    "/opt/homebrew/opt/jpeg-turbo/bin/jpegtran"
    "/usr/local/opt/jpeg-turbo/bin/jpegtran"
    "/opt/libjpeg-turbo/bin/jpegtran"
    "/usr/bin/jpegtran"
    "/c/ProgramData/chocolatey/bin/jpegtran.exe"
    "/c/tools/jpegtran/jpegtran.exe"
  )

  for candidate in "${candidates[@]}"; do
    if [[ -f "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done

  return 1
}

jpegtran="$(find_jpegtran || true)"
if [[ -z "$jpegtran" ]]; then
  echo "jpegtran was not found; install it before packaging" >&2
  exit 1
fi

if [[ "$platform" == windows-* ]]; then
  cp "$jpegtran" "$vendor_dir/jpegtran.exe"
else
  cp "$jpegtran" "$vendor_dir/jpegtran"
  chmod +x "$vendor_dir/jpegtran"
fi

mkdir -p "$dist"
archive_base="asset-squeeze-$platform"

(
  cd "$dist"
  if [[ "$platform" == windows-* ]]; then
    rm -f "$archive_base.zip"
    powershell -NoProfile -Command "Compress-Archive -Path asset-squeeze -DestinationPath $archive_base.zip -Force"
  else
    rm -f "$archive_base.tar.gz"
    tar -czf "$archive_base.tar.gz" asset-squeeze
  fi
)

echo "Packaged asset-squeeze $version for $platform"

