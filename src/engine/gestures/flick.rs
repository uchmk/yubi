//! Flick detection shared by the window-move snap and the title-bar flick of a drag: the speed of
//! the centroid over the last `flick_window_us` before the lift decides.

use std::collections::VecDeque;

use super::super::types::{Ctx, Point, SnapDir, Thresholds};

/// Samples older than this are dropped (the flick window is far shorter).
const KEEP_US: u64 = 200_000;

/// Recent (time, centroid) samples taken with the full finger count.
#[derive(Debug, Default)]
pub struct FlickTracker {
    samples: VecDeque<(u64, Point)>,
}

impl FlickTracker {
    /// Remember the centroid of a real frame (not of a timer tick).
    pub fn sample(&mut self, ctx: &Ctx, c: Point) {
        if !ctx.from_frame {
            return;
        }
        let now = ctx.now;
        self.samples.push_back((now, c));
        while self.samples.front().is_some_and(|(t, _)| t + KEEP_US < now) {
            self.samples.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// Direction of the flick that ended at `lift_us`, from the speed over the last
    /// `flick_window_us` before the lift.
    pub fn dir(&self, th: &Thresholds, lift_us: u64) -> Option<SnapDir> {
        let &(t_last, p_last) = self.samples.back()?;
        if lift_us.saturating_sub(t_last) > th.flick_stale_us {
            return None;
        }
        let &(t0, p0) = self
            .samples
            .iter()
            .find(|(t, _)| t + th.flick_window_us >= t_last)?;
        let span = t_last - t0;
        if span < th.flick_min_span_us {
            return None;
        }
        let secs = span as f32 / 1e6;
        let (vx, vy) = ((p_last.0 - p0.0) / secs, (p_last.1 - p0.1) / secs);
        let (ax, ay) = (vx.abs(), vy.abs());
        if ax.hypot(ay) < th.flick_speed_mm_s {
            return None;
        }
        let ratio = th.flick_axis_ratio;
        if ax >= ratio * ay {
            Some(if vx > 0.0 {
                SnapDir::Right
            } else {
                SnapDir::Left
            })
        } else if ay >= ratio * ax {
            Some(if vy > 0.0 { SnapDir::Down } else { SnapDir::Up })
        } else {
            None
        }
    }
}
