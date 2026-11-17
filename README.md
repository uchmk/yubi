# yubi

yubi adds gestures for the Apple Magic Trackpad on Windows. It is a small resident app (system tray) plus a settings
window, written in Rust with egui. "Magic Trackpad" is used here only to say which hardware it works with.

## Features

- Multi-finger gestures (drag, swipe, pinch and more) that you can assign to actions.
- Corner, edge, press and hold gestures, and a ring menu.
- Per-app profiles, so the same gesture can do different things in different apps.
- Haptic feedback through the driver, with sensitivity tuning and a practice page to check what is recognised.
- Pause and resume from a shortcut or the tray icon.

## Status

Early development (version 0.1). Settings and behaviour may still change.

## Requirements

- Windows 10 or 11.
- The Magic Trackpad driver for Windows, which is a separate project:
  [vitoplantamura/MagicTrackpad2ForWindows](https://github.com/vitoplantamura/MagicTrackpad2ForWindows) (upstream) or the
  fork [uchmk/MagicTrackpad2ForWindows](https://github.com/uchmk/MagicTrackpad2ForWindows). Install the driver from
  there. yubi does not bundle it.

## Install and build

```
cargo install --path . --locked
```

The executable is not code-signed, so Windows SmartScreen may show a warning the first time you run it.

## Relationship to the driver

The driver is a separate project under GPL-2.0. yubi contains no code from it. It only uses the driver's public
interfaces: Raw Input, the driver's device and pipe names, and its registry service keys.

## License

Licensed under either of

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT license](LICENSE-MIT)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as
defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

## Third-party notices

The licences of the libraries and fonts used in the Windows build are in
[THIRD-PARTY-LICENSES.txt](THIRD-PARTY-LICENSES.txt), including the bundled fonts under the SIL Open Font License 1.1
and the Ubuntu Font Licence 1.0. The same text is shown in the app (General page, "ライセンス").

## Trademarks

"Apple" and "Magic Trackpad" are trademarks of Apple Inc. yubi is not affiliated with or endorsed by Apple.
