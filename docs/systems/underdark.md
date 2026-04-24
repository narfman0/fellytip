# System: Underdark Pressure and Raids

The Underdark is a passive threat that sits underneath the world and slowly builds pressure over real time. When the pressure crosses thresholds, environmental signals leak up to the story log. When it peaks, a concrete raid party spawns in the deepest Underdark zone and hops up through the zone graph toward the surface.

This system is the main load-bearing consumer of the Zone Graph (`docs/systems/zones.md`) and the first user of the 0.1 Hz `UnderDarkSimSchedule`.

---

## `UnderDarkPressure` resource

Defined in `crates/server/src/plugins/ai.rs`.

```rust
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct UnderDarkPressure {
    pub score: f32,             // 0.0 = calm, 1.0 = imminent raid
    pub last_raid_tick: u64,    // WorldSimTick when the last raid spawned
    pub thresholds_crossed: u8, // bit 0 = 0.4 distant, bit 1 = 0.7 imminent
}
```

`thresholds_crossed` is a hysteresis bitmask — a bit is **set** when the score crosses the threshold upward and **cleared** when the score drops back below 0.4. This prevents repeated story-event spam while the score oscillates near a threshold.

---

## Schedules

| Schedule | Rate | Systems |
|---|---|---|
| `UnderDarkSimSchedule` | 0.1 Hz (every 10 real seconds) | `accumulate_underdark_pressure`, `deliver_underdark_signals` |
| `WorldSimSchedule` | 1 Hz | `spawn_underdark_raid`, `advance_zone_parties` |

Both schedules are defined in `crates/server/src/plugins/world_sim.rs`. The 0.1 Hz cadence is explicitly slow so pressure buildup feels like a background environmental phenomenon rather than a tick-driven counter.

---

## Accumulation formula (0.1 Hz)

`accumulate_underdark_pressure`:

```
score *= UNDERDARK_DECAY                                      // exponential decay toward 0
if any WarPartyMember is currently in an Underdark zone:
    score += UNDERDARK_ACTIVE_BOOST
if (WorldSimTick - last_raid_tick) > UNDERDARK_NATURAL_BUILDUP_AFTER_TICKS:
    score += UNDERDARK_NATURAL_BOOST
score = clamp(score, 0.0, 1.0)
```

The constants live alongside the system in `plugins/ai.rs`. With no inputs the score decays toward zero (~2 minutes to near-zero). Active Underdark presence and time-since-last-raid are additive boosts — the **composite threat score** is this sum of natural buildup + activity, not a single input.

---

## Threshold triggers

`deliver_underdark_signals` (same 0.1 Hz schedule) reads the score and emits `StoryEvent::UnderDarkThreat` when bits flip upward:

| Score | Bit | `hops_to_surface` | Lore tags | Meaning |
|---|---|---|---|---|
| ≥ 0.4 | bit 0 | 99 | `underdark`, `distant` | Something is stirring far below — distant environmental signal |
| ≥ 0.7 | bit 1 | 2 | `underdark`, `imminent`, `fleeing` | A raid is close — surface ecology hooks (fleeing wildlife etc.) should respond |
| ≥ 0.8 | (trigger) | — | — | `spawn_underdark_raid` converts pressure into a concrete war party |

Hysteresis: when `score < 0.4`, all latched bits clear, so the next buildup re-emits the signals.

The `hops_to_surface = 99` on the distant signal is a synthetic sentinel — no real zone is 99 hops away in the current graph. Once a raid has spawned, `advance_zone_parties` emits `UnderDarkThreat` with the **actual** hop distance whenever it's ≤ 3.

---

## Raid spawning (1 Hz, `spawn_underdark_raid`)

Gates:
1. `pressure.score >= 0.8` (`UNDERDARK_RAID_THRESHOLD`)
2. `ZoneRegistry` and `ZoneTopology` present
3. No existing `WarPartyMember` is in an Underdark zone *or* has `attacker_faction == "underdark"` (only one raid active at a time)

Resolution:
1. Pick the **deepest Underdark zone** (highest `depth` in `ZoneKind::Underdark { depth }`).
2. Pick the **highest-population surface settlement** from `FactionPopulationState` as the raid target.
3. Compute `zone_route: Vec<ZoneId>` via BFS from the deepest zone to `OVERWORLD_ZONE` (currently `shortest_zone_path` local helper in `ai.rs` — slated to move onto `ZoneTopology::shortest_path`).
4. Spawn `UNDERDARK_RAID_PARTY_SIZE = 3` entities in a 3-wide offset grid at the Underdark zone's origin, each with:
    - `CombatParticipant { class: Warrior, level: 2, AC 12, STR 12, DEX 11, CON 12 }`
    - `Health { current: 25, max: 25 }`
    - `ExperienceReward(75)`
    - `FactionBadge { faction_id: "underdark", rank: Grunt }`
    - `WarPartyMember { target_settlement_id, target_x, target_y, attacker_faction: "underdark", current_zone: deepest, zone_route }`
    - `ZoneMembership(deepest)`
5. Reset `pressure.score = 0.0`, set `last_raid_tick = WorldSimTick`, clear `thresholds_crossed`.

---

## Zone hopping (1 Hz, `advance_zone_parties`)

For every `WarPartyMember` with a non-empty `zone_route` whose `current_zone != OVERWORLD_ZONE`:

1. If `ZoneTopology::hop_distance(current_zone, OVERWORLD_ZONE) <= 3`, emit `StoryEvent::UnderDarkThreat { hops_to_surface: hops }` with the party's real faction tag — this is what surface-side ecology and quest hooks watch.
2. Find the exit portal from `current_zone` toward `zone_route[0]`; if none exists, clear the route (party is stuck, handled gracefully).
3. If the party is within the portal's `trigger_radius` of the exit anchor (**currently compared against world origin — portal anchor world-coords are a known TODO**), pop `zone_route[0]` and set `current_zone = next_zone` + update `ZoneMembership`.

When `current_zone` reaches `OVERWORLD_ZONE`, the existing `march_war_parties` system (on `WorldSimSchedule`) takes over with normal surface pathfinding toward the target settlement. Battles that follow go through the usual `run_battle_rounds` pipeline and produce `BattleRecord` entries in `BattleHistory`.

---

## Environmental delivery

The Underdark system is deliberately **non-visible** until it's imminent:

- `StoryEvent::UnderDarkThreat` is emitted at the 0.4/0.7 thresholds and continuously while `hops_to_surface <= 3`. The story log surface-renders these.
- Ecology hooks (consumers of `UnderDarkThreat` lore tags — e.g. wildlife fleeing on the `imminent`/`fleeing` tags) are expected to react on the surface side. Those hooks live in `crates/server/src/plugins/ecology.rs` and are minimal today; the plumbing is in place.
- Entities in deep Underdark zones are **not** replicated to surface players by design — the story event is the signal, not the entity presence. Lightyear per-zone interest groups will enforce this once wired (see `docs/systems/zones.md` Implementation Status).

---

## BRP methods

Registered in `crates/server/src/plugins/dm.rs` and mounted in `crates/client/src/main.rs`.

| Method | Params | Returns | Purpose |
|---|---|---|---|
| `dm/underdark_pressure` | `{}` | `{ score: f32, last_raid_tick: u64 }` | Snapshot current pressure — used by `worldwatch` + ralph polling |
| `dm/force_underdark_pressure` | `{}` | `{ ok: true }` | Force `score = 1.0` so the next 1 Hz tick spawns a raid — used by `underdark_e2e` to skip ~10 slow-tick buildup |

The `underdark_e2e` ralph scenario (`tools/ralph/src/scenarios/underdark_e2e.rs`) drives the full loop end-to-end: clear battle history → force pressure → observe `last_raid_tick` advance → find the 3 raid WarPartyMembers by `FactionBadge.faction_id == "underdark"` → poll their `ZoneMembership` until they reach `OVERWORLD_ZONE` → best-effort poll `BattleHistory` for the ensuing surface battle.

---

## Tuning knobs (quick reference)

All are `const` in `crates/server/src/plugins/ai.rs`:

- `UNDERDARK_DECAY` — per 0.1 Hz tick multiplier, dominant term in the decay curve.
- `UNDERDARK_ACTIVE_BOOST` — added per tick if any WarPartyMember is in an Underdark zone.
- `UNDERDARK_NATURAL_BOOST` — added per tick once `WorldSimTick - last_raid_tick > UNDERDARK_NATURAL_BUILDUP_AFTER_TICKS`.
- `UNDERDARK_THRESHOLD_DISTANT_BIT` / `UNDERDARK_THRESHOLD_IMMINENT_BIT` — bitmask layout for hysteresis.
- `UNDERDARK_RAID_THRESHOLD = 0.8` — minimum score before `spawn_underdark_raid` fires.
- `UNDERDARK_RAID_PARTY_SIZE = 3` — raid party member count.
