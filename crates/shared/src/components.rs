// Shared ECS components — replicated between server and client.

use crate::combat::types::CharacterClass;
use crate::world::faction::NpcRank;
use bevy::ecs::message::Message;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// 3-D world position (game units, not pixels).
///
/// This is the single canonical position component replicated
/// from server to every connected client.
/// `z` is the elevation — entities follow terrain height automatically.
#[derive(
    Component, Clone, PartialEq, Debug, Default,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub struct WorldPosition {
    pub x: f32,
    pub y: f32,
    /// Elevation in world units. 0 = sea level.
    pub z: f32,
}

/// Current and maximum hit points — replicated so clients can render health bars.
#[derive(
    Component, Clone, PartialEq, Debug,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub struct Health {
    pub current: i32,
    pub max: i32,
}

/// Player experience and level — replicated so clients can render the HUD.
#[derive(
    Component, Clone, PartialEq, Debug,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub struct Experience {
    pub xp: u32,
    pub level: u32,
    /// XP required to reach the next level.
    pub xp_to_next: u32,
}

impl Experience {
    /// SRD XP required to *reach* each level (index = level, 1-indexed).
    pub const XP_THRESHOLDS: [u32; 21] = [
        0,       // padding (unused)
        0,       // level 1
        300,     // to reach level 2
        900,     // to reach level 3
        2_700,   // to reach level 4
        6_500,   // to reach level 5
        14_000,  // to reach level 6
        23_000,  // to reach level 7
        34_000,  // to reach level 8
        48_000,  // to reach level 9
        64_000,  // to reach level 10
        85_000,  // to reach level 11
        100_000, // to reach level 12
        120_000, // to reach level 13
        140_000, // to reach level 14
        165_000, // to reach level 15
        195_000, // to reach level 16
        225_000, // to reach level 17
        265_000, // to reach level 18
        305_000, // to reach level 19
        355_000, // to reach level 20
    ];

    /// XP required to advance from `current_level` to the next level.
    /// Returns 0 at level 20 (cap).
    pub fn xp_to_next_level(current_level: u32) -> u32 {
        let next = (current_level + 1).min(20) as usize;
        Self::XP_THRESHOLDS[next]
    }

    pub fn new() -> Self {
        Self { xp: 0, level: 1, xp_to_next: Self::xp_to_next_level(1) }
    }
}

impl Default for Experience {
    fn default() -> Self {
        Self::new()
    }
}

/// Visual / gameplay kind for non-player entities — replicated so the client
/// can render each type with a distinct mesh and colour.
///
/// Players do **not** carry this component; its absence on a replicated entity
/// signals "local or remote player".
#[derive(
    Component, Clone, PartialEq, Debug,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub enum EntityKind {
    /// Faction-aligned guard or soldier NPC.
    FactionNpc,
    /// Ecology-driven predator or prey creature.
    Wildlife,
    /// Static settlement marker (capital or town).
    Settlement,
}

/// Species variant for wildlife entities — replicated so the client renders the correct model.
///
/// Absent on non-wildlife entities.
#[derive(
    Component, Clone, PartialEq, Debug, Default,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub enum WildlifeKind {
    #[default]
    Bison,
    Dog,
    Horse,
}

/// Growth stage for entities — 0.0 = newborn, 1.0 = full adult.
///
/// Replicated so the client can scale entity visuals proportionally.
/// Absent on entities spawned as adults (treated as 1.0 by the renderer).
#[derive(
    Component, Clone, PartialEq, Debug, Default,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub struct GrowthStage(pub f32);

/// Economic role assigned to a faction NPC on spawn (issue #108).
///
/// Server-only — drives the NPC's day-to-day economic behaviour (farming,
/// hunting, trading, guarding). Not replicated to clients.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Reflect)]
#[reflect(Component)]
pub struct EconomicRole {
    pub role: EconomicRoleKind,
    /// Tile coordinates of the NPC's primary workplace (e.g. farm tile, market).
    pub workplace_tile: (u32, u32),
}

/// The economic role an NPC plays within its settlement.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Reflect)]
pub enum EconomicRoleKind {
    /// Works a nearby farm tile, adding food to the settlement economy.
    Farmer,
    /// Patrols wilderness, hunting prey animals for food.
    Hunter,
    /// Travels between settlements on roads, adding trade income on arrival.
    Merchant,
    /// Defends the settlement perimeter.
    Guard,
    /// Combat-ready soldier assigned to war parties.
    Soldier,
    /// No assigned role (default for overflow or special NPCs).
    Idle,
}

/// Replicated faction badge — identifies which faction an NPC belongs to and its
/// rank so the client can tint capsule meshes per-faction.
///
/// Absent on player entities; only present on `EntityKind::FactionNpc` entities.
#[derive(
    Component, Clone, PartialEq, Debug,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub struct FactionBadge {
    /// String form of `FactionId` — avoids pulling `SmolStr` into lightyear's registry.
    pub faction_id: String,
    pub rank: NpcRank,
}

/// Per-faction reputation scores for a player — replicated to the owning client
/// so the HUD can display standings without a server round-trip.
///
/// Updated every world-sim tick (1 Hz) from `PlayerReputationMap`.
/// Only present on player entities.
#[derive(
    Component, Clone, PartialEq, Debug, Default,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub struct PlayerStandings {
    /// `(faction_name, score)` pairs for all known factions, sorted by name.
    pub standings: Vec<(String, i32)>,
}

/// World generation parameters — attached to the local player entity and
/// replicated to the client so the client can regenerate an identical
/// `WorldMap` for client-authoritative movement prediction.
///
/// Using `u32` (not `usize`) for stable cross-platform serialization.
#[derive(
    Component, Clone, PartialEq, Debug,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub struct WorldMeta {
    pub seed:   u64,
    pub width:  u32,
    pub height: u32,
}

/// Axis-aligned bounding box for collision.
///
/// `half_w` is the horizontal radius checked in all four cardinal quadrants.
/// `height` is the entity's vertical extent (feet to crown), reserved for
/// ceiling-clearance checks in a future pass.
#[derive(Component, Clone, Copy, PartialEq, Debug, Serialize, Deserialize, Reflect)]
#[reflect(Component)]
pub struct EntityBounds {
    pub half_w: f32,
    pub height: f32,
}

impl EntityBounds {
    /// Default human-sized player bounds.
    pub const PLAYER: Self = Self { half_w: 0.35, height: 1.8 };
    /// Point check — identical to the old single-point `is_walkable_at` behaviour.
    pub const POINT: Self = Self { half_w: 0.0, height: 0.0 };

    /// The four corners of the footprint in (dx, dy) offsets from the entity centre.
    #[inline]
    pub fn corners(self) -> [(f32, f32); 4] {
        let hw = self.half_w;
        [(-hw, -hw), (hw, -hw), (-hw, hw), (hw, hw)]
    }
}

impl Default for EntityBounds {
    fn default() -> Self { Self::PLAYER }
}

/// Sequence of nav-grid waypoints for an NPC to follow.
///
/// Only direction-change waypoints are stored (not every cell) to keep memory small.
/// `waypoint_index` is the next unvisited waypoint.
#[derive(Component, Clone, Debug, Default)]
pub struct NavPath {
    pub waypoints: Vec<(u16, u16)>,
    pub waypoint_index: usize,
}

impl NavPath {
    pub fn is_complete(&self) -> bool {
        self.waypoint_index >= self.waypoints.len()
    }

    pub fn next_waypoint(&self) -> Option<(u16, u16)> {
        self.waypoints.get(self.waypoint_index).copied()
    }
}

/// Tracks ticks since last A* replan for an NPC.
#[derive(Component, Clone, Debug, Default)]
pub struct NavReplanTimer(pub u32);

/// Marker component: this entity never initiates combat.
///
/// NPCs with this component are skipped by the faction aggression check —
/// used for PeacefulSanctuary dwellers and other non-hostile factions.
#[derive(Component, Clone, Debug, Default, Reflect)]
#[reflect(Component)]
pub struct Pacifist;

/// Marker: this entity has a pending Ability Score Improvement.
///
/// Added when the entity reaches an ASI level. NPC entities resolve it
/// immediately on the next tick (primary stat +2). Player entities keep it
/// pending until the player chooses scores via the UI (future work).
#[derive(Component, Clone, Debug, Default, Reflect)]
#[reflect(Component)]
pub struct PendingAsi;

// ── D&D 5e SRD Ability Scores ─────────────────────────────────────────────────

/// The six D&D 5e SRD ability scores for a character or NPC.
///
/// Standard arrays per class:
/// - Warrior: STR 15, DEX 13, CON 14, INT 8, WIS 10, CHA 12
/// - Rogue:   STR 8,  DEX 15, CON 13, INT 12, WIS 10, CHA 14
/// - Mage:    STR 8,  DEX 13, CON 12, INT 15, WIS 14, CHA 10
///
/// See `docs/dnd5e-srd-reference.md`.
#[derive(
    Component, Clone, PartialEq, Debug,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub struct AbilityScores {
    pub strength:     u8,
    pub dexterity:    u8,
    pub constitution: u8,
    pub intelligence: u8,
    pub wisdom:       u8,
    pub charisma:     u8,
}

impl AbilityScores {
    /// D&D 5e ability modifier: `floor((score - 10) / 2)`.
    ///
    /// Uses arithmetic (floor) division, not truncating division.
    /// Example: score 1 → (1 − 10) / 2 = −9 / 2 = −5 (not −4).
    pub fn modifier(score: u8) -> i8 {
        let diff = score as i16 - 10;
        // Rust integer division truncates toward zero; we need floor division.
        // For negative odd differences: diff.div_euclid(2) gives the correct floor.
        diff.div_euclid(2) as i8
    }

    pub fn str_mod(&self) -> i8  { Self::modifier(self.strength) }
    pub fn dex_mod(&self) -> i8  { Self::modifier(self.dexterity) }
    pub fn con_mod(&self) -> i8  { Self::modifier(self.constitution) }
    pub fn int_mod(&self) -> i8  { Self::modifier(self.intelligence) }
    pub fn wis_mod(&self) -> i8  { Self::modifier(self.wisdom) }
    pub fn cha_mod(&self) -> i8  { Self::modifier(self.charisma) }

    /// Standard array for a Warrior (STR-primary).
    pub fn warrior() -> Self {
        Self { strength: 15, dexterity: 13, constitution: 14, intelligence: 8, wisdom: 10, charisma: 12 }
    }

    /// Standard array for a Rogue (DEX-primary).
    pub fn rogue() -> Self {
        Self { strength: 8, dexterity: 15, constitution: 13, intelligence: 12, wisdom: 10, charisma: 14 }
    }

    /// Standard array for a Mage (INT-primary).
    pub fn mage() -> Self {
        Self { strength: 8, dexterity: 13, constitution: 12, intelligence: 15, wisdom: 14, charisma: 10 }
    }

    /// Generic NPC stat block (unspecialised grunt).
    pub fn npc_grunt() -> Self {
        Self { strength: 10, dexterity: 10, constitution: 10, intelligence: 10, wisdom: 10, charisma: 10 }
    }

    /// Factory: generate stat block for an NPC by class and rank.
    ///
    /// Primary stat scales with rank:
    ///   - Grunt: 12 in primary stat, 10 elsewhere
    ///   - Named: 15 in primary stat, 12 elsewhere
    ///   - Boss:  18 in primary stat, 14 elsewhere
    pub fn for_class(class: &CharacterClass, rank: NpcRank) -> Self {
        let (primary, secondary) = match rank {
            NpcRank::Grunt => (12u8, 10u8),
            NpcRank::Named => (15u8, 12u8),
            NpcRank::Boss  => (18u8, 14u8),
        };
        match class {
            CharacterClass::Warrior | CharacterClass::Fighter | CharacterClass::Paladin => Self {
                strength: primary, dexterity: secondary, constitution: secondary,
                intelligence: 8, wisdom: secondary, charisma: 10,
            },
            CharacterClass::Barbarian => Self {
                strength: primary, dexterity: secondary, constitution: primary,
                intelligence: 8, wisdom: 10, charisma: 8,
            },
            CharacterClass::Ranger | CharacterClass::Rogue | CharacterClass::Monk => Self {
                strength: 10, dexterity: primary, constitution: secondary,
                intelligence: 12, wisdom: secondary, charisma: 10,
            },
            CharacterClass::Mage | CharacterClass::Wizard | CharacterClass::Sorcerer => Self {
                strength: 8, dexterity: secondary, constitution: secondary,
                intelligence: primary, wisdom: secondary, charisma: 10,
            },
            CharacterClass::Warlock => Self {
                strength: 8, dexterity: secondary, constitution: secondary,
                intelligence: secondary, wisdom: 10, charisma: primary,
            },
            CharacterClass::Cleric | CharacterClass::Druid => Self {
                strength: secondary, dexterity: 10, constitution: secondary,
                intelligence: 12, wisdom: primary, charisma: secondary,
            },
            CharacterClass::Bard => Self {
                strength: 10, dexterity: secondary, constitution: secondary,
                intelligence: secondary, wisdom: 10, charisma: primary,
            },
        }
    }

    /// Return a copy of these scores with the NPC's primary stat increased by 2, capped at 20.
    ///
    /// The primary stat is determined by class following the same groupings as `for_class`.
    /// Used by the NPC auto-ASI system when `PendingAsi` is resolved.
    pub fn with_npc_asi(&self, class: &CharacterClass) -> Self {
        let mut s = self.clone();
        match class {
            CharacterClass::Warrior | CharacterClass::Fighter | CharacterClass::Paladin | CharacterClass::Barbarian => {
                s.strength = s.strength.saturating_add(2).min(20);
            }
            CharacterClass::Ranger | CharacterClass::Rogue | CharacterClass::Monk => {
                s.dexterity = s.dexterity.saturating_add(2).min(20);
            }
            CharacterClass::Mage | CharacterClass::Wizard | CharacterClass::Sorcerer => {
                s.intelligence = s.intelligence.saturating_add(2).min(20);
            }
            CharacterClass::Warlock | CharacterClass::Bard => {
                s.charisma = s.charisma.saturating_add(2).min(20);
            }
            CharacterClass::Cleric | CharacterClass::Druid => {
                s.wisdom = s.wisdom.saturating_add(2).min(20);
            }
        }
        s
    }
}

impl Default for AbilityScores {
    fn default() -> Self { Self::npc_grunt() }
}

// ── D&D 5e Hit Dice ───────────────────────────────────────────────────────────

/// How many hit dice a character has and of what type.
/// `count` = character level; `die` = hit die size (6, 8, 10, or 12).
#[derive(Component, Clone, PartialEq, Debug, Serialize, Deserialize, Reflect)]
#[reflect(Component)]
pub struct HitDice {
    pub die: u8,
    pub count: u8,
}

impl HitDice {
    pub fn for_class_level(class: &CharacterClass, level: u32) -> Self {
        use crate::combat::types::hit_die_for_class;
        Self { die: hit_die_for_class(class) as u8, count: level as u8 }
    }
    /// SRD average roll rounded up: (die / 2) + 1.
    pub fn average_roll(&self) -> u8 {
        (self.die / 2) + 1
    }
}

// ── D&D 5e Ability Modifiers (derived) ───────────────────────────────────────

/// Derived modifiers computed from `AbilityScores`.
/// Updated reactively by `sync_ability_modifiers` when `AbilityScores` changes.
#[derive(Component, Clone, PartialEq, Debug, Default, Serialize, Deserialize, Reflect)]
#[reflect(Component)]
pub struct AbilityModifiers {
    pub strength:     i8,
    pub dexterity:    i8,
    pub constitution: i8,
    pub intelligence: i8,
    pub wisdom:       i8,
    pub charisma:     i8,
}

impl AbilityModifiers {
    pub fn from_scores(scores: &AbilityScores) -> Self {
        Self {
            strength:     scores.str_mod(),
            dexterity:    scores.dex_mod(),
            constitution: scores.con_mod(),
            intelligence: scores.int_mod(),
            wisdom:       scores.wis_mod(),
            charisma:     scores.cha_mod(),
        }
    }
}

/// Reactively recompute `AbilityModifiers` whenever `AbilityScores` changes.
pub fn sync_ability_modifiers(
    mut query: Query<(&AbilityScores, &mut AbilityModifiers), Changed<AbilityScores>>,
) {
    for (scores, mut mods) in &mut query {
        *mods = AbilityModifiers::from_scores(scores);
    }
}

/// Reactively recalculate `Health.max` whenever `HitDice` or `AbilityScores` changes.
/// Uses SRD average method: level 1 = max die + CON mod; level 2+ = average rounded up + CON mod.
/// Each level contributes at minimum 1 HP. `Health.current` is clamped to the new maximum.
#[allow(clippy::type_complexity)]
pub fn sync_max_hp(
    mut query: Query<
        (&HitDice, &AbilityScores, &mut Health),
        Or<(Changed<HitDice>, Changed<AbilityScores>)>,
    >,
) {
    for (hit_dice, scores, mut health) in &mut query {
        let con_mod = scores.con_mod() as i32;
        let new_max = crate::combat::calculate_max_hp(hit_dice.die, hit_dice.count as u32, con_mod);
        health.max = new_max;
        health.current = health.current.min(new_max);
    }
}

// ── D&D 5e SRD Saving Throw Proficiencies ────────────────────────────────────

/// Which saving throws a character is proficient in (SRD class feature).
///
/// Class defaults per SRD:
/// - Warrior: STR, CON
/// - Rogue:   DEX, INT
/// - Mage:    INT, WIS
#[derive(
    Component, Clone, PartialEq, Debug, Default,
    Serialize, Deserialize, Reflect,
)]
#[reflect(Component)]
pub struct SavingThrowProficiencies {
    pub strength:     bool,
    pub dexterity:    bool,
    pub constitution: bool,
    pub intelligence: bool,
    pub wisdom:       bool,
    pub charisma:     bool,
}

impl SavingThrowProficiencies {
    /// Warrior saves: STR, CON (SRD §Fighter).
    pub fn warrior() -> Self {
        Self { strength: true, constitution: true, ..Default::default() }
    }

    /// Rogue saves: DEX, INT (SRD §Rogue).
    pub fn rogue() -> Self {
        Self { dexterity: true, intelligence: true, ..Default::default() }
    }

    /// Mage saves: INT, WIS (SRD §Wizard).
    pub fn mage() -> Self {
        Self { intelligence: true, wisdom: true, ..Default::default() }
    }

    /// Fighter saves: STR, CON (SRD §Fighter).
    pub fn fighter() -> Self { Self::warrior() }

    /// Barbarian saves: STR, CON (SRD §Barbarian).
    pub fn barbarian() -> Self { Self::warrior() }

    /// Paladin saves: WIS, CHA (SRD §Paladin).
    pub fn paladin() -> Self {
        Self { wisdom: true, charisma: true, ..Default::default() }
    }

    /// Ranger saves: STR, DEX (SRD §Ranger).
    pub fn ranger() -> Self {
        Self { strength: true, dexterity: true, ..Default::default() }
    }

    /// Cleric saves: WIS, CHA (SRD §Cleric).
    pub fn cleric() -> Self { Self::paladin() }

    /// Druid saves: INT, WIS (SRD §Druid).
    pub fn druid() -> Self { Self::mage() }

    /// Bard saves: DEX, CHA (SRD §Bard).
    pub fn bard() -> Self {
        Self { dexterity: true, charisma: true, ..Default::default() }
    }

    /// Warlock saves: WIS, CHA (SRD §Warlock).
    pub fn warlock() -> Self { Self::paladin() }

    /// Sorcerer saves: CON, CHA (SRD §Sorcerer).
    pub fn sorcerer() -> Self {
        Self { constitution: true, charisma: true, ..Default::default() }
    }

    /// Wizard saves: INT, WIS (SRD §Wizard).
    pub fn wizard() -> Self { Self::mage() }

    /// Monk saves: STR, DEX (SRD §Monk).
    pub fn monk() -> Self { Self::ranger() }

    /// Generate saving throw proficiencies for any `CharacterClass`.
    pub fn for_class(class: &CharacterClass) -> Self {
        match class {
            CharacterClass::Warrior   => Self::warrior(),
            CharacterClass::Fighter   => Self::fighter(),
            CharacterClass::Barbarian => Self::barbarian(),
            CharacterClass::Paladin   => Self::paladin(),
            CharacterClass::Ranger    => Self::ranger(),
            CharacterClass::Monk      => Self::monk(),
            CharacterClass::Rogue     => Self::rogue(),
            CharacterClass::Bard      => Self::bard(),
            CharacterClass::Mage      => Self::mage(),
            CharacterClass::Wizard    => Self::wizard(),
            CharacterClass::Druid     => Self::druid(),
            CharacterClass::Cleric    => Self::cleric(),
            CharacterClass::Warlock   => Self::warlock(),
            CharacterClass::Sorcerer  => Self::sorcerer(),
        }
    }
}

// ── Action Economy (#126) ─────────────────────────────────────────────────────

/// Which action economy slot an ability consumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub enum ActionSlot {
    /// Standard action: basic attack, use item, cast a spell.
    Action,
    /// Bonus action: off-hand attack, some class features.
    BonusAction,
    /// Reaction: opportunity attack, Shield spell.
    Reaction,
}

/// Per-round action economy budget (D&D 5e: Action, Bonus Action, Reaction, movement).
///
/// In real-time mode, spent slots recharge after ~6 s (one D&D round) via
/// `ActionCooldowns` on the server.  In future turn-based mode, all slots reset
/// at the top of each combatant's turn.
///
/// Server-authoritative; replicated to clients so the HUD can render pip indicators.
#[derive(Component, Clone, PartialEq, Debug, Serialize, Deserialize, Reflect)]
#[reflect(Component)]
pub struct ActionBudget {
    /// Standard action (basic attack, spell, use item). Recharges after one round.
    pub action: bool,
    /// Bonus action (off-hand attack, some class features). Recharges after one round.
    pub bonus_action: bool,
    /// Reaction (opportunity attack). One per round.
    pub reaction: bool,
    /// Remaining movement this round in world units (default = PLAYER_SPEED × 6 s = 15.0).
    pub movement_remaining: f32,
}

impl Default for ActionBudget {
    fn default() -> Self {
        Self {
            action: true,
            bonus_action: true,
            reaction: true,
            movement_remaining: 15.0,
        }
    }
}

impl ActionBudget {
    /// Try to spend the given slot. Returns `true` if it was available and is now spent.
    pub fn consume(&mut self, slot: ActionSlot) -> bool {
        match slot {
            ActionSlot::Action      => std::mem::replace(&mut self.action, false),
            ActionSlot::BonusAction => std::mem::replace(&mut self.bonus_action, false),
            ActionSlot::Reaction    => std::mem::replace(&mut self.reaction, false),
        }
    }

    /// Restore all slots and reset movement to the default.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Broadcast when a combatant spends an action slot — animation and audio hook.
#[derive(Message, Clone, Debug)]
pub struct ActionUsedEvent {
    pub entity: Entity,
    pub slot: ActionSlot,
}

// ── NPC Dialogue Flavor (#131) ────────────────────────────────────────────────

/// Return a short greeting line for the given class and rank.
///
/// Used when the player interacts with an NPC. If no interaction system exists
/// yet, this function is wired in for future use.
pub fn greeting_flavor(class: &CharacterClass, rank: NpcRank) -> &'static str {
    match (class, rank) {
        // ── Boss greetings ─────────────────────────────────────────────────────
        (CharacterClass::Warrior | CharacterClass::Fighter, NpcRank::Boss) =>
            "I have bested a thousand warriors. You are nothing.",
        (CharacterClass::Barbarian, NpcRank::Boss) =>
            "MY RAGE IS ETERNAL! Face me if you dare!",
        (CharacterClass::Paladin, NpcRank::Boss) =>
            "The divine has condemned you. Prepare for judgment.",
        (CharacterClass::Mage | CharacterClass::Wizard, NpcRank::Boss) =>
            "You have stepped into the presence of a true archmage. Regret it.",
        (CharacterClass::Sorcerer, NpcRank::Boss) =>
            "The arcane blood in my veins burns with your destruction in mind.",
        (CharacterClass::Warlock, NpcRank::Boss) =>
            "My patron desires your soul. I am merely the instrument.",
        (CharacterClass::Cleric | CharacterClass::Druid, NpcRank::Boss) =>
            "The old powers stir. You should not have come here.",
        (CharacterClass::Rogue, NpcRank::Boss) =>
            "You never saw me coming. You still don't understand what that means.",
        (CharacterClass::Ranger, NpcRank::Boss) =>
            "I have tracked prey across a thousand leagues. You are no different.",
        (CharacterClass::Bard, NpcRank::Boss) =>
            "Every hero has a story. Yours ends here — I've written the finale.",
        (CharacterClass::Monk, NpcRank::Boss) =>
            "Stillness. Then oblivion. That is your fate.",
        // ── Named greetings ────────────────────────────────────────────────────
        (CharacterClass::Warrior | CharacterClass::Fighter, NpcRank::Named) =>
            "I guard this place with my life. State your business.",
        (CharacterClass::Barbarian, NpcRank::Named) =>
            "Speak quickly before my patience runs out.",
        (CharacterClass::Paladin, NpcRank::Named) =>
            "The light guides my blade. Tread carefully, traveller.",
        (CharacterClass::Mage | CharacterClass::Wizard, NpcRank::Named) =>
            "Curiosity brought you here? Wisdom will decide if you leave.",
        (CharacterClass::Sorcerer, NpcRank::Named) =>
            "The weave bends to my will. Mind your words.",
        (CharacterClass::Warlock, NpcRank::Named) =>
            "I've made deals you couldn't imagine. What do you want?",
        (CharacterClass::Cleric, NpcRank::Named) =>
            "May the gods watch over you — though I'll be watching too.",
        (CharacterClass::Druid, NpcRank::Named) =>
            "The forest has shown me your coming. Not all omens are welcome.",
        (CharacterClass::Rogue, NpcRank::Named) =>
            "You're lucky I decided to let you see me.",
        (CharacterClass::Ranger, NpcRank::Named) =>
            "I don't get many visitors out here. There's a reason for that.",
        (CharacterClass::Bard, NpcRank::Named) =>
            "Ah, a new face! Every face is a story. Buy me a drink?",
        (CharacterClass::Monk, NpcRank::Named) =>
            "Breathe. Centre yourself. Then we may speak.",
        // ── Grunt greetings ────────────────────────────────────────────────────
        (CharacterClass::Warrior | CharacterClass::Fighter, NpcRank::Grunt) =>
            "Move along. Nothing here for you.",
        (CharacterClass::Barbarian, NpcRank::Grunt) =>
            "Hrm. What do you want?",
        (CharacterClass::Paladin, NpcRank::Grunt) =>
            "The order keeps the peace. Keep yours.",
        (CharacterClass::Mage | CharacterClass::Wizard, NpcRank::Grunt) =>
            "I'm busy. Come back later.",
        (CharacterClass::Sorcerer, NpcRank::Grunt) =>
            "Sparks fly when I'm bothered. Consider that a warning.",
        (CharacterClass::Warlock, NpcRank::Grunt) =>
            "My patron doesn't like interruptions.",
        (CharacterClass::Cleric, NpcRank::Grunt) =>
            "Blessings upon you. Now if you'll excuse me…",
        (CharacterClass::Druid, NpcRank::Grunt) =>
            "The roots and the rain care nothing for your problems.",
        (CharacterClass::Rogue, NpcRank::Grunt) =>
            "Eyes forward. Wasn't me.",
        (CharacterClass::Ranger, NpcRank::Grunt) =>
            "Keep your noise down — you'll scare the wildlife.",
        (CharacterClass::Bard, NpcRank::Grunt) =>
            "Coin for a song? No? Then good day to you.",
        (CharacterClass::Monk, NpcRank::Grunt) =>
            "Peace. Be still.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ability_scores_modifier_spot_checks() {
        assert_eq!(AbilityScores::modifier(10), 0);
        assert_eq!(AbilityScores::modifier(11), 0);
        assert_eq!(AbilityScores::modifier(12), 1);
        assert_eq!(AbilityScores::modifier(15), 2);
        assert_eq!(AbilityScores::modifier(8), -1);
        assert_eq!(AbilityScores::modifier(1), -5);
        assert_eq!(AbilityScores::modifier(20), 5);
    }

    #[test]
    fn ability_scores_warrior_standard_array() {
        let a = AbilityScores::warrior();
        assert_eq!(a.strength, 15);
        assert_eq!(a.constitution, 14);
        assert_eq!(a.str_mod(), 2);
        assert_eq!(a.con_mod(), 2);
    }

    #[test]
    fn ability_scores_rogue_standard_array() {
        let a = AbilityScores::rogue();
        assert_eq!(a.dexterity, 15);
        assert_eq!(a.dex_mod(), 2);
    }

    #[test]
    fn ability_scores_mage_standard_array() {
        let a = AbilityScores::mage();
        assert_eq!(a.intelligence, 15);
        assert_eq!(a.int_mod(), 2);
    }

    #[test]
    fn saving_throw_proficiencies_warrior() {
        let s = SavingThrowProficiencies::warrior();
        assert!(s.strength);
        assert!(s.constitution);
        assert!(!s.dexterity);
        assert!(!s.intelligence);
    }

    #[test]
    fn saving_throw_proficiencies_rogue() {
        let s = SavingThrowProficiencies::rogue();
        assert!(s.dexterity);
        assert!(s.intelligence);
        assert!(!s.strength);
    }

    #[test]
    fn saving_throw_proficiencies_mage() {
        let s = SavingThrowProficiencies::mage();
        assert!(s.intelligence);
        assert!(s.wisdom);
        assert!(!s.constitution);
    }

    #[test]
    fn saving_throw_proficiencies_paladin() {
        let s = SavingThrowProficiencies::paladin();
        assert!(s.wisdom);
        assert!(s.charisma);
        assert!(!s.strength);
    }

    #[test]
    fn saving_throw_proficiencies_ranger() {
        let s = SavingThrowProficiencies::ranger();
        assert!(s.strength);
        assert!(s.dexterity);
        assert!(!s.constitution);
    }

    #[test]
    fn saving_throw_proficiencies_bard() {
        let s = SavingThrowProficiencies::bard();
        assert!(s.dexterity);
        assert!(s.charisma);
        assert!(!s.strength);
    }

    #[test]
    fn saving_throw_proficiencies_sorcerer() {
        let s = SavingThrowProficiencies::sorcerer();
        assert!(s.constitution);
        assert!(s.charisma);
        assert!(!s.intelligence);
    }

    #[test]
    fn saving_throw_proficiencies_for_class_covers_all_classes() {
        use crate::combat::types::CharacterClass;
        let cases = [
            (CharacterClass::Warrior,   true,  false, true,  false, false, false),
            (CharacterClass::Fighter,   true,  false, true,  false, false, false),
            (CharacterClass::Barbarian, true,  false, true,  false, false, false),
            (CharacterClass::Rogue,     false, true,  false, true,  false, false),
            (CharacterClass::Bard,      false, true,  false, false, false, true),
            (CharacterClass::Monk,      true,  true,  false, false, false, false),
            (CharacterClass::Ranger,    true,  true,  false, false, false, false),
            (CharacterClass::Paladin,   false, false, false, false, true,  true),
            (CharacterClass::Cleric,    false, false, false, false, true,  true),
            (CharacterClass::Warlock,   false, false, false, false, true,  true),
            (CharacterClass::Mage,      false, false, false, true,  true,  false),
            (CharacterClass::Wizard,    false, false, false, true,  true,  false),
            (CharacterClass::Druid,     false, false, false, true,  true,  false),
            (CharacterClass::Sorcerer,  false, false, true,  false, false, true),
        ];
        for (class, str, dex, con, int, wis, cha) in cases {
            let s = SavingThrowProficiencies::for_class(&class);
            assert_eq!(s.strength,     str, "{class:?} strength");
            assert_eq!(s.dexterity,    dex, "{class:?} dexterity");
            assert_eq!(s.constitution, con, "{class:?} constitution");
            assert_eq!(s.intelligence, int, "{class:?} intelligence");
            assert_eq!(s.wisdom,       wis, "{class:?} wisdom");
            assert_eq!(s.charisma,     cha, "{class:?} charisma");
        }
    }

    #[test]
    fn wildlife_kind_default_is_bison() {
        assert_eq!(WildlifeKind::default(), WildlifeKind::Bison);
    }

    #[test]
    fn entity_bounds_corners_symmetric() {
        let b = EntityBounds { half_w: 0.5, height: 1.0 };
        let c = b.corners();
        assert_eq!(c, [(-0.5, -0.5), (0.5, -0.5), (-0.5, 0.5), (0.5, 0.5)]);
    }

    #[test]
    fn wildlife_kind_serde_round_trip() {
        for variant in [WildlifeKind::Bison, WildlifeKind::Dog, WildlifeKind::Horse] {
            let json = serde_json::to_string(&variant).unwrap();
            let back: WildlifeKind = serde_json::from_str(&json).unwrap();
            assert_eq!(variant, back);
        }
    }

    #[test]
    fn action_budget_default_all_available() {
        let b = ActionBudget::default();
        assert!(b.action);
        assert!(b.bonus_action);
        assert!(b.reaction);
        assert!(b.movement_remaining > 0.0);
    }

    #[test]
    fn action_budget_consume_action() {
        let mut b = ActionBudget::default();
        assert!(b.consume(ActionSlot::Action));
        assert!(!b.action);
        assert!(b.bonus_action);
        assert!(b.reaction);
    }

    #[test]
    fn action_budget_cannot_double_spend() {
        let mut b = ActionBudget::default();
        assert!(b.consume(ActionSlot::BonusAction));
        assert!(!b.consume(ActionSlot::BonusAction));
    }

    #[test]
    fn action_budget_reset_restores_all() {
        let mut b = ActionBudget::default();
        b.consume(ActionSlot::Action);
        b.consume(ActionSlot::BonusAction);
        b.consume(ActionSlot::Reaction);
        b.reset();
        assert!(b.action);
        assert!(b.bonus_action);
        assert!(b.reaction);
    }

    #[test]
    fn action_budget_serde_round_trip() {
        let b = ActionBudget { action: false, bonus_action: true, reaction: false, movement_remaining: 7.5 };
        let json = serde_json::to_string(&b).unwrap();
        let back: ActionBudget = serde_json::from_str(&json).unwrap();
        assert_eq!(b, back);
    }

    #[test]
    fn npc_asi_increases_primary_stat() {
        use crate::combat::types::CharacterClass;
        let base = AbilityScores { strength: 15, dexterity: 13, constitution: 14, intelligence: 8, wisdom: 10, charisma: 12 };
        let after = base.with_npc_asi(&CharacterClass::Fighter);
        assert_eq!(after.strength, 17);
        assert_eq!(after.dexterity, 13); // unchanged

        let base_rogue = AbilityScores::rogue();
        let after_rogue = base_rogue.with_npc_asi(&CharacterClass::Rogue);
        assert_eq!(after_rogue.dexterity, 17);

        let base_wizard = AbilityScores::mage();
        let after_wizard = base_wizard.with_npc_asi(&CharacterClass::Wizard);
        assert_eq!(after_wizard.intelligence, 17);
    }

    #[test]
    fn npc_asi_caps_at_20() {
        use crate::combat::types::CharacterClass;
        let near_cap = AbilityScores { strength: 19, dexterity: 10, constitution: 10, intelligence: 10, wisdom: 10, charisma: 10 };
        let after = near_cap.with_npc_asi(&CharacterClass::Fighter);
        assert_eq!(after.strength, 20);

        let at_cap = AbilityScores { strength: 20, dexterity: 10, constitution: 10, intelligence: 10, wisdom: 10, charisma: 10 };
        let after2 = at_cap.with_npc_asi(&CharacterClass::Warrior);
        assert_eq!(after2.strength, 20);
    }
}
