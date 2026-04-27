//! Spell data, spell slots, and spellbook components (issue #125).
//!
//! All types here are pure data — no ECS I/O, no game logic.

use bevy::prelude::*;

use crate::combat::types::CharacterClass;

// ── Spell school & damage type ────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpellSchool {
    Evocation,
    Conjuration,
    Abjuration,
    Necromancy,
    Illusion,
    Transmutation,
    Divination,
    Enchantment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DamageType {
    Fire,
    Cold,
    Lightning,
    Thunder,
    Force,
    Necrotic,
    Radiant,
    Poison,
}

// ── Spell definition ──────────────────────────────────────────────────────────

pub struct Spell {
    pub name: &'static str,
    /// 0 = cantrip
    pub level: u8,
    pub school: SpellSchool,
    pub damage_dice_count: u8,
    pub damage_dice_sides: u8,
    pub damage_type: Option<DamageType>,
    pub heal_dice_count: u8,
    pub heal_dice_sides: u8,
    /// Saving throw ability: 0=STR,1=DEX,2=CON,3=INT,4=WIS,5=CHA
    pub save_ability: Option<u8>,
    pub save_dc_base: u8,
}

// ── Spell catalogue ───────────────────────────────────────────────────────────

pub const SPELLS: &[Spell] = &[
    // ── Cantrips ──────────────────────────────────────────────────────────────
    Spell {
        name: "Fire Bolt", level: 0, school: SpellSchool::Evocation,
        damage_dice_count: 1, damage_dice_sides: 10, damage_type: Some(DamageType::Fire),
        heal_dice_count: 0, heal_dice_sides: 0, save_ability: None, save_dc_base: 0,
    },
    Spell {
        name: "Ray of Frost", level: 0, school: SpellSchool::Evocation,
        damage_dice_count: 1, damage_dice_sides: 8, damage_type: Some(DamageType::Cold),
        heal_dice_count: 0, heal_dice_sides: 0, save_ability: None, save_dc_base: 0,
    },
    Spell {
        name: "Eldritch Blast", level: 0, school: SpellSchool::Evocation,
        damage_dice_count: 1, damage_dice_sides: 10, damage_type: Some(DamageType::Force),
        heal_dice_count: 0, heal_dice_sides: 0, save_ability: None, save_dc_base: 0,
    },
    Spell {
        name: "Sacred Flame", level: 0, school: SpellSchool::Evocation,
        damage_dice_count: 1, damage_dice_sides: 8, damage_type: Some(DamageType::Radiant),
        heal_dice_count: 0, heal_dice_sides: 0, save_ability: Some(1), save_dc_base: 13,
    },
    // ── Level 1 ───────────────────────────────────────────────────────────────
    Spell {
        name: "Magic Missile", level: 1, school: SpellSchool::Evocation,
        damage_dice_count: 3, damage_dice_sides: 4, damage_type: Some(DamageType::Force),
        heal_dice_count: 0, heal_dice_sides: 0, save_ability: None, save_dc_base: 0,
    },
    Spell {
        name: "Cure Wounds", level: 1, school: SpellSchool::Evocation,
        damage_dice_count: 0, damage_dice_sides: 0, damage_type: None,
        heal_dice_count: 1, heal_dice_sides: 8, save_ability: None, save_dc_base: 0,
    },
    Spell {
        name: "Burning Hands", level: 1, school: SpellSchool::Evocation,
        damage_dice_count: 3, damage_dice_sides: 6, damage_type: Some(DamageType::Fire),
        heal_dice_count: 0, heal_dice_sides: 0, save_ability: Some(1), save_dc_base: 13,
    },
    Spell {
        name: "Thunderwave", level: 1, school: SpellSchool::Evocation,
        damage_dice_count: 2, damage_dice_sides: 8, damage_type: Some(DamageType::Thunder),
        heal_dice_count: 0, heal_dice_sides: 0, save_ability: Some(2), save_dc_base: 13,
    },
    // ── Level 2 ───────────────────────────────────────────────────────────────
    Spell {
        name: "Shatter", level: 2, school: SpellSchool::Evocation,
        damage_dice_count: 3, damage_dice_sides: 8, damage_type: Some(DamageType::Thunder),
        heal_dice_count: 0, heal_dice_sides: 0, save_ability: Some(2), save_dc_base: 14,
    },
    Spell {
        name: "Scorching Ray", level: 2, school: SpellSchool::Evocation,
        damage_dice_count: 2, damage_dice_sides: 6, damage_type: Some(DamageType::Fire),
        heal_dice_count: 0, heal_dice_sides: 0, save_ability: None, save_dc_base: 0,
    },
    // ── Level 3 ───────────────────────────────────────────────────────────────
    Spell {
        name: "Fireball", level: 3, school: SpellSchool::Evocation,
        damage_dice_count: 8, damage_dice_sides: 6, damage_type: Some(DamageType::Fire),
        heal_dice_count: 0, heal_dice_sides: 0, save_ability: Some(1), save_dc_base: 15,
    },
    Spell {
        name: "Lightning Bolt", level: 3, school: SpellSchool::Evocation,
        damage_dice_count: 8, damage_dice_sides: 6, damage_type: Some(DamageType::Lightning),
        heal_dice_count: 0, heal_dice_sides: 0, save_ability: Some(1), save_dc_base: 15,
    },
];

/// Look up a spell by name (case-sensitive).
pub fn find_spell(name: &str) -> Option<&'static Spell> {
    SPELLS.iter().find(|s| s.name == name)
}

// ── Spell slots ───────────────────────────────────────────────────────────────

/// Tracks available and used spell slots per spell level (SRD 5e).
///
/// Index 0 is unused (cantrips require no slot). Indices 1–8 map to
/// spell levels 1–9 (array length 9 covers levels 1-9; index 0 unused).
#[derive(Component, Clone, Default)]
pub struct SpellSlots {
    /// Maximum slots per spell level (index 0 unused).
    pub max_slots: [u8; 9],
    /// Used slots per spell level (index 0 unused).
    pub used_slots: [u8; 9],
}

impl SpellSlots {
    /// SRD full-caster slot table for levels 1–20.
    ///
    /// Returns max slots for spell levels 1–9 (index 0 unused).
    fn full_caster_slots(level: u8) -> [u8; 9] {
        // SRD Table: Spell Slots per Spell Level (Wizard/Cleric/etc.)
        match level {
            1  => [0, 2, 0, 0, 0, 0, 0, 0, 0],
            2  => [0, 3, 0, 0, 0, 0, 0, 0, 0],
            3  => [0, 4, 2, 0, 0, 0, 0, 0, 0],
            4  => [0, 4, 3, 0, 0, 0, 0, 0, 0],
            5  => [0, 4, 3, 2, 0, 0, 0, 0, 0],
            6  => [0, 4, 3, 3, 0, 0, 0, 0, 0],
            7  => [0, 4, 3, 3, 1, 0, 0, 0, 0],
            8  => [0, 4, 3, 3, 2, 0, 0, 0, 0],
            9  => [0, 4, 3, 3, 3, 1, 0, 0, 0],
            10 => [0, 4, 3, 3, 3, 2, 0, 0, 0],
            11 => [0, 4, 3, 3, 3, 2, 1, 0, 0],
            12 => [0, 4, 3, 3, 3, 2, 1, 0, 0],
            13 => [0, 4, 3, 3, 3, 2, 1, 1, 0],
            14 => [0, 4, 3, 3, 3, 2, 1, 1, 0],
            15 => [0, 4, 3, 3, 3, 2, 1, 1, 1],
            16 => [0, 4, 3, 3, 3, 2, 1, 1, 1],
            17 => [0, 4, 3, 3, 3, 2, 1, 1, 1],
            18 => [0, 4, 3, 3, 3, 3, 1, 1, 1],
            19 => [0, 4, 3, 3, 3, 3, 2, 1, 1],
            _  => [0, 4, 3, 3, 3, 3, 2, 2, 1], // level 20+
        }
    }

    /// Half-caster slot table for Paladin / Ranger (SRD).
    fn half_caster_slots(level: u8) -> [u8; 9] {
        // Half-casters start casting at level 2 (Paladin) or 2 (Ranger).
        // Use floor(level/2) as effective caster level.
        let eff = (level / 2).max(1);
        match eff {
            1  => [0, 2, 0, 0, 0, 0, 0, 0, 0],
            2  => [0, 3, 0, 0, 0, 0, 0, 0, 0],
            3  => [0, 4, 2, 0, 0, 0, 0, 0, 0],
            4  => [0, 4, 3, 0, 0, 0, 0, 0, 0],
            5  => [0, 4, 3, 2, 0, 0, 0, 0, 0],
            6  => [0, 4, 3, 3, 0, 0, 0, 0, 0],
            7  => [0, 4, 3, 3, 1, 0, 0, 0, 0],
            8  => [0, 4, 3, 3, 2, 0, 0, 0, 0],
            _  => [0, 4, 3, 3, 3, 1, 0, 0, 0],
        }
    }

    /// Warlock pact magic: 1-2 slots, always at their max known spell level.
    fn warlock_slots(level: u8) -> [u8; 9] {
        // SRD Warlock: slots = 1 at L1, 2 from L2+; level tracks pact slot level.
        // Pact slot level: 1→1, 2→1, 3→2, 4→2, 5→3, 6→3, 7→4, 8→4, 9+→5
        let (slot_level, slot_count) = match level {
            1     => (1u8, 1u8),
            2     => (1, 2),
            3..=4 => (2, 2),
            5..=6 => (3, 2),
            7..=8 => (4, 2),
            _     => (5, 2),
        };
        let mut slots = [0u8; 9];
        if slot_level > 0 && slot_level < 9 {
            slots[slot_level as usize] = slot_count;
        }
        slots
    }

    /// Generate spell slots appropriate for a class and character level.
    pub fn for_class(class: &CharacterClass, level: u8) -> Self {
        let max_slots = match class {
            // Full casters
            CharacterClass::Wizard
            | CharacterClass::Sorcerer
            | CharacterClass::Cleric
            | CharacterClass::Druid
            | CharacterClass::Bard
            | CharacterClass::Mage => Self::full_caster_slots(level),
            // Half casters
            CharacterClass::Paladin | CharacterClass::Ranger => Self::half_caster_slots(level),
            // Warlock pact magic
            CharacterClass::Warlock => Self::warlock_slots(level),
            // Non-casters get no slots
            CharacterClass::Warrior
            | CharacterClass::Fighter
            | CharacterClass::Barbarian
            | CharacterClass::Monk
            | CharacterClass::Rogue => [0; 9],
        };
        Self { max_slots, used_slots: [0; 9] }
    }

    /// Returns true if the entity can cast a spell of the given level.
    ///
    /// Cantrips (level 0) always return true.
    pub fn can_cast(&self, spell_level: u8) -> bool {
        if spell_level == 0 {
            return true;
        }
        let idx = spell_level as usize;
        if idx >= 9 { return false; }
        self.used_slots[idx] < self.max_slots[idx]
    }

    /// Expend one slot of the given spell level. No-op for cantrips.
    pub fn expend(&mut self, spell_level: u8) {
        if spell_level > 0 {
            let idx = spell_level as usize;
            if idx < 9 {
                self.used_slots[idx] = self.used_slots[idx].saturating_add(1);
            }
        }
    }

    /// Restore all expended slots (long rest).
    pub fn long_rest(&mut self) {
        self.used_slots = [0; 9];
    }
}

// ── Spellbook ─────────────────────────────────────────────────────────────────

/// Known spells for a character or NPC — stored by name for look-up via `find_spell`.
#[derive(Component, Clone, Default)]
pub struct Spellbook {
    pub known: Vec<&'static str>,
}

impl Spellbook {
    /// Default spellbook for a class (issue #125).
    pub fn for_class(class: &CharacterClass) -> Self {
        let known: &[&'static str] = match class {
            CharacterClass::Mage | CharacterClass::Wizard => {
                &["Fire Bolt", "Ray of Frost", "Magic Missile", "Burning Hands", "Fireball"]
            }
            CharacterClass::Sorcerer => {
                &["Fire Bolt", "Ray of Frost", "Magic Missile", "Scorching Ray"]
            }
            CharacterClass::Cleric => {
                &["Sacred Flame", "Cure Wounds", "Thunderwave"]
            }
            CharacterClass::Druid => {
                &["Shatter", "Cure Wounds", "Thunderwave"]
            }
            CharacterClass::Bard => {
                &["Ray of Frost", "Cure Wounds", "Shatter"]
            }
            CharacterClass::Paladin => {
                &["Cure Wounds", "Thunderwave"]
            }
            CharacterClass::Ranger => {
                &["Cure Wounds"]
            }
            CharacterClass::Warlock => {
                &["Eldritch Blast", "Scorching Ray"]
            }
            // Non-casters carry an empty spellbook.
            CharacterClass::Warrior
            | CharacterClass::Fighter
            | CharacterClass::Barbarian
            | CharacterClass::Monk
            | CharacterClass::Rogue => &[],
        };
        Self { known: known.to_vec() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_spell_works() {
        let s = find_spell("Fireball").unwrap();
        assert_eq!(s.level, 3);
        assert_eq!(s.damage_dice_count, 8);
        assert_eq!(s.damage_dice_sides, 6);
    }

    #[test]
    fn find_spell_missing_returns_none() {
        assert!(find_spell("Wish").is_none());
    }

    #[test]
    fn spell_slots_wizard_level_5() {
        let slots = SpellSlots::for_class(&CharacterClass::Wizard, 5);
        assert_eq!(slots.max_slots[1], 4);
        assert_eq!(slots.max_slots[2], 3);
        assert_eq!(slots.max_slots[3], 2);
        assert_eq!(slots.max_slots[4], 0);
    }

    #[test]
    fn spell_slots_can_cast_and_expend() {
        let mut slots = SpellSlots::for_class(&CharacterClass::Wizard, 3);
        assert!(slots.can_cast(1));
        slots.expend(1);
        slots.expend(1);
        slots.expend(1);
        slots.expend(1);
        assert!(!slots.can_cast(1));
        slots.long_rest();
        assert!(slots.can_cast(1));
    }

    #[test]
    fn cantrip_always_castable() {
        let slots = SpellSlots::for_class(&CharacterClass::Fighter, 5);
        assert!(slots.can_cast(0));
    }

    #[test]
    fn warrior_has_no_slots() {
        let slots = SpellSlots::for_class(&CharacterClass::Warrior, 10);
        assert!(!slots.can_cast(1));
    }

    #[test]
    fn warlock_pact_slots() {
        let slots = SpellSlots::for_class(&CharacterClass::Warlock, 5);
        // Level 5 warlock: 2 slots at level 3
        assert_eq!(slots.max_slots[3], 2);
    }

    #[test]
    fn spellbook_wizard_has_fireball() {
        let book = Spellbook::for_class(&CharacterClass::Wizard);
        assert!(book.known.contains(&"Fireball"));
    }

    #[test]
    fn spellbook_cleric_has_cure_wounds() {
        let book = Spellbook::for_class(&CharacterClass::Cleric);
        assert!(book.known.contains(&"Cure Wounds"));
    }

    #[test]
    fn spellbook_fighter_empty() {
        let book = Spellbook::for_class(&CharacterClass::Fighter);
        assert!(book.known.is_empty());
    }
}
