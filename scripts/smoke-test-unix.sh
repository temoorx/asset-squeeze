#!/usr/bin/env bash
set -euo pipefail

platform="${1:?usage: scripts/smoke-test-unix.sh <platform> <root> [installed]}"
root="${2:?usage: scripts/smoke-test-unix.sh <platform> <root> [installed]}"
layout="${3:-package}"

if [[ "$layout" == "installed" ]]; then
  cli="$root/bin/asset-squeeze"
else
  cli="$root/asset-squeeze"
fi
vendor="$root/vendor/bin/$platform"

[[ -x "$cli" ]] || { echo "missing executable: $cli" >&2; exit 1; }
"$cli" --version

for backend in jpegtran cjpeg djpeg cwebp dwebp; do
  tool="$vendor/$backend"
  [[ -x "$tool" ]] || { echo "missing executable backend: $tool" >&2; exit 1; }
  "$tool" -version >/dev/null 2>&1
done

fixture="$(mktemp -d)"
cleanup() {
  rm -rf "$fixture"
}
trap cleanup EXIT INT TERM

awk 'BEGIN {
  printf "P6\n256 256\n255\n"
  for (y = 0; y < 256; y++) {
    for (x = 0; x < 256; x++) {
      printf "%c%c%c", x, y, (x * 3 + y * 5) % 256
    }
  }
}' > "$fixture/source.ppm"

"$vendor/cjpeg" -quality 95 -outfile "$fixture/source.jpg" "$fixture/source.ppm"
"$vendor/cwebp" -quiet -lossless "$fixture/source.jpg" -o "$fixture/source.webp"

before_jpeg="$(wc -c < "$fixture/source.jpg")"
before_webp="$(wc -c < "$fixture/source.webp")"

"$cli" optimize "$fixture/source.jpg" "$fixture/source.webp" --quality 60 --strip all --dry-run
"$cli" optimize "$fixture/source.jpg" "$fixture/source.webp" --quality 60 --strip all

after_jpeg="$(wc -c < "$fixture/source.jpg")"
after_webp="$(wc -c < "$fixture/source.webp")"
[[ "$after_jpeg" -lt "$before_jpeg" ]] || { echo "JPEG did not shrink" >&2; exit 1; }
[[ "$after_webp" -lt "$before_webp" ]] || { echo "WebP did not shrink" >&2; exit 1; }

"$vendor/djpeg" -outfile "$fixture/verified.ppm" "$fixture/source.jpg"
"$vendor/dwebp" "$fixture/source.webp" -o "$fixture/verified.png"
[[ -s "$fixture/verified.ppm" ]] || { echo "decoded JPEG is empty" >&2; exit 1; }
[[ -s "$fixture/verified.png" ]] || { echo "decoded WebP is empty" >&2; exit 1; }

mkdir -p "$fixture/flutter/assets"
cp "$fixture/source.jpg" "$fixture/flutter/assets/photo.jpg"
cp "$fixture/source.webp" "$fixture/flutter/assets/photo.webp"
printf 'name: smoke_test\nflutter:\n  assets:\n    - assets/\n' > "$fixture/flutter/pubspec.yaml"

doctor_output="$("$cli" doctor --project "$fixture/flutter")"
printf '%s\n' "$doctor_output"
grep -q 'Framework: Flutter' <<< "$doctor_output"
grep -Eq 'jpeg:[[:space:]]+1' <<< "$doctor_output"
grep -Eq 'webp:[[:space:]]+1' <<< "$doctor_output"
grep -q 'jpeg lossy:' <<< "$doctor_output"
grep -q 'webp lossy:' <<< "$doctor_output"

"$cli" optimize --project "$fixture/flutter" --quality 65 --dry-run
"$cli" update --dry-run

if "$cli" optimize "$fixture/source.jpg" --quality 0 >/dev/null 2>&1; then
  echo "invalid --quality 0 unexpectedly succeeded" >&2
  exit 1
fi

echo "Unix package smoke test passed for $platform"
