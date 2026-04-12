# System: Factions

Factions are independent political entities that simulate their own goals, resources, and inter-faction relationships. Each faction has standing rules that govern how it treats players, and an aggression flag that can override those rules entirely.

Pure faction logic lives in `crates/shared/src/world/faction.rs`. Server-only ECS wiring lives in `crates/server/src/plugins/ai.rs`.

## Faction roster

| Faction | ID | Default Standing | Aggressive | Wars Against | Lore |
|---|---|---|---|---|---|
| Iron Wolves | `iron_wolves` | 0 (Neutral) | No | Ash Covenant | Mercenary warband guarding northern mines. Respects strength; will trade. |
| Merchant Guild | `merchant_guild` | 0 (Neutral) | No | Deep Tide | Trade consortium controlling southern ports. Prefers alliances to war. |
| Ash Covenant | `ash_covenant` | −500 (Hostile) | Yes | Iron Wolves | Zealot order that treats ancient ruins as sacred ground. Purges all outsiders on sight. |
| Deep Tide | `deep_tide` | −500 (Hostile) | Yes | Merchant Guild | Underdark raiders that surface seasonally to plunder before retreating below. |

Default standing is applied when a player has no prior interaction with the faction. Ash Covenant and Deep Tide always begin at Hostile, meaning their NPCs attack players on sight from the first encounter.

Disposition maps are seeded in `seed_factions` so that Iron Wolves and Ash Covenant are mutually `Disposition::Hostile`, as are Merchant Guild and Deep Tide. These dispositions drive war-party targeting in `tick_population`.

## Standing tiers

Player–faction reputation is a signed integer in `[-999, 1000]`, mapped to a tier by `standing_tier(score)` in `faction.rs`.

| Tier | Score range | NPC behaviour |
|---|---|---|
| Exalted | ≥ 750 | Faction merchants give discounts; NPCs offer quests |
| Honored | ≥ 500 | Faction guards assist player against other factions in combat |
| Friendly | ≥ 250 | NPCs greet player; no aggression |
| Neutral | ≥ 0 | Default; no special treatment |
| Unfriendly | ≥ −250 | NPCs make hostile comments; refuse to trade |
| Hostile | ≥ −500 | NPCs attack player on sight within aggro range |
| Hated | < −500 | All faction entities attack immediately; recovery requires quests |

`StandingTier::is_aggressive()` returns `true` for Hostile and Hated. This is the threshold used by the aggression-check system.

## Kill penalties

Killing a faction NPC reduces standing with that faction:

| Rank | Delta | Examples |
|---|---|---|
| `Grunt` | −50 | Ordinary soldiers, roadside guards |
| `Named` | −200 | Lieutenants, champions |
| `Boss` | −500 | Unique named faction bosses |

10 grunt kills on an Iron Wolves NPC: 0 + (10 × −50) = −500 → Hostile tier → Iron Wolves NPCs now attack on sight.

Penalties are applied in `resolve_interrupts` Phase 4b, immediately after the kill effect is processed. The delta is logged at `DEBUG` level with the new score and tier.

## Aggression rules

Two independent triggers cause a faction NPC to insert `PendingAttack` against a nearby player:

1. **Faction flag** — `Faction::is_aggressive == true`. Any player within aggro range is attacked regardless of their standing score.
2. **Standing threshold** — `standing_tier(score).is_aggressive()`. Even non-aggressive factions attack a player whose standing has crossed into Hostile or Hated.

Range: 10 tiles (squared distance ≤ 100.0). Checked in `check_faction_aggression`, which runs at `FixedUpdate` (62.5 Hz) before `process_player_input`.

## Reputation storage

`PlayerReputationMap` is a `Resource` in the server ECS:

```
PlayerReputationMap(HashMap<Uuid, HashMap<FactionId, i32>>)
```

Keyed by player UUID (same UUID as `CombatantId`). Score access falls back to `default_standing(faction_id)` when no record exists. All mutations clamp to `[-999, 1000]`.

The reputation map is persisted to SQLite in the `player_faction_standing` table (migration `003_reputation.sql`). Load on connect, save on disconnect — same pattern as other player state in `on_client_connected` / `on_client_disconnected`.

## NPC rank component

Each faction NPC carries `FactionNpcRank(NpcRank)` — a server-only component set at spawn time. Currently all spawned faction NPCs are `NpcRank::Grunt`. Named NPCs and bosses will be given higher ranks when dungeon content is added.

## ECS components (server-only)

| Component | Location | Description |
|---|---|---|
| `FactionMember(FactionId)` | `plugins/ai.rs` | Which faction this NPC belongs to |
| `FactionNpcRank(NpcRank)` | `plugins/ai.rs` | Rank for kill-penalty calculation |
| `CurrentGoal(Option<FactionGoal>)` | `plugins/ai.rs` | Active AI goal being pursued |
| `HomePosition(WorldPosition)` | `plugins/ai.rs` | Origin for bounded-wander pathfinding |
| `WarPartyMember { target_settlement_id, target_x, target_y }` | `plugins/ai.rs` | Tags an NPC as a war-party warrior marching to the target settlement |
| `ActiveBattle { settlement_id, attacker_faction, defender_faction, battle_x, battle_y, attacker_casualties, defender_casualties }` | `plugins/ai.rs` | Bookkeeping entity spawned when a war party arrives; despawned when the battle ends |

## Resources (server)

| Resource | Type | Description |
|---|---|---|
| `FactionRegistry` | `Resource` | All live faction data (`Vec<Faction>`), including disposition maps |
| `PlayerReputationMap` | `Resource` | Per-player, per-faction standing scores |
| `FactionPopulationState` | `Resource` | Per-settlement birth counters and adult/child counts for the population simulation |

## Current state

- 4 factions seeded at startup (`seed_factions` Startup system).
- 3 guard NPCs spawned per faction at their home settlement.
- Aggression check fires every `FixedUpdate` tick (62.5 Hz).
- Kill penalties applied live in `resolve_interrupts`.
- Persistence schema (`003_reputation.sql`) exists; load/save hooks are stubs.
- Faction goal AI runs at 1 Hz (`WorldSimSchedule`).
- Settlement populations grow via `tick_population_system` and `age_npcs_system` (1 Hz).
- War parties form when `adult_count ≥ 15` and march toward hostile-faction settlements via `march_war_parties` (1 Hz).
- Battles resolve via `run_battle_rounds` using seeded deterministic dice; results broadcast to clients.
- Non-war-party NPCs (guards) are stationary pending pathfinding implementation.
