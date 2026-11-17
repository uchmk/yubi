# Security policy

## Reporting a vulnerability

Please report a vulnerability privately, not in a public issue:

- Use **Security → Report a vulnerability** on this repository (GitHub private vulnerability reporting).

Include what you found, how to reproduce it, and the version or commit it affects. I will reply as soon as I can and
will tell you when a fix is released.

## Supported versions

Only the latest release is supported. Fixes land on `main` and ship in the next release.

## Scope

yubi reads touch input from the Magic Trackpad driver and sends keyboard and mouse input to other apps. Reports about
input reaching an app it should not, or about yubi's settings files, are in scope. Problems in the driver itself belong to
the driver's project ([vitoplantamura/MagicTrackpad2ForWindows](https://github.com/vitoplantamura/MagicTrackpad2ForWindows)).
