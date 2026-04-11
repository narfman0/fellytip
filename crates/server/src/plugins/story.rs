//! Story plugin: collects WriteStoryEvent messages and appends them to
//! the StoryLog resource. A flush checkpoint is logged every 300 world-sim
//! ticks (≈5 minutes at 1 Hz); actual SQLite writes land in a later step.

use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use fellytip_shared::world::story::{StoryLog, WriteStoryEvent};

use crate::plugins::world_sim::{WorldSimSchedule, WorldSimTick};

pub struct StoryPlugin;

/// How many world-sim ticks between SQLite flushes.
const FLUSH_INTERVAL_TICKS: u64 = 300; // 5 min at 1 Hz

impl Plugin for StoryPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StoryLog>();
        app.add_message::<WriteStoryEvent>();
        app.add_systems(Update, collect_story_events);
        app.add_systems(WorldSimSchedule, flush_story_log);
    }
}

/// Each frame: drain WriteStoryEvent queue → append to StoryLog.
fn collect_story_events(
    mut reader: MessageReader<WriteStoryEvent>,
    mut log: ResMut<StoryLog>,
) {
    for WriteStoryEvent(ev) in reader.read() {
        tracing::info!(kind = ?ev.kind, tick = ev.tick, "Story event recorded");
        log.push(ev.clone());
    }
}

/// Each world-sim tick: log flush checkpoint when interval elapses.
fn flush_story_log(log: Res<StoryLog>, tick: Res<WorldSimTick>) {
    if tick.0 > 0 && tick.0.is_multiple_of(FLUSH_INTERVAL_TICKS) {
        tracing::info!(
            count = log.events.len(),
            tick = tick.0,
            "Story log flush checkpoint (SQLite write pending)"
        );
    }
}
