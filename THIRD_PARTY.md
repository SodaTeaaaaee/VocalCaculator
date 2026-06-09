# Third-Party Licenses and Attribution

## Slint

- **Website**: https://slint.dev
- **License**: Slint Community License (free for desktop/mobile/web with attribution)
- **Used for**: Retained-mode GUI framework (Windows, Android, Linux, macOS)

## kira

- **Repository**: https://github.com/tesselode/kira
- **License**: MIT
- **Version**: 0.12.1
- **Used for**: Audio playback engine (static sound data, handles, tweened stop)

## rust_decimal

- **Repository**: https://github.com/paupino/rust-decimal
- **License**: MIT
- **Version**: 1.42.0
- **Used for**: Arbitrary-precision decimal arithmetic (no floating-point rounding in calculator logic)

## serde

- **License**: MIT OR Apache-2.0
- **Used for**: Configuration serialization

## toml

- **License**: MIT OR Apache-2.0
- **Used for**: Configuration file format

## sysdirs

- **License**: MIT OR Apache-2.0
- **Used for**: Platform-specific config/data directory resolution

## Voice Assets

The voice WAV files in `resource/Vocal/Normal/` and `resource/Vocal/Broken/`
are bundled with this project. These assets are compiled into the binary at
build time via `include_bytes!()`.

## External Music Assets (Optional)

The AR7778-digitized-MIDI repository (https://github.com/evnchn-AR7778/AR7778-digitized-MIDI)
is **not** included in this build. It has no confirmed license and is not
redistributable by default. Music mode support is implemented but disabled
unless a user-supplied asset pack is installed locally.
