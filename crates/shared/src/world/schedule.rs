//! NPC daily schedule types — what an NPC does at each hour of the world day.

use serde::{Deserialize, Serialize};

/// A complete NPC schedule made of sequential phases.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NpcSchedule {
    pub phases: Vec<SchedulePhase>,
}

/// One time-slot in an NPC's day.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchedulePhase {
    pub start_hour: f32,
    pub end_hour: f32,
    pub activity: NpcActivity,
    pub location: LocationHint,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum NpcActivity {
    Sleep,
    Work,
    Patrol,
    Trade,
    Guard,
    Socialize,
}

/// Rough location hint used by movement AI to pick a destination.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum LocationHint {
    Home,
    Workplace,
    Market,
    GuardPost,
    Tavern,
}
