//! Pure combat rule functions: fn(State, dice) -> (State, Vec<Effect>).
//!
//! Dice values are always **injected** — never rolled internally.
//! This makes every function trivially proptest-able and replay-able.

use crate::combat::types::{
    AttackRollResult, CharacterClass, CombatState, CombatantId, CombatantSnapshot, Effect,
    hit_die_for_class, proficiency_bonus,
};
use smol_str::SmolStr;

// ── Attack roll ───────────────────────────────────────────────────────────────

/// Determine hit/miss/crit from an injected d20 roll.
///
/// Uses SRD attack formula: `d20 + ability_mod + proficiency_bonus >= defender.armor_class`.
/// Natural 20 is always a critical hit; natural 1 is always a miss.
/// See `docs/dnd5e-srd-reference.md`.
///
/// `roll` — integer in [1, 20] provided by caller (test or ECS bridge).
pub fn resolve_attack_roll(
    attacker: &CombatantSnapshot,
    defender: &CombatantSnapshot,
    roll: i32,
) -> AttackRollResult {
    // Natural 20: critical hit regardless of AC (SRD §Attack Rolls).
    if roll == 20 {
        return AttackRollResult::CriticalHit;
    }
    // Natural 1: always a miss regardless of modifiers (SRD §Attack Rolls).
    if roll == 1 {
        return AttackRollResult::Miss;
    }
    // Ability modifier for attack rolls by class (SRD §Ability Score).
    let ability_mod = match attacker.class {
        // DEX-primary classes
        CharacterClass::Rogue | CharacterClass::Monk | CharacterClass::Ranger => attacker.dex_mod(),
        // INT-primary spellcasters
        CharacterClass::Mage | CharacterClass::Wizard => CombatantSnapshot::modifier(attacker.stats.intellect),
        // CHA-primary spellcasters
        CharacterClass::Warlock | CharacterClass::Bard | CharacterClass::Sorcerer
            => CombatantSnapshot::modifier(attacker.stats.charisma),
        // WIS-primary spellcasters
        CharacterClass::Cleric | CharacterClass::Druid
            => CombatantSnapshot::modifier(attacker.stats.wisdom),
        // STR-primary melee
        CharacterClass::Warrior | CharacterClass::Fighter
        | CharacterClass::Paladin | CharacterClass::Barbarian
            => attacker.str_mod(),
    };
    let prof = proficiency_bonus(attacker.level);
    if roll + ability_mod + prof >= defender.armor_class {
        AttackRollResult::Hit
    } else {
        AttackRollResult::Miss
    }
}

// ── Damage resolution ─────────────────────────────────────────────────────────

/// Calculate effects from a successful roll.
///
/// `dmg_roll` — raw damage die value provided by caller.
///
/// Damage modifier is class-appropriate:
/// - Warrior: STR modifier (melee weapon attacks)
/// - Rogue: DEX modifier (finesse weapons)
/// - Mage: INT modifier (spell attacks)
pub fn resolve_damage(
    result: &AttackRollResult,
    attacker: &CombatantSnapshot,
    defender: &CombatantSnapshot,
    dmg_roll: i32,
) -> Vec<Effect> {
    let ability_mod = match attacker.class {
        // DEX-primary
        CharacterClass::Rogue | CharacterClass::Monk | CharacterClass::Ranger => attacker.dex_mod(),
        // INT spellcasters
        CharacterClass::Mage | CharacterClass::Wizard => CombatantSnapshot::modifier(attacker.stats.intellect),
        // CHA spellcasters
        CharacterClass::Warlock | CharacterClass::Bard | CharacterClass::Sorcerer
            => CombatantSnapshot::modifier(attacker.stats.charisma),
        // WIS spellcasters
        CharacterClass::Cleric | CharacterClass::Druid
            => CombatantSnapshot::modifier(attacker.stats.wisdom),
        // STR melee
        CharacterClass::Warrior | CharacterClass::Fighter
        | CharacterClass::Paladin | CharacterClass::Barbarian
            => attacker.str_mod(),
    };
    match result {
        AttackRollResult::Miss => vec![],
        AttackRollResult::Hit => {
            // No flat damage reduction in 5e — AC already determined whether we hit.
            let amount = (dmg_roll + ability_mod).max(1);
            vec![Effect::TakeDamage {
                target: defender.id.clone(),
                amount,
            }]
        }
        AttackRollResult::CriticalHit => {
            // Crit doubles the die (not the modifier), ignores armour.
            let raw = (dmg_roll * 2 + ability_mod).max(1);
            vec![Effect::TakeDamage {
                target: defender.id.clone(),
                amount: raw,
            }]
        }
    }
}

// ── Effect application ────────────────────────────────────────────────────────

/// Apply a list of effects to `state`. Returns the updated state and any
/// secondary effects (e.g. `Die` when health reaches 0).
pub fn apply_effects(
    mut state: CombatState,
    effects: Vec<Effect>,
) -> (CombatState, Vec<Effect>) {
    let mut secondary = Vec::new();

    for effect in effects {
        match &effect {
            Effect::TakeDamage { target, amount } => {
                if let Some(c) = state.get_mut(target) {
                    c.health = (c.health - amount).max(0);
                    if c.health == 0 {
                        secondary.push(Effect::Die { target: target.clone() });
                    }
                }
            }
            Effect::HealDamage { target, amount } => {
                if let Some(c) = state.get_mut(target) {
                    c.health = (c.health + amount).min(c.snapshot.health_max);
                }
            }
            Effect::ApplyStatus { target, status } => {
                if let Some(c) = state.get_mut(target)
                    && !c.statuses.contains(status)
                {
                    c.statuses.push(status.clone());
                }
            }
            Effect::Die { .. } => {
                // Die effect is just a marker; ECS bridge handles despawn.
            }
        }
    }

    (state, secondary)
}

// ── Convenience: full round ───────────────────────────────────────────────────

/// Resolve one attack round end-to-end.
///
/// Returns the updated state and all emitted effects (including Die).
pub fn resolve_round(
    state: CombatState,
    attacker_id: &CombatantId,
    defender_id: &CombatantId,
    attack_roll: i32,
    dmg_roll: i32,
) -> (CombatState, Vec<Effect>) {
    let attacker = match state.get(attacker_id) {
        Some(c) => c.snapshot.clone(),
        None => return (state, vec![]),
    };
    let defender = match state.get(defender_id) {
        Some(c) => c.snapshot.clone(),
        None => return (state, vec![]),
    };

    let roll_result = resolve_attack_roll(&attacker, &defender, attack_roll);
    let effects = resolve_damage(&roll_result, &attacker, &defender, dmg_roll);
    let all_effects = effects.clone();
    let (state, secondary) = apply_effects(state, effects);

    let mut all = all_effects;
    all.extend(secondary);
    (state, all)
}

// ── Ability resolution ────────────────────────────────────────────────────────

/// Resolve an activated ability.
///
/// `rolls` — pre-rolled dice injected by the ECS bridge (never rolled here).
/// Layout for ability 1 (StrongAttack): `[attack_d20, dmg_d8_1, dmg_d8_2]`.
/// Unknown `ability_id` values return an empty effect list.
pub fn resolve_ability(
    ability_id: u8,
    caster: &CombatantSnapshot,
    target: &CombatantSnapshot,
    rolls: &[i32],
) -> Vec<Effect> {
    match ability_id {
        1 => {
            // StrongAttack: 2×d8 damage + apply "weakened" status on hit.
            let attack_roll = rolls.first().copied().unwrap_or(1);
            let dmg1 = rolls.get(1).copied().unwrap_or(1);
            let dmg2 = rolls.get(2).copied().unwrap_or(1);
            let roll_result = resolve_attack_roll(caster, target, attack_roll);
            let mut effects = resolve_damage(&roll_result, caster, target, dmg1 + dmg2);
            if matches!(roll_result, AttackRollResult::Hit | AttackRollResult::CriticalHit) {
                effects.push(Effect::ApplyStatus {
                    target: target.id.clone(),
                    status: SmolStr::new("weakened"),
                });
            }
            effects
        }
        2 => {
            // SneakAttack (Rogue): single d6 stab with bonus damage on hit.
            // Rolls layout: [attack_d20, dmg_d6, bonus_d6].
            let attack_roll = rolls.first().copied().unwrap_or(1);
            let dmg1 = rolls.get(1).copied().unwrap_or(1);
            let dmg2 = rolls.get(2).copied().unwrap_or(1);
            let roll_result = resolve_attack_roll(caster, target, attack_roll);
            let mut effects = resolve_damage(&roll_result, caster, target, dmg1 + dmg2);
            // On hit, apply "poisoned" status representing the rogue's toxin.
            if matches!(roll_result, AttackRollResult::Hit | AttackRollResult::CriticalHit) {
                effects.push(Effect::ApplyStatus {
                    target: target.id.clone(),
                    status: SmolStr::new("poisoned"),
                });
            }
            effects
        }
        3 => {
            // ArcaneBlast (Mage): ranged spell, no attack roll — auto-hits for d8+INT mod,
            // and ignores armour (flat damage). Crits double the die as usual.
            // Rolls layout: [dmg_d8].
            let dmg_roll = rolls.first().copied().unwrap_or(1);
            let int_mod   = CombatantSnapshot::modifier(caster.stats.intellect);
            let amount    = (dmg_roll + int_mod).max(1);
            vec![
                Effect::TakeDamage { target: target.id.clone(), amount },
                Effect::ApplyStatus {
                    target: target.id.clone(),
                    status: SmolStr::new("scorched"),
                },
            ]
        }
        4 => vec![], // reserved
        5 => {
            // BossRage (Phase2): heavy attack with a flat +3 bonus to damage,
            // applies "enraged" self-buff to the caster so the bridge can boost later rolls.
            // Rolls layout: [attack_d20, dmg_d10].
            let attack_roll = rolls.first().copied().unwrap_or(1);
            let dmg_roll    = rolls.get(1).copied().unwrap_or(1);
            let roll_result = resolve_attack_roll(caster, target, attack_roll);
            let mut effects = resolve_damage(&roll_result, caster, target, dmg_roll + 3);
            if matches!(roll_result, AttackRollResult::Hit | AttackRollResult::CriticalHit) {
                // Self-buff: caster gains "enraged" status (ECS bridge may apply bonuses).
                effects.push(Effect::ApplyStatus {
                    target: caster.id.clone(),
                    status: SmolStr::new("enraged"),
                });
            }
            effects
        }
        6 => {
            // BossFrenzy (Phase3): two rapid strikes at reduced individual damage.
            // Rolls layout: [attack_d20_1, dmg_d6_1, attack_d20_2, dmg_d6_2].
            let mut effects = Vec::new();
            for strike in 0..2_usize {
                let a_roll = rolls.get(strike * 2).copied().unwrap_or(1);
                let d_roll = rolls.get(strike * 2 + 1).copied().unwrap_or(1);
                let roll_result = resolve_attack_roll(caster, target, a_roll);
                effects.extend(resolve_damage(&roll_result, caster, target, d_roll));
            }
            // On any damage landed, apply "weakened" to the target.
            let dealt_damage = effects.iter().any(|e| matches!(e, Effect::TakeDamage { .. }));
            if dealt_damage {
                effects.push(Effect::ApplyStatus {
                    target: target.id.clone(),
                    status: SmolStr::new("weakened"),
                });
            }
            effects
        }
        7 => {
            // HealAlly (Cleric/Druid): restore 1d8 + WIS mod HP to the caster.
            // The ECS bridge targets the lowest-HP ally; here we heal the "target" passed in.
            // Rolls layout: [dmg_d8].
            let heal_roll = rolls.first().copied().unwrap_or(1);
            let wis_mod = CombatantSnapshot::modifier(caster.stats.wisdom);
            let amount = (heal_roll + wis_mod).max(1);
            vec![Effect::HealDamage {
                target: target.id.clone(),
                amount,
            }]
        }
        8 => {
            // RageEntry (Barbarian): +2 flat damage on next hit, gains "raging" status.
            // Rolls layout: [attack_d20, dmg_d12].
            let attack_roll = rolls.first().copied().unwrap_or(1);
            let dmg_roll    = rolls.get(1).copied().unwrap_or(1);
            let roll_result = resolve_attack_roll(caster, target, attack_roll);
            let mut effects = resolve_damage(&roll_result, caster, target, dmg_roll + 2);
            effects.push(Effect::ApplyStatus {
                target: caster.id.clone(),
                status: SmolStr::new("raging"),
            });
            effects
        }
        9 => {
            // DefensiveStance (Fighter at < 50 % HP): grants "defending" self-buff.
            // Rolls layout: [] (no attack — pure buff).
            vec![Effect::ApplyStatus {
                target: caster.id.clone(),
                status: SmolStr::new("defending"),
            }]
        }
        _ => vec![],
    }
}

// ── Spell resolution ──────────────────────────────────────────────────────────

/// Resolve a spell cast by looking it up in the `SPELLS` catalogue.
///
/// `rolls` — pre-rolled dice injected by the ECS bridge:
///   - Indices `[0..dice_count)` are the damage/heal dice.
///   - The final element (if `save_ability` is set) is the target's saving throw d20.
///
/// Returns a list of `Effect`s (TakeDamage, HealDamage, or empty on save success).
pub fn resolve_spell(
    spell_name: &str,
    _caster: &CombatantSnapshot,
    target: &CombatantSnapshot,
    rolls: &[i32],
) -> Vec<Effect> {
    use crate::combat::spells::find_spell;

    let Some(spell) = find_spell(spell_name) else {
        return vec![];
    };

    // ── Healing spells ───────────────────────────────────────────────────────
    if spell.heal_dice_count > 0 && spell.damage_dice_count == 0 {
        let total_heal: i32 = rolls.iter()
            .take(spell.heal_dice_count as usize)
            .sum::<i32>()
            .max(1);
        return vec![Effect::HealDamage {
            target: target.id.clone(),
            amount: total_heal,
        }];
    }

    // ── Damage spells ────────────────────────────────────────────────────────
    let raw_damage: i32 = rolls.iter()
        .take(spell.damage_dice_count as usize)
        .sum::<i32>()
        .max(1);

    // Saving throw: if the spell has one, check the last element of rolls.
    let mut damage = raw_damage;
    if let Some(save_ability_idx) = spell.save_ability {
        let save_roll = rolls.last().copied().unwrap_or(1);
        let ability_score = match save_ability_idx {
            0 => target.stats.strength,
            1 => target.stats.dexterity,
            2 => target.stats.constitution,
            3 => target.stats.intellect,
            4 => target.stats.wisdom,
            _ => target.stats.charisma,
        };
        let dc = spell.save_dc_base as i32;
        let saved = resolve_saving_throw(ability_score, false, proficiency_bonus(target.level), dc, save_roll);
        if saved {
            damage /= 2; // half damage on successful save (SRD §Saving Throws)
        }
    }

    vec![Effect::TakeDamage {
        target: target.id.clone(),
        amount: damage.max(1),
    }]
}

// ── Saving throw resolution ───────────────────────────────────────────────────

/// Resolve a D&D 5e SRD saving throw.
///
/// Formula: `d20 + ability_modifier + (proficiency_bonus if proficient) >= dc`
///
/// `roll` — injected d20 value [1, 20] (never rolled here — callers inject it).
/// Returns `true` if the save succeeds (defender resists the effect).
///
/// See `docs/dnd5e-srd-reference.md`.
pub fn resolve_saving_throw(
    ability_score: i32,
    proficient: bool,
    proficiency_bonus: i32,
    dc: i32,
    roll: i32,
) -> bool {
    let ability_mod = CombatantSnapshot::modifier(ability_score);
    let prof_bonus = if proficient { proficiency_bonus } else { 0 };
    roll + ability_mod + prof_bonus >= dc
}

// ── Progression helpers ───────────────────────────────────────────────────────

/// XP required to advance from `level` to `level + 1` (SRD table).
/// Returns `u32::MAX` for level 20+ (no further advancement).
/// See `docs/dnd5e-srd-reference.md`.
pub fn xp_to_next_level(level: u32) -> u32 {
    match level {
        1  =>     300,
        2  =>     600,
        3  =>   1_800,
        4  =>   3_800,
        5  =>   7_500,
        6  =>   9_000,
        7  =>  11_000,
        8  =>  14_000,
        9  =>  16_000,
        10 =>  21_000,
        11 =>  15_000,
        12 =>  20_000,
        13 =>  20_000,
        14 =>  25_000,
        15 =>  30_000,
        16 =>  30_000,
        17 =>  40_000,
        18 =>  40_000,
        19 =>  50_000,
        _  => u32::MAX, // level 20: no further advancement
    }
}

/// Roll a D&D 5e saving throw.
/// Returns the total: d20 + ability_modifier + (proficiency_bonus if proficient).
pub fn roll_saving_throw(
    ability_mod: i32,
    proficient: bool,
    level: u32,
    rng: &mut impl rand::Rng,
) -> i32 {
    use rand::RngExt as _;
    let d20: i32 = rng.random_range(1..=20_i32);
    let prof = if proficient { proficiency_bonus(level) } else { 0 };
    d20 + ability_mod + prof
}

/// HP gained on level-up: roll the class hit die (provided via `rng`) + CON mod, min 1.
///
/// `rng` must yield values in `[1, hit_die_for_class(class)]`. The roll is clamped to
/// that range internally so out-of-bounds injected values cannot produce invalid results.
pub fn hp_on_level_up(
    class: &CharacterClass,
    con_mod: i32,
    rng: &mut impl Iterator<Item = i32>,
) -> i32 {
    let max_die = hit_die_for_class(class);
    let die_roll = rng.next().unwrap_or(1).clamp(1, max_die);
    (die_roll + con_mod).max(1)
}

/// Calculate total max HP across all levels using the SRD average method.
///
/// - Level 1: `max_die + con_mod` (always maximum per SRD), minimum 1.
/// - Level 2+: `(max_die / 2 + 1) + con_mod` (average rounded up) per level, minimum 1.
///
/// `die` is the hit die face count (e.g. 10 for d10); `level` is character level.
pub fn calculate_max_hp(die: u8, level: u32, con_mod: i32) -> i32 {
    if level == 0 {
        return 0;
    }
    let max_die = die as i32;
    let average = (max_die / 2) + 1;
    // Level 1 always uses the full die value.
    let level1 = (max_die + con_mod).max(1);
    // Remaining levels use the average rounded up.
    let remaining = level.saturating_sub(1) as i32;
    let additional = remaining * (average + con_mod).max(1);
    level1 + additional
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::types::{CharacterClass, CombatantSnapshot, CombatantState, CoreStats};
    use uuid::Uuid;

    fn make_snapshot(id: Uuid, hp: i32, armor_class: i32) -> CombatantSnapshot {
        CombatantSnapshot {
            id: CombatantId(id),
            faction: None,
            class: CharacterClass::Warrior,
            stats: CoreStats::default(),
            health_current: hp,
            health_max: hp,
            level: 1,
            armor_class,
        }
    }

    fn make_state(attacker_hp: i32, defender_hp: i32) -> (CombatState, CombatantId, CombatantId) {
        let aid = Uuid::new_v4();
        let did = Uuid::new_v4();
        let state = CombatState {
            combatants: vec![
                // Attacker: AC 10 (unarmored), defender: AC 12 (leather)
                CombatantState::new(make_snapshot(aid, attacker_hp, 10)),
                CombatantState::new(make_snapshot(did, defender_hp, 12)),
            ],
            round: 0,
        };
        (state, CombatantId(aid), CombatantId(did))
    }

    // ── Attack roll tests ─────────────────────────────────────────────────────

    #[test]
    fn natural_20_always_crits() {
        let (state, aid, did) = make_state(20, 20);
        let (_, effects) = resolve_round(state, &aid, &did, 20, 5);
        // Crit: 5*2 + str_mod(0) = 10 damage
        assert!(effects.iter().any(|e| matches!(e, Effect::TakeDamage { amount, .. } if *amount == 10)));
    }

    #[test]
    fn natural_1_always_misses() {
        let (state, aid, did) = make_state(20, 20);
        let (_, effects) = resolve_round(state, &aid, &did, 1, 5);
        assert!(!effects.iter().any(|e| matches!(e, Effect::TakeDamage { .. })));
    }

    #[test]
    fn ac_check_with_proficiency() {
        // L1 attacker: STR 10 (mod 0), proficiency +2. Defender AC 12.
        // Roll 10: 10 + 0 + 2 = 12 >= 12 → hit.
        // Roll 9:  9 + 0 + 2 = 11 < 12  → miss.
        let aid = Uuid::new_v4();
        let did = Uuid::new_v4();
        let attacker = make_snapshot(aid, 20, 10);
        let defender = make_snapshot(did, 20, 12);
        assert_eq!(resolve_attack_roll(&attacker, &defender, 10), AttackRollResult::Hit);
        assert_eq!(resolve_attack_roll(&attacker, &defender, 9),  AttackRollResult::Miss);
    }

    #[test]
    fn no_dr_on_hit_damage_equals_roll_plus_mod() {
        let aid = Uuid::new_v4();
        let did = Uuid::new_v4();
        let attacker = make_snapshot(aid, 20, 10);
        // Defender has high AC but we pass a guaranteed-hit result directly
        let defender = make_snapshot(did, 50, 20);
        let effects = resolve_damage(&AttackRollResult::Hit, &attacker, &defender, 7);
        let damage: i32 = effects.iter()
            .filter_map(|e| if let Effect::TakeDamage { amount, .. } = e { Some(*amount) } else { None })
            .sum();
        // STR 10 → mod 0; expected = (7 + 0).max(1) = 7; no DR subtracted
        assert_eq!(damage, 7);
    }

    #[test]
    fn lethal_damage_emits_die() {
        let (state, aid, did) = make_state(20, 1);
        let (_, effects) = resolve_round(state, &aid, &did, 20, 1);
        assert!(effects.iter().any(|e| matches!(e, Effect::Die { .. })));
    }

    #[test]
    fn health_never_negative() {
        let (state, aid, did) = make_state(20, 5);
        let (next, _) = resolve_round(state, &aid, &did, 20, 100);
        let defender_health = next.combatants.iter()
            .find(|c| c.snapshot.id == did)
            .map(|c| c.health)
            .unwrap_or(0);
        assert_eq!(defender_health, 0);
    }

    // ── XP table tests ────────────────────────────────────────────────────────

    #[test]
    fn xp_table_spot_checks() {
        assert_eq!(xp_to_next_level(1),  300);
        assert_eq!(xp_to_next_level(4),  3_800);
        assert_eq!(xp_to_next_level(9),  16_000);
        assert_eq!(xp_to_next_level(19), 50_000);
        assert_eq!(xp_to_next_level(20), u32::MAX);
    }

    #[test]
    fn proficiency_bonus_table() {
        for level in 1..=4  { assert_eq!(proficiency_bonus(level), 2, "level {level}"); }
        for level in 5..=8  { assert_eq!(proficiency_bonus(level), 3, "level {level}"); }
        for level in 9..=12 { assert_eq!(proficiency_bonus(level), 4, "level {level}"); }
        for level in 13..=16 { assert_eq!(proficiency_bonus(level), 5, "level {level}"); }
        for level in 17..=20 { assert_eq!(proficiency_bonus(level), 6, "level {level}"); }
    }

    // ── HP on level-up tests ──────────────────────────────────────────────────

    #[test]
    fn hp_on_level_up_minimum_one() {
        // CON mod -5, roll 1: (1 + -5).max(1) = 1
        let mut rng = std::iter::once(1);
        let hp = hp_on_level_up(&CharacterClass::Warrior, -5, &mut rng);
        assert_eq!(hp, 1);
    }

    #[test]
    fn hp_warrior_vs_mage_same_roll() {
        // Roll 8: Warrior clamps to 8 (d10 max=10), Mage clamps to 6 (d6 max=6)
        let warrior_hp = hp_on_level_up(&CharacterClass::Warrior, 0, &mut std::iter::once(8));
        let mage_hp    = hp_on_level_up(&CharacterClass::Mage,    0, &mut std::iter::once(8));
        assert!(warrior_hp >= mage_hp, "warrior={warrior_hp}, mage={mage_hp}");
    }

    // ── calculate_max_hp tests ────────────────────────────────────────────────

    #[test]
    fn max_hp_level_zero_is_zero() {
        assert_eq!(calculate_max_hp(10, 0, 0), 0);
    }

    #[test]
    fn max_hp_level1_warrior_no_con() {
        // Warrior d10, CON mod 0: level 1 = max die = 10
        assert_eq!(calculate_max_hp(10, 1, 0), 10);
    }

    #[test]
    fn max_hp_level1_warrior_positive_con() {
        // Warrior d10, CON mod +2: level 1 = 10 + 2 = 12
        assert_eq!(calculate_max_hp(10, 1, 2), 12);
    }

    #[test]
    fn max_hp_level2_warrior_no_con() {
        // Warrior d10, CON mod 0: level 1 = 10, level 2 = average(d10) = 6 → total 16
        assert_eq!(calculate_max_hp(10, 2, 0), 16);
    }

    #[test]
    fn max_hp_level2_warrior_positive_con() {
        // Warrior d10, CON +2: level 1 = 12, level 2 = (6+2) = 8 → total 20
        assert_eq!(calculate_max_hp(10, 2, 2), 20);
    }

    #[test]
    fn max_hp_level1_mage_no_con() {
        // Mage d6, CON mod 0: level 1 = 6
        assert_eq!(calculate_max_hp(6, 1, 0), 6);
    }

    #[test]
    fn max_hp_level2_mage_no_con() {
        // Mage d6, CON 0: level 1 = 6, level 2 = average(d6) = 4 → total 10
        assert_eq!(calculate_max_hp(6, 2, 0), 10);
    }

    #[test]
    fn max_hp_minimum_one_per_level() {
        // d6, CON mod -5: level 1 = (6-5).max(1) = 1; level 2 = (4-5).max(1) = 1 → total 2
        assert_eq!(calculate_max_hp(6, 2, -5), 2);
    }

    #[test]
    fn max_hp_barbarian_d12_level3_no_con() {
        // Barbarian d12, CON 0: level 1 = 12, level 2 = 7, level 3 = 7 → total 26
        assert_eq!(calculate_max_hp(12, 3, 0), 26);
    }

    // ── Ability tests ─────────────────────────────────────────────────────────

    #[test]
    fn strong_attack_deals_more_damage_than_basic_on_same_roll() {
        let (state, caster_id, target_id) = make_state(20, 100);
        let caster = state.get(&caster_id).unwrap().snapshot.clone();
        let target = state.get(&target_id).unwrap().snapshot.clone();

        // Basic: 4 damage; Strong: 4+4=8 damage (both hit)
        let basic_effects  = resolve_damage(&AttackRollResult::Hit, &caster, &target, 4);
        let strong_effects = resolve_ability(1, &caster, &target, &[15, 4, 4]);

        let sum_dmg = |effects: &[Effect]| -> i32 {
            effects.iter()
                .filter_map(|e| if let Effect::TakeDamage { amount, .. } = e { Some(*amount) } else { None })
                .sum()
        };
        assert!(sum_dmg(&strong_effects) > sum_dmg(&basic_effects));
    }

    #[test]
    fn strong_attack_applies_weakened_status_on_hit() {
        let (state, aid, did) = make_state(20, 100);
        let caster = state.get(&aid).unwrap().snapshot.clone();
        let target = state.get(&did).unwrap().snapshot.clone();
        let effects = resolve_ability(1, &caster, &target, &[15, 4, 4]);
        assert!(effects.iter().any(|e| matches!(e,
            Effect::ApplyStatus { status, .. } if status == "weakened"
        )));
    }

    #[test]
    fn unknown_ability_id_returns_no_effects() {
        let (state, aid, did) = make_state(20, 100);
        let caster = state.get(&aid).unwrap().snapshot.clone();
        let target = state.get(&did).unwrap().snapshot.clone();
        let effects = resolve_ability(99, &caster, &target, &[15, 4, 4]);
        assert!(effects.is_empty());
    }

    // ── Rogue SneakAttack (ability 2) tests ───────────────────────────────────

    fn make_rogue_snapshot(id: Uuid) -> CombatantSnapshot {
        CombatantSnapshot {
            id: CombatantId(id),
            faction: None,
            class: CharacterClass::Rogue,
            stats: CoreStats { dexterity: 16, ..CoreStats::default() }, // DEX 16 → mod +3
            health_current: 20,
            health_max: 20,
            level: 1,
            armor_class: 12,
        }
    }

    fn make_mage_snapshot(id: Uuid) -> CombatantSnapshot {
        CombatantSnapshot {
            id: CombatantId(id),
            faction: None,
            class: CharacterClass::Mage,
            stats: CoreStats { intellect: 18, ..CoreStats::default() }, // INT 18 → mod +4
            health_current: 10,
            health_max: 10,
            level: 1,
            armor_class: 10,
        }
    }

    #[test]
    fn sneak_attack_applies_poisoned_on_hit() {
        let rogue_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let caster = make_rogue_snapshot(rogue_id);
        let target = make_snapshot(target_id, 50, 10);
        // Roll 15: guaranteed hit (15 + DEX mod 3 + prof 2 = 20 >= AC 10)
        let effects = resolve_ability(2, &caster, &target, &[15, 3, 3]);
        assert!(effects.iter().any(|e| matches!(e,
            Effect::ApplyStatus { status, .. } if status == "poisoned"
        )));
    }

    #[test]
    fn sneak_attack_deals_double_die_damage_on_hit() {
        let rogue_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let caster = make_rogue_snapshot(rogue_id);
        let target = make_snapshot(target_id, 50, 10);
        // Rolls: attack 15 (hit), dmg_d6=4, bonus_d6=4 → 4+4+DEX mod(3) = 11
        let effects = resolve_ability(2, &caster, &target, &[15, 4, 4]);
        let total_damage: i32 = effects.iter()
            .filter_map(|e| if let Effect::TakeDamage { amount, .. } = e { Some(*amount) } else { None })
            .sum();
        // 4+4 roll + DEX mod 3 = 11
        assert_eq!(total_damage, 11);
    }

    #[test]
    fn sneak_attack_no_poison_on_miss() {
        let rogue_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let caster = make_rogue_snapshot(rogue_id);
        // High AC so natural roll 5 still misses: 5 + 3 + 2 = 10 < 20
        let target = make_snapshot(target_id, 50, 20);
        let effects = resolve_ability(2, &caster, &target, &[5, 4, 4]);
        assert!(!effects.iter().any(|e| matches!(e, Effect::ApplyStatus { .. })));
    }

    // ── Mage ArcaneBlast (ability 3) tests ───────────────────────────────────

    #[test]
    fn arcane_blast_always_hits_and_applies_scorched() {
        let mage_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let caster = make_mage_snapshot(mage_id);
        let target = make_snapshot(target_id, 50, 30); // AC 30 — never hittable by normal attack
        let effects = resolve_ability(3, &caster, &target, &[5]);
        // Should deal damage even to AC 30 target (auto-hit)
        assert!(effects.iter().any(|e| matches!(e, Effect::TakeDamage { .. })));
        assert!(effects.iter().any(|e| matches!(e,
            Effect::ApplyStatus { status, .. } if status == "scorched"
        )));
    }

    #[test]
    fn arcane_blast_damage_includes_int_modifier() {
        let mage_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let caster = make_mage_snapshot(mage_id); // INT 18 → mod +4
        let target = make_snapshot(target_id, 50, 10);
        // Roll 6: expected damage = (6 + 4).max(1) = 10
        let effects = resolve_ability(3, &caster, &target, &[6]);
        let total: i32 = effects.iter()
            .filter_map(|e| if let Effect::TakeDamage { amount, .. } = e { Some(*amount) } else { None })
            .sum();
        assert_eq!(total, 10);
    }

    // ── Saving throw tests ────────────────────────────────────────────────────

    #[test]
    fn saving_throw_succeeds_on_exact_dc() {
        // STR 14 (mod +2), not proficient, DC 12, roll 10: 10 + 2 + 0 = 12 >= 12 → pass
        assert!(resolve_saving_throw(14, false, 2, 12, 10));
    }

    #[test]
    fn saving_throw_fails_one_below_dc() {
        // STR 14 (mod +2), not proficient, DC 12, roll 9: 9 + 2 + 0 = 11 < 12 → fail
        assert!(!resolve_saving_throw(14, false, 2, 12, 9));
    }

    #[test]
    fn saving_throw_proficiency_makes_difference() {
        // STR 10 (mod 0), prof +2, DC 12, roll 9: 9 + 0 + 2 = 11 → fail without prof
        // With prof: 9 + 0 + 2 = 11 → still fails at DC 12
        // Use roll 10: without prof 10 + 0 + 0 = 10 < 12 → fail; with prof 10 + 0 + 2 = 12 → pass
        assert!(!resolve_saving_throw(10, false, 2, 12, 10));
        assert!( resolve_saving_throw(10, true,  2, 12, 10));
    }

    // ── Class ability modifier tests ──────────────────────────────────────────

    #[test]
    fn rogue_uses_dex_for_attack_roll() {
        let rogue_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let caster = make_rogue_snapshot(rogue_id); // DEX 16 → mod +3
        let target = make_snapshot(target_id, 50, 15); // AC 15
        // Roll 10: 10 + DEX mod 3 + prof 2 = 15 ≥ 15 → hit
        assert_eq!(resolve_attack_roll(&caster, &target, 10), AttackRollResult::Hit);
        // Roll 9:  9 + 3 + 2 = 14 < 15 → miss
        assert_eq!(resolve_attack_roll(&caster, &target, 9), AttackRollResult::Miss);
    }

    #[test]
    fn mage_uses_int_for_attack_roll() {
        let mage_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let caster = make_mage_snapshot(mage_id); // INT 18 → mod +4
        let target = make_snapshot(target_id, 50, 16); // AC 16
        // Roll 10: 10 + INT mod 4 + prof 2 = 16 ≥ 16 → hit
        assert_eq!(resolve_attack_roll(&caster, &target, 10), AttackRollResult::Hit);
        // Roll 9:  9 + 4 + 2 = 15 < 16 → miss
        assert_eq!(resolve_attack_roll(&caster, &target, 9), AttackRollResult::Miss);
    }
}
