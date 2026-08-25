# Third-Party Notices

This file tracks notices required when shipping bundled optimizer backends.

## oxipng

Used in-process for PNG/APNG optimization.

License: MIT

Source: https://github.com/oxipng/oxipng

## libjpeg-turbo / jpegtran / cjpeg / djpeg

Used as the bundled or PATH-provided JPEG lossless and lossy optimization backends.

Licenses: IJG License, BSD-3-Clause, zlib

Source: https://github.com/libjpeg-turbo/libjpeg-turbo

Packaging note: release archives that include `jpegtran` should also include
libjpeg-turbo's upstream `LICENSE.md`, `README.ijg`, and any other license files
distributed with the exact binary build.

## libwebp / cwebp / dwebp

Used as the bundled or PATH-provided static WebP lossy encoding backend.

License: BSD-3-Clause

Source: https://chromium.googlesource.com/webm/libwebp/

Packaging note: release archives include `LICENSE-libwebp.txt`, copied from
libwebp 1.6.0's official source release.

## serde_yaml_ng

Used for reading `pubspec.yaml`.

License: MIT or Apache-2.0

Source: https://github.com/serde-yaml-ng/serde-yaml-ng

## roxmltree

Used for validating SVG XML before and after conservative SVG optimization.

License: MIT or Apache-2.0

Source: https://github.com/RazrFalcon/roxmltree
