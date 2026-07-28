#!/usr/bin/env sh
set -eu

REPO="${ASSET_SQUEEZE_REPO:-temoorx/asset-squeeze}"
INSTALL_DIR="${ASSET_SQUEEZE_INSTALL_DIR:-$HOME/.asset-squeeze}"
BIN_DIR="$INSTALL_DIR/bin"

detect_platform() {
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Darwin) os="macos" ;;
    Linux) os="linux" ;;
    *) echo "Unsupported OS: $os" >&2; exit 1 ;;
  esac

  case "$arch" in
    arm64|aarch64) arch="aarch64" ;;
    x86_64|amd64) arch="x86_64" ;;
    *) echo "Unsupported CPU architecture: $arch" >&2; exit 1 ;;
  esac

  printf '%s-%s' "$os" "$arch"
}

need_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

need_command curl
need_command tar

platform="$(detect_platform)"
archive="asset-squeeze-${platform}.tar.gz"
tmp_dir="$(mktemp -d)"

cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT INT TERM

url="https://github.com/${REPO}/releases/latest/download/${archive}"
checksums_url="https://github.com/${REPO}/releases/latest/download/SHA256SUMS"

echo "Installing asset-squeeze for ${platform}"
echo "Downloading ${url}"

curl -fsSL "$url" -o "$tmp_dir/$archive"
curl -fsSL "$checksums_url" -o "$tmp_dir/SHA256SUMS"

expected_checksum="$(grep "[[:space:]]${archive}$" "$tmp_dir/SHA256SUMS" | awk '{print $1}')"
if [ -z "$expected_checksum" ]; then
  echo "Could not find checksum for $archive" >&2
  exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
  actual_checksum="$(sha256sum "$tmp_dir/$archive" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  actual_checksum="$(shasum -a 256 "$tmp_dir/$archive" | awk '{print $1}')"
else
  echo "Missing sha256sum or shasum for checksum verification" >&2
  exit 1
fi

if [ "$actual_checksum" != "$expected_checksum" ]; then
  echo "Checksum verification failed for $archive" >&2
  exit 1
fi

tar -xzf "$tmp_dir/$archive" -C "$tmp_dir"

mkdir -p "$BIN_DIR"
cp "$tmp_dir/asset-squeeze/asset-squeeze" "$BIN_DIR/asset-squeeze"
chmod +x "$BIN_DIR/asset-squeeze"

rm -rf "$INSTALL_DIR/vendor"
if [ -d "$tmp_dir/asset-squeeze/vendor" ]; then
  cp -R "$tmp_dir/asset-squeeze/vendor" "$INSTALL_DIR/vendor"
  find "$INSTALL_DIR/vendor" -type f -name 'jpegtran*' -exec chmod +x {} \;
fi

if [ -f "$tmp_dir/asset-squeeze/THIRD_PARTY_NOTICES.md" ]; then
  cp "$tmp_dir/asset-squeeze/THIRD_PARTY_NOTICES.md" "$INSTALL_DIR/THIRD_PARTY_NOTICES.md"
fi

case ":$PATH:" in
  *":$BIN_DIR:"*) path_ready="yes" ;;
  *) path_ready="no" ;;
esac

if [ "$path_ready" = "no" ]; then
  shell_name="$(basename "${SHELL:-sh}")"
  case "$shell_name" in
    zsh) profile="$HOME/.zshrc" ;;
    bash) profile="$HOME/.bashrc" ;;
    *) profile="$HOME/.profile" ;;
  esac

  if ! grep -qs "$BIN_DIR" "$profile" 2>/dev/null; then
    {
      echo ''
      echo '# asset-squeeze'
      echo "export PATH=\"$BIN_DIR:\$PATH\""
    } >> "$profile"
  fi

  echo "Added $BIN_DIR to PATH in $profile"
  echo "Restart your terminal or run:"
  echo "  export PATH=\"$BIN_DIR:\$PATH\""
fi

"$BIN_DIR/asset-squeeze" --version

echo ''
echo "asset-squeeze installed successfully."
echo "Try it in a Flutter project:"
echo "  asset-squeeze doctor"
echo "  asset-squeeze optimize --dry-run"
