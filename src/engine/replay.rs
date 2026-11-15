//! Recording and replay of frames as JSON lines (`yubi --record <file>`).
//!
//! One line per frame: `{"t_us":1000,"button":false,"contacts":[{"id":1,"x_mm":40.0,"y_mm":50.0}]}`.
//! Contact fields other than `id` are optional when reading (see `input::Contact`).
#![allow(
    dead_code,
    reason = "`parse` and `run` are test tooling; only `write_frame` is used by the app"
)]

use std::io::Write;

use super::Frame;
use super::recognizer::{Action, Recognizer};
use crate::config::Config;

/// Write one frame as a line.
pub fn write_frame(out: &mut impl Write, frame: &Frame) -> std::io::Result<()> {
    serde_json::to_writer(&mut *out, frame).map_err(std::io::Error::other)?;
    out.write_all(b"\n")
}

/// Read frames from JSON lines. Blank lines and `#` comment lines are skipped.
pub fn parse(text: &str) -> Result<Vec<Frame>, String> {
    let mut frames = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Frame = serde_json::from_str(line).map_err(|e| format!("line {}: {e}", i + 1))?;
        if frames.last().is_some_and(|p: &Frame| p.t_us > f.t_us) {
            return Err(format!("line {}: time goes backwards", i + 1));
        }
        frames.push(f);
    }
    Ok(frames)
}

/// Feed `frames` to a fresh recognizer, firing its timers between frames the way the engine
/// thread does. Returns each action with the time it was produced.
pub fn run(frames: &[Frame], cfg: &Config) -> Vec<(u64, Action)> {
    let mut r = Recognizer::default();
    let mut out = Vec::new();
    for f in frames {
        fire_timers_until(&mut r, f.t_us, &mut out);
        out.extend(r.step(f, cfg).into_iter().map(|a| (f.t_us, a)));
    }
    if let Some(last) = frames.last() {
        // Let any remaining timer run out (bounded, a timer fires at most once).
        fire_timers_until(&mut r, last.t_us + 10_000_000, &mut out);
    }
    out
}

fn fire_timers_until(r: &mut Recognizer, until: u64, out: &mut Vec<(u64, Action)>) {
    // Each iteration consumes a deadline; the cap only guards against a logic error.
    for _ in 0..16 {
        let Some(due) = r.next_deadline().filter(|&d| d <= until) else {
            return;
        };
        out.extend(r.tick(due).into_iter().map(|a| (due, a)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Contact;

    #[test]
    fn round_trip() {
        let f = Frame {
            t_us: 1234,
            button: true,
            contacts: vec![Contact {
                id: 3,
                x: 0.1,
                y: 0.2,
                x_mm: 16.0,
                y_mm: 23.0,
                tip: true,
                confidence: true,
                pressure: Some(0.5),
            }],
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &f).unwrap();
        write_frame(&mut buf, &f).unwrap();
        let back = parse(std::str::from_utf8(&buf).unwrap()).unwrap();
        assert_eq!(back, vec![f.clone(), f]);
    }

    #[test]
    fn minimal_contact_and_comments() {
        let text =
            "# comment\n\n{\"t_us\":5,\"contacts\":[{\"id\":1,\"x_mm\":2.0,\"y_mm\":3.0}]}\n";
        let f = parse(text).unwrap();
        assert_eq!(f.len(), 1);
        assert!(f[0].contacts[0].tip && f[0].contacts[0].confidence);
        assert!(!f[0].button);
    }

    #[test]
    fn rejects_bad_and_unordered_lines() {
        assert!(parse("{nope").is_err());
        let text = "{\"t_us\":9,\"contacts\":[]}\n{\"t_us\":3,\"contacts\":[]}\n";
        assert!(parse(text).unwrap_err().contains("backwards"));
    }
}
