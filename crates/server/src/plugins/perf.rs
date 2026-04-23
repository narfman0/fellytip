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

/// System set that brackets all AI / world-sim systems so timing bookends
/// can use `.before()` / `.after()` on a stable anchor.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PerfRecordStart;

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PerfRecordEnd;

pub struct PerfPlugin;

impl Plugin for PerfPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TickStartTime>()
            .init_resource::<TickTimings>()
            .add_systems(
                WorldSimSchedule,
                record_tick_start.in_set(PerfRecordStart),
            )
            .add_systems(
                WorldSimSchedule,
                record_tick_end.in_set(PerfRecordEnd).after(PerfRecordStart),
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
}
