// Shared ECS components — replicated between server and client.

use crate::combat::types::CharacterClass;
use crate::world::faction::NpcRank;
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
    pub fn new() -> Self {
        Self { xp: 0, level: 1, xp_to_next: 300 }
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
}

impl Default for AbilityScores {
    fn default() -> Self { Self::npc_grunt() }
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
}
