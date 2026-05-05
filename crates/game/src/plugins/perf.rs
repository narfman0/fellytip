//! Adaptive-AI performance plugin: tick-duration sampling and throttle derivation.
//!
//! Bookend systems on `WorldSimSchedule` record how long each tick takes in
//! `TickTimings`, a rolling 60-entry ring buffer. Downstream systems (see
//! `AdaptiveScheduler`) read P95 latency to decide whether to throttle AI work.

use std::collections::VecDeque;
use std::time::Instant;

use bevy::prelude::*;

use crate::plugins::world_sim::WorldSimSchedule;

/// Target tick budget for the 1 Hz world-sim schedule (server default).
pub const AI_TICK_BUDGET_MS: f32 = 50.0;

pub use fellytip_shared::bridge::{ClientFrameTimings, HOST_FRAME_FLOOR_SECS};

#[derive(Resource)]
pub struct TickStartTime(pub Instant);

impl Default for TickStartTime {
    fn default() -> Self {
        Self(Instant::now())
    }
}

/// Rolling-window timings for `WorldSimSchedule` ticks.
#[derive(Resource, Default)]
pub struct TickTimings {
    samples: VecDeque<f32>,
    pub consecutive_over: u32,
    pub consecutive_under: u32,
}

impl TickTimings {
    pub fn push(&mut self, ms: f32) {
        if self.samples.len() >= 60 {
            self.samples.pop_front();
        }
        self.samples.push_back(ms);
        if ms > AI_TICK_BUDGET_MS {
            self.consecutive_over += 1;
            self.consecutive_under = 0;
        } else {
            self.consecutive_under += 1;
            self.consecutive_over = 0;
        }
    }

    pub fn p95_ms(&self) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let mut sorted: Vec<f32> = self.samples.iter().copied().collect();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        sorted[(sorted.len() as f32 * 0.95) as usize]
    }

    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }
}

/// Server-side adaptive AI throttle level.
///
/// `Ord` ranks from least-throttled (`Full`) to most-throttled (`Suspended`)
/// so `max(a, b)` escalates in the "more throttled" direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum ThrottleLevel {
    #[default]
    Full,
    Reduced,
    Minimal,
    Suspended,
}

impl ThrottleLevel {
    /// Drop exactly one step toward `Full` (used for hysteresis deescalation).
    pub fn deescalate_one(self) -> Self {
        match self {
            ThrottleLevel::Suspended => ThrottleLevel::Minimal,
            ThrottleLevel::Minimal => ThrottleLevel::Reduced,
            ThrottleLevel::Reduced => ThrottleLevel::Full,
            ThrottleLevel::Full => ThrottleLevel::Full,
        }
    }
}

/// Derived throttle level exposed to AI / pathfinding systems.
#[derive(Resource, Default)]
pub struct AdaptiveScheduler {
    pub level: ThrottleLevel,
}

/// Number of consecutive under-budget ticks required before relaxing one step.
const DEESCALATE_AFTER: u32 = 10;

/// Map a P95 latency (ms) to the corresponding throttle level.
fn derive_level(p95_ms: f32) -> ThrottleLevel {
    if p95_ms > AI_TICK_BUDGET_MS * 1.5 {
        ThrottleLevel::Suspended
    } else if p95_ms > AI_TICK_BUDGET_MS {
        ThrottleLevel::Minimal
    } else if p95_ms > AI_TICK_BUDGET_MS * 0.75 {
        ThrottleLevel::Reduced
    } else {
        ThrottleLevel::Full
    }
}

/// System set that brackets all AI / world-sim systems so timing bookends
/// can use `.before()` / `.after()` on a stable anchor.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PerfRecordStart;

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PerfRecordEnd;

/// Runs after `PerfRecordStart` and before all AI systems so downstream
/// systems see an up-to-date throttle level.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PerfThrottleUpdate;

pub struct PerfPlugin;

impl Plugin for PerfPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TickStartTime>()
            .init_resource::<TickTimings>()
            .init_resource::<AdaptiveScheduler>()
            .init_resource::<ClientFrameTimings>()
            .add_systems(
                WorldSimSchedule,
                record_tick_start.in_set(PerfRecordStart),
            )
            .add_systems(
                WorldSimSchedule,
                update_throttle_level
                    .in_set(PerfThrottleUpdate)
                    .after(PerfRecordStart),
            )
            .add_systems(
                WorldSimSchedule,
                record_tick_end
                    .in_set(PerfRecordEnd)
                    .after(PerfThrottleUpdate),
            );
    }
}

fn record_tick_start(mut start: ResMut<TickStartTime>) {
    start.0 = Instant::now();
}

fn record_tick_end(start: Res<TickStartTime>, mut timings: ResMut<TickTimings>) {
    let ms = start.0.elapsed().as_secs_f32() * 1000.0;
    timings.push(ms);
}

fn update_throttle_level(
    timings: Res<TickTimings>,
    frame_timings: Res<ClientFrameTimings>,
    mut scheduler: ResMut<AdaptiveScheduler>,
) {
    let mut derived = derive_level(timings.p95_ms());
    if frame_timings.under_pressure {
        derived = bump_one(derived);
    }
    scheduler.level = if derived > scheduler.level {
        derived
    } else if derived < scheduler.level
        && timings.consecutive_under >= DEESCALATE_AFTER
    {
        scheduler.level.deescalate_one()
    } else {
        scheduler.level
    };
}

/// Escalate exactly one step (Full→Reduced→Minimal→Suspended).
fn bump_one(level: ThrottleLevel) -> ThrottleLevel {
    match level {
        ThrottleLevel::Full => ThrottleLevel::Reduced,
        ThrottleLevel::Reduced => ThrottleLevel::Minimal,
        ThrottleLevel::Minimal => ThrottleLevel::Suspended,
        ThrottleLevel::Suspended => ThrottleLevel::Suspended,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_trims_to_60() {
        let mut t = TickTimings::default();
        for i in 0..120 {
            t.push(i as f32);
        }
        assert_eq!(t.sample_count(), 60);
    }

    #[test]
    fn counters_track_consecutive_runs() {
        let mut t = TickTimings::default();
        t.push(10.0);
        t.push(10.0);
        assert_eq!(t.consecutive_under, 2);
        assert_eq!(t.consecutive_over, 0);
        t.push(100.0);
        assert_eq!(t.consecutive_over, 1);
        assert_eq!(t.consecutive_under, 0);
    }

    #[test]
    fn p95_empty_is_zero() {
        let t = TickTimings::default();
        assert_eq!(t.p95_ms(), 0.0);
    }

    #[test]
    fn p95_picks_upper_quantile() {
        let mut t = TickTimings::default();
        for i in 1..=20 {
            t.push(i as f32);
        }
        // 20 samples: index = (20 * 0.95) as usize = 19 → value 20.0
        assert_eq!(t.p95_ms(), 20.0);
    }

    #[test]
    fn derive_level_thresholds() {
        assert_eq!(derive_level(0.0), ThrottleLevel::Full);
        assert_eq!(derive_level(AI_TICK_BUDGET_MS * 0.5), ThrottleLevel::Full);
        assert_eq!(derive_level(AI_TICK_BUDGET_MS * 0.8), ThrottleLevel::Reduced);
        assert_eq!(derive_level(AI_TICK_BUDGET_MS * 1.2), ThrottleLevel::Minimal);
        assert_eq!(derive_level(AI_TICK_BUDGET_MS * 2.0), ThrottleLevel::Suspended);
    }

    #[test]
    fn deescalate_one_steps_down() {
        assert_eq!(
            ThrottleLevel::Suspended.deescalate_one(),
            ThrottleLevel::Minimal
        );
        assert_eq!(ThrottleLevel::Minimal.deescalate_one(), ThrottleLevel::Reduced);
        assert_eq!(ThrottleLevel::Reduced.deescalate_one(), ThrottleLevel::Full);
        assert_eq!(ThrottleLevel::Full.deescalate_one(), ThrottleLevel::Full);
    }

    fn run_update(timings: &TickTimings, scheduler: &mut AdaptiveScheduler) {
        run_update_with_frame(timings, &ClientFrameTimings::default(), scheduler);
    }

    fn run_update_with_frame(
        timings: &TickTimings,
        frame: &ClientFrameTimings,
        scheduler: &mut AdaptiveScheduler,
    ) {
        let mut derived = derive_level(timings.p95_ms());
        if frame.under_pressure {
            derived = bump_one(derived);
        }
        scheduler.level = if derived > scheduler.level {
            derived
        } else if derived < scheduler.level && timings.consecutive_under >= DEESCALATE_AFTER {
            scheduler.level.deescalate_one()
        } else {
            scheduler.level
        };
    }

    #[test]
    fn escalates_immediately() {
        let mut timings = TickTimings::default();
        for _ in 0..5 {
            timings.push(AI_TICK_BUDGET_MS * 2.0);
        }
        let mut s = AdaptiveScheduler::default();
        run_update(&timings, &mut s);
        assert_eq!(s.level, ThrottleLevel::Suspended);
    }

    #[test]
    fn deescalates_only_after_hysteresis_window() {
        let mut timings = TickTimings::default();
        // Prime the scheduler into Suspended via one escalation.
        timings.push(AI_TICK_BUDGET_MS * 2.0);
        let mut s = AdaptiveScheduler::default();
        run_update(&timings, &mut s);
        assert_eq!(s.level, ThrottleLevel::Suspended);

        // Push enough under-budget samples to drive P95 below the derived threshold.
        // With a single high sample in the ring buffer, ~20 low samples are enough
        // to drop the 95th percentile below budget × 0.75.
        for _ in 0..9 {
            timings.push(1.0);
            run_update(&timings, &mut s);
        }
        // 9 consecutive under-budget ticks: still below hysteresis threshold.
        assert_eq!(s.level, ThrottleLevel::Suspended);

        // 10th under-budget tick — and enough prior low samples to push P95 low.
        for _ in 0..20 {
            timings.push(1.0);
        }
        run_update(&timings, &mut s);
        // Now we've hit both: P95 is below budget AND consecutive_under ≥ 10.
        // One step down (Suspended → Minimal).
        assert_eq!(s.level, ThrottleLevel::Minimal);
    }

    #[test]
    fn client_pressure_bumps_one_step() {
        let mut timings = TickTimings::default();
        timings.push(1.0);
        let mut frame = ClientFrameTimings::default();
        for _ in 0..10 {
            frame.push(0.1);
        }
        assert!(frame.under_pressure);

        let mut s = AdaptiveScheduler::default();
        run_update_with_frame(&timings, &frame, &mut s);
        // P95 is under budget (Full), but host is under pressure → bumped to Reduced.
        assert_eq!(s.level, ThrottleLevel::Reduced);
    }

    #[test]
    fn frame_timings_trim_and_detect() {
        let mut frame = ClientFrameTimings::default();
        for _ in 0..120 {
            frame.push(0.001);
        }
        assert_eq!(frame.sample_count(), 60);
        assert!(!frame.under_pressure);

        for _ in 0..60 {
            frame.push(0.1);
        }
        assert!(frame.under_pressure);
    }
}
