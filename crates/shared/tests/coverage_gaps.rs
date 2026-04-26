//! Coverage gap tests: edge cases and cross-system interactions.
//!
//! Tests are grouped into three sections matching the gaps identified in issue #6:
//!   1. Population dynamics — famine / resource depletion
//!   2. War party & conflict resolution — prolonged engagement, friendly fire
//!   3. Inter-systemic dependencies — resource sink, diplomacy failure

use fellytip_shared::{
    combat::{
        rules::resolve_round,
        types::{
            CharacterClass, CombatState, CombatantId, CombatantSnapshot, CombatantState,
            CoreStats, Effect,
        },
    },
    world::{
        ecology::{
            tick_ecology, EcologyEvent, Population, RegionEcology, RegionId, SpeciesId,
            COLLAPSE_THRESHOLD,
        },
        faction::{
            kill_standing_delta, pick_goal, standing_tier, Faction, FactionGoal, FactionId,
            FactionResources, NpcRank, PlayerReputationMap, StandingTier, STANDING_NEUTRAL,
        },
        population::{
            tick_population, PopulationEffect, SettlementPopulation, WAR_PARTY_MILITARY_MIN,
            WAR_PARTY_THRESHOLD,
        },
        zone::WORLD_SURFACE,
    },
};
use std::collections::HashMap;
use uuid::Uuid;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_pop(adult_count: u32, military_strength: f32) -> SettlementPopulation {
    SettlementPopulation {
        settlement_id: Uuid::nil(),
        faction_id: FactionId("test".into()),
        world_id: WORLD_SURFACE,
        birth_ticks: 0,
        adult_count,
        child_count: 0,
        home_x: 0.0,
        home_y: 0.0,
        home_z: 0.0,
        war_party_cooldown: 0,
        military_strength,
    }
}

fn make_faction(food: f32, military: f32, goals: Vec<FactionGoal>) -> Faction {
    Faction {
        id: FactionId("iron_wolves".into()),
        name: "Iron Wolves".into(),
        disposition: HashMap::new(),
        goals,
        resources: FactionResources { food, military_strength: military, gold: 0.0 },
        territory: vec![],
        is_aggressive: false,
        player_default_standing: STANDING_NEUTRAL,
    }
}

fn make_combat_state(
    attacker_id: Uuid,
    attacker_hp: i32,
    attacker_str: i32,
    attacker_ac: i32,
    defender_id: Uuid,
    defender_hp: i32,
    defender_ac: i32,
    faction: Option<FactionId>,
) -> CombatState {
    let attacker = CombatantSnapshot {
        id: CombatantId(attacker_id),
        faction: faction.clone(),
        class: CharacterClass::Warrior,
        stats: CoreStats { strength: attacker_str, ..CoreStats::default() },
        health_current: attacker_hp,
        health_max: attacker_hp,
        level: 1,
        armor_class: attacker_ac,
    };
    let defender = CombatantSnapshot {
        id: CombatantId(defender_id),
        faction: faction.clone(),
        class: CharacterClass::Warrior,
        stats: CoreStats::default(),
        health_current: defender_hp,
        health_max: defender_hp,
        level: 1,
        armor_class: defender_ac,
    };
    CombatState {
        combatants: vec![CombatantState::new(attacker), CombatantState::new(defender)],
        round: 0,
    }
}

// ── 1. Population dynamics: resource depletion / famine ──────────────────────

/// When military_strength drops below WAR_PARTY_MILITARY_MIN (simulating troops
/// unable to function due to starvation), war party formation is blocked even
/// when adult population is above the threshold.
#[test]
fn military_depletion_blocks_war_party() {
    let target = (Uuid::new_v4(), 100.0f32, 100.0f32, 0.0f32);
    let state = make_pop(WAR_PARTY_THRESHOLD, WAR_PARTY_MILITARY_MIN - 0.1);
    let (_, effects) = tick_population(state, &[target], None);
    assert!(
        !effects.iter().any(|e| matches!(e, PopulationEffect::FormWarParty { .. })),
        "depleted military should block war party formation"
    );
}

/// Simulates a 70%+ resource-depletion famine: a fully-staffed settlement
/// (adults=20, military=50) has its military reduced to zero. No war party
/// must form until military is replenished.
#[test]
fn famine_stops_war_party_dispatch_until_recovery() {
    let target = (Uuid::new_v4(), 100.0f32, 100.0f32, 0.0f32);

    // Pre-famine: comfortable military — war party forms.
    let pre_famine = make_pop(20, 50.0);
    let (_, effects) = tick_population(pre_famine, &[target], None);
    assert!(
        effects.iter().any(|e| matches!(e, PopulationEffect::FormWarParty { .. })),
        "war party should form before famine"
    );

    // Famine wipes out military capacity entirely.
    let post_famine = make_pop(20, 0.0);
    let (_, effects) = tick_population(post_famine, &[target], None);
    assert!(
        !effects.iter().any(|e| matches!(e, PopulationEffect::FormWarParty { .. })),
        "war party must NOT form after famine wipes out military"
    );

    // Recovery: military restored to threshold — war party can form again.
    let recovered = make_pop(20, WAR_PARTY_MILITARY_MIN);
    let (_, effects) = tick_population(recovered, &[target], None);
    assert!(
        effects.iter().any(|e| matches!(e, PopulationEffect::FormWarParty { .. })),
        "war party should form again after military recovery"
    );
}

/// Ecology: prey population collapses under sustained high predation pressure
/// (plague / famine analog). Validates that the collapse event fires correctly.
#[test]
fn prey_collapse_under_sustained_predation() {
    let mut state = RegionEcology {
        region: RegionId("valley".into()),
        prey: Population { species: SpeciesId("rabbit".into()), count: 10.0 },
        predator: Population { species: SpeciesId("fox".into()), count: 200.0 },
        r: 0.5,
        k: 200.0,
        alpha: 0.1, // heavy predation
        beta: 0.5,
        delta: 0.1,
    };

    let mut collapse_seen = false;
    for _ in 0..10 {
        let (next, events) = tick_ecology(state);
        state = next;
        if events.iter().any(|e| matches!(e, EcologyEvent::Collapse { .. })) {
            collapse_seen = true;
            break;
        }
    }
    assert!(collapse_seen, "prey should collapse under sustained high predation (plague analog)");
    assert!(state.prey.count < COLLAPSE_THRESHOLD, "prey count should be below collapse threshold");
}

// ── 2. War party & conflict resolution ───────────────────────────────────────

/// A weak defender facing a strong attacker (high STR, guaranteed hits) should
/// accumulate damage across rounds and eventually receive a Die effect.
/// This is the morale-decay analog: prolonged engagement against superior
/// numbers ends in defeat.
#[test]
fn prolonged_engagement_against_superior_force_results_in_death() {
    let aid = Uuid::new_v4();
    let did = Uuid::new_v4();
    // Attacker: STR 20 (mod +5). Defender: 15 HP, AC 10.
    // Roll 15: 15 + 5 (STR mod) + 2 (proficiency) = 22 ≥ 10 (AC) → hit.
    // Damage per round: 4 + 5 = 9. Defender dead after 2 rounds (15 → 6 → 0).
    let mut state = make_combat_state(aid, 100, 20, 10, did, 15, 10, None);
    let attacker_id = CombatantId(aid);
    let defender_id = CombatantId(did);

    let mut death_emitted = false;
    for _ in 0..5 {
        let (next_state, effects) = resolve_round(state, &attacker_id, &defender_id, 15, 4);
        state = next_state;
        if effects.iter().any(|e| matches!(e, Effect::Die { .. })) {
            death_emitted = true;
            break;
        }
    }
    assert!(death_emitted, "defender should die after prolonged engagement with superior attacker");
}

/// Two combatants from the same faction can attack each other without any
/// special penalty or immunity. This confirms that allied combat resolution
/// applies the same rules as hostile combat — no unit is accidentally shielded.
#[test]
fn same_faction_combat_no_special_handling() {
    let aid = Uuid::new_v4();
    let did = Uuid::new_v4();
    let shared_faction = Some(FactionId("iron_wolves".into()));

    // Natural 20 crit guarantees damage regardless of AC or faction.
    // Crit: 4 * 2 + str_mod(10) = 8 damage. Defender HP: 30 → 22.
    let state = make_combat_state(aid, 30, 10, 10, did, 30, 10, shared_faction);
    let attacker_id = CombatantId(aid);
    let defender_id = CombatantId(did);

    let (next_state, effects) = resolve_round(state, &attacker_id, &defender_id, 20, 4);

    assert!(
        effects.iter().any(|e| matches!(e, Effect::TakeDamage { .. })),
        "same-faction attacker must deal damage (no friendly-fire immunity)"
    );
    let defender_hp = next_state
        .combatants
        .iter()
        .find(|c| c.snapshot.id == defender_id)
        .map(|c| c.health)
        .unwrap();
    assert!(defender_hp < 30, "defender HP should be reduced by same-faction attacker");
}

// ── 3. Inter-systemic dependencies ───────────────────────────────────────────

/// Food depletion (resource sink event) shifts the faction's active goal from
/// ExpandTerritory (comfortable) to Survive (famine). Validates that the
/// goal-scoring bridge correctly propagates the FactionResources state change.
#[test]
fn food_depletion_shifts_goal_from_expand_to_survive() {
    let goals = vec![
        FactionGoal::Survive,
        FactionGoal::ExpandTerritory {
            target_region: RegionId("north".into()),
        },
        FactionGoal::RaidResource {
            resource_node_id: "forest_01".into(),
        },
    ];

    // Comfortable: food=100, military=50 → ExpandTerritory wins (score 30.0).
    let comfortable = make_faction(100.0, 50.0, goals.clone());
    assert!(
        matches!(pick_goal(&comfortable), Some(FactionGoal::ExpandTerritory { .. })),
        "comfortable faction should prioritize expansion"
    );

    // Famine: food=5 (>70% depletion from 100 → 5) → Survive wins (score 100.0).
    let starving = make_faction(5.0, 50.0, goals.clone());
    assert!(
        matches!(pick_goal(&starving), Some(FactionGoal::Survive)),
        "starving faction must prioritize survival after 70%+ food depletion"
    );
}

/// Diplomacy failure sequence: neutral player kills grunts → standing drops to
/// Hostile tier → is_aggressive() returns true → faction attacks rather than
/// negotiates.
///
/// Tests the chain: KillGrunt × 10 → StandingTier::Hostile → is_aggressive == true.
#[test]
fn diplomacy_failure_sequence_hostile_standing_triggers_aggression() {
    let mut rep = PlayerReputationMap::default();
    let player = Uuid::new_v4();
    let faction = FactionId("iron_wolves".into());

    // Start neutral.
    assert_eq!(standing_tier(rep.score(player, &faction)), StandingTier::Neutral);

    // 10 grunt kills × -50 each = -500 → Hostile (threshold is -500).
    for _ in 0..10 {
        rep.apply_delta(player, &faction, kill_standing_delta(NpcRank::Grunt));
    }

    let score = rep.score(player, &faction);
    let tier = standing_tier(score);
    assert_eq!(
        tier,
        StandingTier::Hostile,
        "10 grunt kills should reach Hostile tier; score={score}"
    );
    assert!(
        tier.is_aggressive(),
        "Hostile tier must trigger aggression (attack, not negotiate)"
    );
}

/// Resource sink → war party pressure: low food causes the faction to prefer
/// RaidResource over diplomacy, AND the population system can form a war party
/// when military meets the threshold.
///
/// Validates both systems respond consistently to the same resource shortage.
#[test]
fn resource_sink_drives_raid_over_diplomacy() {
    // food=20 (low), military=25 → RaidResource (40.0) beats FormAlliance (15.0).
    let goals = vec![
        FactionGoal::Survive,
        FactionGoal::RaidResource {
            resource_node_id: "granary_01".into(),
        },
        FactionGoal::FormAlliance {
            with: FactionId("stone_hand".into()),
            min_trust: 0.5,
        },
    ];
    let hungry_faction = make_faction(20.0, 25.0, goals);

    assert!(
        matches!(pick_goal(&hungry_faction), Some(FactionGoal::RaidResource { .. })),
        "resource-depleted faction should raid rather than pursue diplomacy"
    );

    // Population level: military=25 ≥ WAR_PARTY_MILITARY_MIN (15.0) and
    // adults ≥ WAR_PARTY_THRESHOLD → war party dispatched toward hostile target.
    let target = (Uuid::new_v4(), 200.0f32, 200.0f32, 0.0f32);
    let pop = make_pop(WAR_PARTY_THRESHOLD, 25.0);
    let (_, effects) = tick_population(pop, &[target], None);
    assert!(
        effects.iter().any(|e| matches!(e, PopulationEffect::FormWarParty { .. })),
        "starving but militarily capable settlement should dispatch war party"
    );
}
