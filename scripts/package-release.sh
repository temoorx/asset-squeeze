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
cp "$root/LICENSE-libwebp.txt" "$package_dir/LICENSE-libwebp.txt"

copy_notice_if_present() {
  local source="$1"
  local destination="$2"
  if [[ -f "$source" ]]; then
    cp "$source" "$package_dir/$destination"
  fi
}

find_backend() {
  local binary="$1"
  if command -v "$binary" >/dev/null 2>&1; then
    command -v "$binary"
    return 0
  fi

  local candidates=(
    "/opt/homebrew/opt/jpeg-turbo/bin/$binary"
    "/usr/local/opt/jpeg-turbo/bin/$binary"
    "/opt/libjpeg-turbo/bin/$binary"
    "/usr/bin/$binary"
    "/c/ProgramData/chocolatey/bin/$binary.exe"
    "/c/tools/jpegtran/$binary.exe"
  )

  for candidate in "${candidates[@]}"; do
    if [[ -f "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done

  return 1
}

for backend in jpegtran cjpeg djpeg cwebp dwebp; do
  backend_path="$(find_backend "$backend" || true)"
  if [[ -z "$backend_path" ]]; then
    echo "$backend was not found; install it before packaging" >&2
    exit 1
  fi

  if [[ "$platform" == windows-* ]]; then
    cp "$backend_path" "$vendor_dir/$backend.exe"
  else
    cp "$backend_path" "$vendor_dir/$backend"
    chmod +x "$vendor_dir/$backend"
  fi
done

if [[ "$platform" == macos-* ]]; then
  jpegtran="$(find_backend jpegtran)"
  jpeg_prefix="$(brew --prefix jpeg-turbo)"
  jpeg_dependency="$(otool -L "$jpegtran" | awk '/libjpeg.*dylib/{print $1; exit}')"
  if [[ -n "$jpeg_dependency" ]]; then
    jpeg_name="$(basename "$jpeg_dependency")"
    cp "$jpeg_prefix/lib/$jpeg_name" "$vendor_dir/$jpeg_name"
    for backend in jpegtran cjpeg djpeg; do
      install_name_tool -change "$jpeg_dependency" "@loader_path/$jpeg_name" "$vendor_dir/$backend"
      codesign --force --sign - "$vendor_dir/$backend"
    done
  fi
  copy_notice_if_present "$jpeg_prefix/LICENSE.md" "LICENSE-libjpeg-turbo.md"
  copy_notice_if_present "$jpeg_prefix/README.ijg" "README-libjpeg-ijg.txt"
elif [[ "$platform" == linux-* ]]; then
  jpegtran="$(find_backend jpegtran)"
  jpeg_library="$(ldd "$jpegtran" | awk '/libjpeg\.so/{print $3; exit}')"
  if [[ -n "$jpeg_library" ]]; then
    cp "$jpeg_library" "$vendor_dir/$(basename "$jpeg_library")"
    for backend in jpegtran cjpeg djpeg; do
      patchelf --set-rpath '$ORIGIN' "$vendor_dir/$backend"
    done
  fi
  copy_notice_if_present "/usr/share/doc/libjpeg-turbo-progs/copyright" "LICENSE-libjpeg-turbo.txt"
elif [[ "$platform" == windows-* ]]; then
  jpegtran="$(find_backend jpegtran)"
  jpeg_root="$(dirname "$(dirname "$jpegtran")")"
  find "$(dirname "$jpegtran")" -maxdepth 1 -type f -name '*.dll' -exec cp {} "$vendor_dir" \;
  copy_notice_if_present "$jpeg_root/LICENSE.md" "LICENSE-libjpeg-turbo.md"
  copy_notice_if_present "$jpeg_root/README.ijg" "README-libjpeg-ijg.txt"
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
