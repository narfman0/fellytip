//! Story event log — the append-only world narrative.
//!
//! Systems emit `WriteStoryEvent` Bevy events; the `story_writer` system
//! appends them to the `StoryLog` resource and flushes to SQLite every
//! 5 minutes.

use crate::world::ecology::{RegionId, SpeciesId};
use crate::world::faction::FactionId;
use bevy::ecs::message::Message;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use uuid::Uuid;

// ── Identifiers ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Hash, Component, Serialize, Deserialize, Reflect)]
#[reflect(opaque)]
pub struct GameEntityId(pub Uuid);

// ── Event kinds ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum StoryEventKind {
    // World sim events
    FactionWarDeclared { attacker: FactionId, defender: FactionId },
    SettlementFounded   { faction: FactionId, name: SmolStr },
    SettlementRazed     { by: FactionId },
    EcologyCollapse     { species: SpeciesId, region: RegionId },
    AllianceFormed      { a: FactionId, b: FactionId },
    // Player-triggered
    PlayerKilledNamed   { victim: GameEntityId, killer: GameEntityId },
    PartyDefeatedBoss   { boss: GameEntityId },
    QuestCompleted      { quest_id: SmolStr },
    PlayerJoinedFaction { player: GameEntityId, faction: FactionId },
    // Emergent
    NpcDefected         { npc: GameEntityId, from: FactionId, to: FactionId },
    MonsterMigrated     { species: SpeciesId, from: RegionId, to: RegionId },
    /// A war party in the underground (the Sunken Realm) is within a few hops
    /// of the surface and poised to erupt. Emitted while `hops_to_surface <= 3`.
    UndergroundThreat   { faction_id: SmolStr, hops_to_surface: usize },
}

// ── Story event ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct StoryEvent {
    pub id: Uuid,
    pub tick: u64,
    pub world_day: u32,
    pub kind: StoryEventKind,
    pub participants: Vec<GameEntityId>,
    pub location: Option<IVec2>,
    pub lore_tags: Vec<SmolStr>,
}

// ── Bevy event wrapper ────────────────────────────────────────────────────────

/// Send this message from any system to append to the story log.
#[derive(Message, Clone, Debug)]
pub struct WriteStoryEvent(pub StoryEvent);

// ── Resource: the in-memory log ───────────────────────────────────────────────

/// In-memory story log; flushed to SQLite by the persistence plugin.
#[derive(Resource, Default)]
pub struct StoryLog {
    pub events: Vec<StoryEvent>,
}

impl StoryLog {
    pub fn push(&mut self, ev: StoryEvent) {
        self.events.push(ev);
    }

    /// Return events whose lore_tags contain `tag`.
    pub fn by_tag(&self, tag: &str) -> Vec<&StoryEvent> {
        self.events
            .iter()
            .filter(|e| e.lore_tags.iter().any(|t| t.as_str() == tag))
            .collect()
    }
}
