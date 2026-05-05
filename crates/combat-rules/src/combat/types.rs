//! Core combat types: snapshots, state, effects, and results.
//!
//! All types here are pure data — no ECS, no I/O.

use crate::FactionId;
use bevy::reflect::Reflect;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use uuid::Uuid;

// ── Stable identity ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CombatantId(pub Uuid);

// ── Stats ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct CoreStats {
    pub strength: i32,
    pub dexterity: i32,
    pub constitution: i32,
    /// Intelligence (field kept as `intellect` for backward compatibility
    /// with existing tests and ECS snapshots).
    pub intellect: i32,
    pub wisdom: i32,
    pub charisma: i32,
}

impl Default for CoreStats {
    fn default() -> Self {
        Self {
            strength: 10,
            dexterity: 10,
            constitution: 10,
            intellect: 10,
            wisdom: 10,
            charisma: 10,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
pub enum CharacterClass {
    // ── Legacy player-facing aliases (kept for save compatibility) ────────────
    Warrior,
    Rogue,
    Mage,
    // ── Full SRD 5e classes (NPCs) ────────────────────────────────────────────
    Fighter,
    Wizard,
    Cleric,
    Ranger,
    Paladin,
    Druid,
    Bard,
    Warlock,
    Sorcerer,
    Monk,
    Barbarian,
}

// ── Snapshot ──────────────────────────────────────────────────────────────────

/// Immutable snapshot of one combatant passed into rule functions.
#[derive(Clone, Debug, PartialEq)]
pub struct CombatantSnapshot {
    pub id: CombatantId,
    pub faction: Option<FactionId>,
    pub class: CharacterClass,
    pub stats: CoreStats,
    pub health_current: i32,
    pub health_max: i32,
    pub level: u32,
    /// Armour Class — the total that an attack roll must meet or beat to hit.
    /// See `docs/dnd5e-srd-reference.md` for AC values by armour type.
    pub armor_class: i32,
}

impl CombatantSnapshot {
    /// Modifier from a stat value (D&D 5e SRD floor division).
    ///
    /// Uses `div_euclid` to get true floor division for negative values.
    /// Example: stat 9 → (9 − 10).div_euclid(2) = −1 (correct), not 0.
    pub fn modifier(stat: i32) -> i32 {
        (stat - 10).div_euclid(2)
    }
    pub fn str_mod(&self) -> i32 { Self::modifier(self.stats.strength) }
    pub fn dex_mod(&self) -> i32 { Self::modifier(self.stats.dexterity) }
    pub fn con_mod(&self) -> i32 { Self::modifier(self.stats.constitution) }
}

/// Proficiency bonus for the given character level (SRD table).
/// See `docs/dnd5e-srd-reference.md`.
pub fn proficiency_bonus(level: u32) -> i32 {
    match level {
        1..=4   => 2,
        5..=8   => 3,
        9..=12  => 4,
        13..=16 => 5,
        _       => 6, // levels 17–20+
    }
}

/// Hit die size for the given class (max face value of the die).
/// See `docs/dnd5e-srd-reference.md`.
pub fn hit_die_for_class(class: &CharacterClass) -> i32 {
    match class {
        // Legacy player aliases
        CharacterClass::Warrior   => 10,
        CharacterClass::Rogue     =>  8,
        CharacterClass::Mage      =>  6,
        // SRD NPC classes
        CharacterClass::Barbarian => 12,
        CharacterClass::Fighter   => 10,
        CharacterClass::Paladin   => 10,
        CharacterClass::Ranger    => 10,
        CharacterClass::Monk      =>  8,
        CharacterClass::Cleric    =>  8,
        CharacterClass::Druid     =>  8,
        CharacterClass::Bard      =>  8,
        CharacterClass::Warlock   =>  8,
        CharacterClass::Sorcerer  =>  6,
        CharacterClass::Wizard    =>  6,
    }
}

/// SRD ASI levels per class — the character levels at which the class gains
/// an Ability Score Improvement (+2 points).
///
/// Fighter has extra ASIs at 6 and 14; Rogue has an extra one at 10.
/// All other classes use the standard set: 4, 8, 12, 16, 19.
pub fn asi_levels_for_class(class: &CharacterClass) -> &'static [u32] {
    match class {
        CharacterClass::Fighter | CharacterClass::Warrior => &[4, 6, 8, 12, 14, 16, 19],
        CharacterClass::Rogue => &[4, 8, 10, 12, 16, 19],
        _ => &[4, 8, 12, 16, 19],
    }
}

// ── Effects ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    TakeDamage { target: CombatantId, amount: i32 },
    HealDamage  { target: CombatantId, amount: i32 },
    ApplyStatus { target: CombatantId, status: SmolStr },
    Die         { target: CombatantId },
}

// ── Combat state ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct CombatantState {
    pub snapshot: CombatantSnapshot,
    pub health: i32,
    pub statuses: Vec<SmolStr>,
}

impl CombatantState {
    pub fn new(snapshot: CombatantSnapshot) -> Self {
        let health = snapshot.health_current;
        Self { snapshot, health, statuses: vec![] }
    }
    pub fn is_alive(&self) -> bool { self.health > 0 }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CombatState {
    pub combatants: Vec<CombatantState>,
    pub round: u32,
}

impl CombatState {
    pub fn get(&self, id: &CombatantId) -> Option<&CombatantState> {
        self.combatants.iter().find(|c| &c.snapshot.id == id)
    }
    pub fn get_mut(&mut self, id: &CombatantId) -> Option<&mut CombatantState> {
        self.combatants.iter_mut().find(|c| &c.snapshot.id == id)
    }
}

// ── Attack roll result ────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum AttackRollResult {
    CriticalHit,
    Hit,
    Miss,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asi_levels_standard_classes() {
        let standard = asi_levels_for_class(&CharacterClass::Wizard);
        assert_eq!(standard, &[4, 8, 12, 16, 19]);
        assert_eq!(asi_levels_for_class(&CharacterClass::Cleric), &[4, 8, 12, 16, 19]);
        assert_eq!(asi_levels_for_class(&CharacterClass::Barbarian), &[4, 8, 12, 16, 19]);
    }

    #[test]
    fn asi_levels_fighter_has_extras() {
        let fighter = asi_levels_for_class(&CharacterClass::Fighter);
        assert!(fighter.contains(&6));
        assert!(fighter.contains(&14));
        assert_eq!(fighter, &[4, 6, 8, 12, 14, 16, 19]);
        assert_eq!(asi_levels_for_class(&CharacterClass::Warrior), fighter);
    }

    #[test]
    fn asi_levels_rogue_has_extra_at_10() {
        let rogue = asi_levels_for_class(&CharacterClass::Rogue);
        assert!(rogue.contains(&10));
        assert!(!rogue.contains(&6));
        assert_eq!(rogue, &[4, 8, 10, 12, 16, 19]);
    }
}
