//! Story plugin: collects WriteStoryEvent messages and appends them to
//! the StoryLog resource.  Events are flushed to SQLite every
//! `FLUSH_INTERVAL_TICKS` world-sim ticks (≈5 minutes at 1 Hz).
//! Significant events are also broadcast to all connected clients as `StoryMsg`.

use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use fellytip_shared::{
    protocol::{StoryMsg, WorldStateChannel},
    world::story::{StoryEvent, StoryEventKind, StoryLog, WriteStoryEvent},
};
use lightyear::prelude::{server::Server, NetworkTarget, ServerMultiMessageSender};

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

/// Format a `StoryEventKind` into a human-readable string for client display.
fn format_story_event(ev: &StoryEvent) -> String {
    let day = ev.world_day;
    match &ev.kind {
        StoryEventKind::FactionWarDeclared { attacker, defender } =>
            format!("Day {day}: {} declares war on {}!", attacker.0, defender.0),
        StoryEventKind::SettlementRazed { by } =>
            format!("Day {day}: {} razes a settlement!", by.0),
        StoryEventKind::SettlementFounded { faction, name } =>
            format!("Day {day}: {} founds {}!", faction.0, name),
        StoryEventKind::EcologyCollapse { species, region } =>
            format!("Day {day}: {} collapse in {}!", species.0, region.0),
        StoryEventKind::AllianceFormed { a, b } =>
            format!("Day {day}: {} and {} form an alliance!", a.0, b.0),
        StoryEventKind::PlayerKilledNamed { .. } =>
            format!("Day {day}: A named foe has fallen!"),
        StoryEventKind::PartyDefeatedBoss { .. } =>
            format!("Day {day}: A boss has been slain!"),
        StoryEventKind::QuestCompleted { quest_id } =>
            format!("Day {day}: Quest '{quest_id}' completed!"),
        StoryEventKind::PlayerJoinedFaction { faction, .. } =>
            format!("Day {day}: A hero joins the {}!", faction.0),
        StoryEventKind::NpcDefected { from, to, .. } =>
            format!("Day {day}: A soldier defects from {} to {}!", from.0, to.0),
        StoryEventKind::MonsterMigrated { species, from, to } =>
            format!("Day {day}: {} migrate from {} to {}!", species.0, from.0, to.0),
    }
}

/// Each frame: drain WriteStoryEvent queue → append to StoryLog and broadcast to clients.
fn collect_story_events(
    mut reader: MessageReader<WriteStoryEvent>,
    mut log: ResMut<StoryLog>,
    mut msg_sender: ServerMultiMessageSender,
    server: Option<Single<&Server>>,
) {
    for WriteStoryEvent(ev) in reader.read() {
        tracing::info!(kind = ?ev.kind, tick = ev.tick, "Story event recorded");
        // Broadcast to all connected clients.
        if let Some(ref s) = server {
            let text = format_story_event(ev);
            let msg = StoryMsg { text };
            let _ = msg_sender.send::<StoryMsg, WorldStateChannel>(&msg, s, &NetworkTarget::All);
        }
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

/// Flush all pending story events to SQLite immediately (blocking).
///
/// Called by the timed flush system and by the graceful shutdown hook so that
/// both code paths share the same SQL logic.
pub fn flush_story_now(log: &mut StoryLog, db: &Db) {
    let events: Vec<StoryEvent> = log.events.drain(..).collect();
    if events.is_empty() {
        return;
    }

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
        tracing::info!(flushed = ok, total = events.len(), "Story log flushed to SQLite");
    });
}

/// Every `FLUSH_INTERVAL_TICKS` world-sim ticks: write accumulated events to SQLite.
fn flush_story_log(mut log: ResMut<StoryLog>, tick: Res<WorldSimTick>, db: Res<Db>) {
    if tick.0 == 0 || !tick.0.is_multiple_of(FLUSH_INTERVAL_TICKS) {
        return;
    }
    flush_story_now(&mut log, &db);
}
