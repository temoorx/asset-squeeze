# asset-squeeze

Lossless-first Flutter and React Native asset optimization from one command.

`asset-squeeze` discovers project image assets, optimizes supported files, and keeps every asset in its original format.

```bash
asset-squeeze optimize
```

## Why

Mobile projects often accumulate heavy image assets over time. `asset-squeeze` helps reduce app size without rewriting your asset paths, changing formats, or secretly lowering image quality.

The core rules:

- Same format in, same format out.
- Lossless-first by default.
- Replace files only when the optimized result is smaller.
- Read assets from framework-aware sources instead of blindly walking random folders.
- Skip risky transformations unless the user explicitly opts in.

## Install

### macOS and Linux

```bash
curl -fsSL https://raw.githubusercontent.com/temoorx/asset-squeeze/main/install.sh | sh
```

### Windows PowerShell

```powershell
irm https://raw.githubusercontent.com/temoorx/asset-squeeze/main/install.ps1 | iex
```

The installer downloads the latest release for your platform, verifies it against `SHA256SUMS`, installs the binary into `~/.asset-squeeze/bin`, and bundles JPEG support so you do not need to install `jpegtran` separately.

Prebuilt archives are published for macOS Apple Silicon, Windows x64, and Linux x64.

## Quick Start

From your Flutter or React Native project root:

```bash
asset-squeeze doctor
asset-squeeze optimize --dry-run
asset-squeeze optimize
```

`--dry-run` previews changes without writing files. The real optimize command replaces files in place only when the optimized file is smaller.

To update later:

```bash
asset-squeeze update
```

## Commands

### Check The Project

```bash
asset-squeeze doctor
```

Shows the detected framework, discovered image assets, and available optimization backends.

### Choose A Framework

Auto detection is the default:

```bash
asset-squeeze optimize
```

You can force a framework:

```bash
asset-squeeze optimize --framework flutter
asset-squeeze optimize --framework react-native
```

### Preview Optimization

```bash
asset-squeeze optimize --dry-run
```

### Optimize Assets

```bash
asset-squeeze optimize
```

### Optimize One Format

```bash
asset-squeeze optimize --format png
asset-squeeze optimize --format jpeg
asset-squeeze optimize --format svg
asset-squeeze optimize --format webp
```

Repeat `--format` to include multiple formats:

```bash
asset-squeeze optimize --format png --format jpeg
```

### Use A Different Project Path

```bash
asset-squeeze optimize --project /path/to/mobile_app
```

### CI Check

```bash
asset-squeeze optimize --check
```

This exits with a non-zero status if any asset can still be optimized.

### Verbose Output

```bash
asset-squeeze optimize --verbose
```

By default, unchanged files are hidden so the output stays readable. Use `--verbose` to print them too.

### Update The CLI

```bash
asset-squeeze update
```

This downloads the latest GitHub release through the official installer, verifies checksums, and replaces the binary in `~/.asset-squeeze/bin`.

To preview the updater command:

```bash
asset-squeeze update --dry-run
```

## Supported Formats

| Format | Status | Notes |
| --- | --- | --- |
| PNG/APNG | Supported | Embedded `oxipng` lossless optimizer. |
| JPEG/JPG | Supported | Uses bundled `jpegtran`, or `jpegtran` from `PATH`. |
| SVG | Supported | Embedded conservative optimizer. |
| WebP | Supported | Embedded RIFF metadata optimizer. No re-encoding. |
| GIF | Skipped | Planned, pending a clean licensing/backend choice. |
| BMP/WBMP | Skipped | Meaningful savings usually require conversion, which is not a default goal. |

## Metadata

Default:

```bash
asset-squeeze optimize --strip safe
```

Other options:

```bash
asset-squeeze optimize --strip none
asset-squeeze optimize --strip all
```

For JPEG and WebP, `--strip safe` removes EXIF/XMP metadata when supported. `--strip all` may also remove WebP ICC color profiles. These operations do not lower encoded image quality, but metadata and color-profile removal may matter for some workflows.

WebP support is intentionally conservative: `asset-squeeze` edits the WebP RIFF container, removes selected metadata chunks, updates `VP8X` feature flags, and leaves image/animation payload chunks byte-for-byte untouched.

## SVG Safety

SVG optimization is intentionally conservative. It removes XML comments when metadata stripping is enabled, collapses whitespace between tags, and validates the original and optimized SVG as XML before replacing the file.

To avoid changing rendering semantics, the SVG backend currently skips files containing:

- `script`
- `style`
- `text`
- `tspan`
- `CDATA`
- `DOCTYPE`
- `foreignObject`
- XML stylesheets
- `xml:space`

## Flutter Asset Resolution

`asset-squeeze` reads `flutter.assets` from `pubspec.yaml`.

It supports:

```yaml
flutter:
  assets:
    - assets/images/
    - assets/logo.png
    - path: assets/flavored/logo.png
      flavors:
        - free
```

It also resolves Flutter image density variants such as:

```text
assets/icon.png
assets/2.0x/icon.png
assets/3.0x/icon.png
```

## React Native Asset Resolution

`asset-squeeze` scans JavaScript and TypeScript source files for static local asset references:

```tsx
import logo from "./assets/logo.png";
import icon from "./assets/icon.svg";

const hero = require("./assets/hero.jpg");
```

It also resolves React Native sibling variants such as:

```text
assets/logo.png
assets/logo@2x.png
assets/logo@3x.png
assets/logo.ios.png
assets/logo.android.png
```

For safety and predictability, React Native support currently skips:

- `node_modules`
- `ios`
- `android`
- `build`
- `dist`
- dynamic requires such as `require("./assets/" + name + ".png")`
- aliased imports such as `@assets/logo.png`
- remote URI images

React Native's own image system requires static image names for bundler resolution, so this matches the asset pattern the bundler can reliably see.

## Development

```bash
cargo test
cargo run -- doctor --project /path/to/flutter_app
cargo run -- optimize --dry-run --project /path/to/flutter_app
cargo run -- doctor --framework react-native --project /path/to/react_native_app
```

Run all release checks:

```bash
scripts/release-check.sh
```

The script runs formatting, tests, Clippy with warnings denied, a release build, and CLI smoke checks.

## Release

Create and push a version tag:

```bash
git tag v0.2.0
git push origin v0.2.0
```

GitHub Actions builds release archives for macOS, Linux, and Windows, then publishes them to GitHub Releases.

Release archives are named:

```text
asset-squeeze-macos-aarch64.tar.gz
asset-squeeze-linux-x86_64.tar.gz
asset-squeeze-windows-x86_64.zip
```

Each archive includes:

```text
asset-squeeze
vendor/bin/<platform>/jpegtran
README.md
LICENSE
THIRD_PARTY_NOTICES.md
```

## License

MIT. See `LICENSE`.

When distributing release archives with bundled `jpegtran`, include the libjpeg-turbo notices listed in `THIRD_PARTY_NOTICES.md`.
