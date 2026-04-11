//! Shared Bevy resources synced or derived from server world state.

use bevy::prelude::*;

/// In-game world clock — advances one hour per world-sim tick.
#[derive(Resource, Default, Clone, Debug)]
pub struct WorldClock {
    /// Total elapsed world-sim ticks.
    pub tick: u64,
    /// Current world day (resets each 24 in-game hours).
    pub day: u32,
    /// Current hour within the day [0, 24).
    pub hour: f32,
}

impl WorldClock {
    /// Advance by one world-sim tick (= 1 s real time = 1 world minute at 60x speed).
    pub fn advance(&mut self) {
        self.tick += 1;
        self.hour = (self.hour + 1.0 / 60.0).rem_euclid(24.0);
        if self.tick.is_multiple_of(24 * 60) {
            self.day += 1;
        }
    }
}
