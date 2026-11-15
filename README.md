# yubi

Gesture engine and settings app for Apple Magic Trackpad 2 on Windows, written in Rust + egui.

It talks to the driver fork [uchmk/MagicTrackpad2ForWindows](https://github.com/uchmk/MagicTrackpad2ForWindows)
only through public interfaces (Raw Input, the haptic IOCTL / named pipe), and contains no code from it.

Status: skeleton.
