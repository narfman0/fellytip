//! Core combat types: snapshots, state, effects, and results.
//!
//! All types here are pure data — no ECS, no I/O.

use crate::world::faction::FactionId;
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
    pub intellect: i32,
}

impl Default for CoreStats {
    fn default() -> Self {
        Self { strength: 10, dexterity: 10, constitution: 10, intellect: 10 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CharacterClass {
    Warrior,
    Rogue,
    Mage,
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
    /// Armour rating — reduces incoming physical damage.
    pub armor: i32,
}

impl CombatantSnapshot {
    /// Modifier from a stat value (D&D-style floor division).
    pub fn modifier(stat: i32) -> i32 {
        (stat - 10) / 2
    }
    pub fn str_mod(&self) -> i32 { Self::modifier(self.stats.strength) }
    pub fn dex_mod(&self) -> i32 { Self::modifier(self.stats.dexterity) }
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
