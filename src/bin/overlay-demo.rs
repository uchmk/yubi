//! Manual check of the ring overlay: opens it at the cursor (or `--at X Y`), cycles the selection
//! every 300 ms, commits, closes, and reports whether the foreground window changed.
#![cfg_attr(not(debug_assertions), allow(dead_code))]

#[path = "../overlay/mod.rs"]
mod overlay;

use std::time::Duration;

use overlay::{Overlay, OverlayCmd, RingSlot};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow};

fn main() {
    // Physical pixels everywhere, like the overlay (and unlike a DPI-unaware process).
    // SAFETY: process-wide setting, done before any window exists.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let args: Vec<String> = std::env::args().collect();
    let mut at = POINT::default();
    // SAFETY: plain query.
    unsafe {
        let _ = GetCursorPos(&mut at);
    }
    if let Some(p) = args.iter().position(|a| a == "--at")
        && let (Some(x), Some(y)) = (args.get(p + 1), args.get(p + 2))
    {
        at = POINT {
            x: x.parse().unwrap_or(at.x),
            y: y.parse().unwrap_or(at.y),
        };
    }
    let hold = args.iter().any(|a| a == "--hold");

    let names = [
        ("前のタブ", '\u{E72B}'),
        ("音量を上げる", '\u{E995}'),
        ("次のタブ", '\u{E72A}'),
        ("最小化", '\u{E921}'),
        ("閉じる", '\u{E711}'),
        ("スクリーンショット", '\u{E722}'),
        ("最大化", '\u{E922}'),
        ("デスクトップ", '\u{E80F}'),
    ];
    let items: Vec<RingSlot> = names
        .iter()
        .map(|(l, c)| RingSlot {
            label: (*l).into(),
            icon: Some(*c),
        })
        .collect();

    // SAFETY: plain query.
    let before = unsafe { GetForegroundWindow() };
    let mut ov = Overlay::start().expect("overlay");
    let generation = 1;
    ov.send(OverlayCmd::Open {
        anchor_px: (at.x, at.y),
        items,
        generation,
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(400));
    for i in 0..8usize {
        ov.send(OverlayCmd::Select {
            index: Some(i),
            generation,
        })
        .unwrap();
        println!("selected {i} state={:?}", ov.state());
        std::thread::sleep(Duration::from_millis(300));
        if hold && i == 2 {
            // Leave the ring up on slot 2 for an external screenshot.
            std::thread::sleep(Duration::from_millis(4000));
        }
    }
    ov.send(OverlayCmd::Close {
        committed: Some(7),
        generation,
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(400));
    // SAFETY: plain query.
    let after = unsafe { GetForegroundWindow() };
    println!(
        "foreground unchanged: {} state={:?}",
        before == after,
        ov.state()
    );
    ov.stop();
}
