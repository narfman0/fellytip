//! Story plugin: collects WriteStoryEvent messages and appends them to
//! the StoryLog resource.  Events are flushed to SQLite every
//! `FLUSH_INTERVAL_TICKS` world-sim ticks (≈5 minutes at 1 Hz).

use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use fellytip_shared::world::story::{StoryEvent, StoryLog, WriteStoryEvent};

use crate::plugins::persistence::Db;
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

/// Serialize one story event into a row and insert it into SQLite.
async fn insert_story_event(
    pool: &sqlx::Pool<sqlx::Sqlite>,
    ev: &StoryEvent,
) -> Result<(), sqlx::Error> {
    let id        = ev.id.to_string();
    let tick      = ev.tick as i64;
    let world_day = ev.world_day as i64;
    let kind      = format!("{:?}", ev.kind);
    let parts: Vec<String> = ev.participants.iter().map(|p| p.0.to_string()).collect();
    let participants = serde_json::to_string(&parts).unwrap_or_default();
    let lore_tags = serde_json::to_string(&ev.lore_tags).unwrap_or_default();
    let loc_x = ev.location.map(|l| l.x);
    let loc_y = ev.location.map(|l| l.y);

    sqlx::query(
        "INSERT OR IGNORE INTO story_events \
         (id, tick, world_day, kind, participants, loc_x, loc_y, lore_tags) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(tick)
    .bind(world_day)
    .bind(kind)
    .bind(participants)
    .bind(loc_x)
    .bind(loc_y)
    .bind(lore_tags)
    .execute(pool)
    .await?;

    Ok(())
}

/// Every `FLUSH_INTERVAL_TICKS` world-sim ticks: write accumulated events to SQLite.
fn flush_story_log(
    mut log: ResMut<StoryLog>,
    tick: Res<WorldSimTick>,
    db: Res<Db>,
) {
    if tick.0 == 0 || !tick.0.is_multiple_of(FLUSH_INTERVAL_TICKS) {
        return;
    }

    let events: Vec<StoryEvent> = log.events.drain(..).collect();
    if events.is_empty() {
        return;
    }

    let flush_tick = tick.0;
    let pool = db.pool().clone();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime for story flush");

    rt.block_on(async move {
        let mut ok = 0usize;
        for ev in &events {
            match insert_story_event(&pool, ev).await {
                Ok(()) => ok += 1,
                Err(e) => tracing::warn!(event_id = %ev.id, "Story event flush failed: {e}"),
            }
        }
        tracing::info!(flushed = ok, total = events.len(), tick = flush_tick, "Story log flushed to SQLite");
    });
}
