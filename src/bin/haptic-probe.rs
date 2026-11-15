//! Dev tool: check and exercise trackpad haptics on real hardware.
//!
//! ```text
//! cargo run --bin haptic-probe -- status
//! cargo run --bin haptic-probe -- click weak|medium|strong|0x060617
//! cargo run --bin haptic-probe -- pulse 3 [interval_ms]
//! cargo run --bin haptic-probe -- battery
//! cargo run --bin haptic-probe -- perm      (read-only: show AllowNonAdminAccess)
//! ```
//! It never changes permissions; that is `permission::enable_non_admin_access` in the app.

#[path = "../haptic/mod.rs"]
mod haptic;

use std::time::Duration;

use haptic::{HapticStatus, Haptics, Strength};

fn usage() -> ! {
    eprintln!(
        "usage: haptic-probe status | click [weak|medium|strong|0xHEX] | pulse N [interval_ms] | battery | perm"
    );
    std::process::exit(2);
}

fn describe(s: &HapticStatus) -> String {
    match s {
        HapticStatus::Available => "Available (haptics usable)".into(),
        HapticStatus::AccessDenied => {
            "AccessDenied (endpoint exists, but this user may not open it; enable AllowNonAdminAccess or run elevated)".into()
        }
        HapticStatus::NotFound => "NotFound (driver endpoint missing: driver not installed or trackpad not connected)".into(),
        HapticStatus::Error(m) => format!("Error: {m}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        usage()
    };

    if cmd == "perm" {
        let st = haptic::permission::read_state();
        println!(
            "AllowNonAdminAccess  usb={:?}  bluetooth={:?}  enabled={}",
            st.usb,
            st.bluetooth,
            st.is_enabled()
        );
        println!(
            "trackpad instances: {:?}",
            haptic::permission::find_trackpad_instances()
        );
        return;
    }

    let h = Haptics::new();
    if !h.wait_ready(Duration::from_secs(5)) {
        eprintln!("probe did not finish within 5 s");
        std::process::exit(1);
    }
    let status = h.status();
    println!("status: {}", describe(&status));
    println!("battery: {:?}", h.battery());

    match cmd {
        "status" | "battery" => {}
        "click" => {
            let s = args
                .get(1)
                .map_or(Some(Strength::Medium), |a| Strength::parse(a));
            let Some(s) = s else { usage() };
            println!("click {s:?} (value 0x{:06X})", s.value());
            h.actuate(s);
            settle(&h, 1);
        }
        "pulse" => {
            let n: u32 = args
                .get(1)
                .and_then(|a| a.parse().ok())
                .unwrap_or_else(|| usage());
            let iv: u32 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(80);
            println!("pulse x{n} every {iv} ms");
            h.pulse(n, iv);
            settle(&h, n);
        }
        _ => usage(),
    }
    println!("status after: {}", describe(&h.status()));
}

/// Give the worker time to send the queued clicks before the process exits.
fn settle(_h: &Haptics, clicks: u32) {
    std::thread::sleep(Duration::from_millis(100 + 120 * clicks.min(16) as u64));
}
