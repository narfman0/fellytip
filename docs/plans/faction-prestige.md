# Plan: Faction Prestige / Reputation System

## Context

The faction system has solid architectural bones (`Faction`, `FactionId`, `Disposition`, `FactionGoal`,
`FactionRegistry`) but zero player-facing reputation mechanics. `Disposition` tracks inter-faction
relationships only; there is no per-player standing. Killing NPCs records `killer: GameEntityId(Uuid::nil())`
— a known stub. Some factions should be hostile by default (attack on sight); currently no NPC ever
initiates combat. This plan delivers a faction design document first, then wires the reputation mechanics
into the code.

---

## Phase 1 — Write `docs/systems/factions.md`

Create the canonical design reference. This is the first deliverable — document before code.

### Faction Roster

| Faction | Canonical ID | Default Player Standing | Aggressive | Lore |
|---|---|---|---|---|
| Iron Wolves | `iron_wolves` | 0 (Neutral) | No | Mercenary warband guarding northern mines. Respects strength; will trade. Rename from `"wolves"` in code. |
| Merchant Guild | `merchant_guild` | 0 (Neutral) | No | Trade consortium controlling southern ports. Prefers alliances to war. Rename from `"guild"` in code. |
| Ash Covenant | `ash_covenant` | −500 (Hostile) | Yes | Zealot order treating the ancient ruins as sacred ground. Purges all outsiders on sight. |
| Deep Tide | `deep_tide` | −500 (Hostile) | Yes | Underdark raiders that surface seasonally to plunder before retreating below. |

### Standing Tiers

| Tier | Score Range | Effect on NPC Behaviour |
|---|---|---|
| Exalted | ≥ 750 | Faction merchants give discounts; NPCs offer quests |
| Honored | ≥ 500 | Faction guards assist player against other factions in combat |
| Friendly | ≥ 250 | NPCs greet player; no aggression |
| Neutral | ≥ 0 | Default; no special treatment |
| Unfriendly | ≥ −250 | NPCs make hostile comments; refuse to trade |
| Hostile | ≥ −500 | NPCs attack player on sight within aggro range |
| Hated | < −500 | All faction entities attack immediately; recovery requires quests |

### Kill Penalty Mechanics

| NPC Rank | Standing Delta | Examples |
|---|---|---|
| Grunt | −50 | Ordinary soldiers, roadside guards |
| Named | −200 | Lieutenants, champions |
| Boss | −500 | Unique named faction bosses |

10 grunt kills → −500 → standing crosses into Hostile.

### Aggression Rules

Two triggers cause a faction NPC to initiate combat:
1. **Faction is aggressive** (`is_aggressive = true`): any player within 10 tiles is attacked regardless
   of standing.
2. **Player standing is Hostile or Hated**: even non-aggressive factions will attack a player whose
   standing has fallen that low.

Aggression checks run at `FixedUpdate` (62.5 Hz) in a new `check_faction_aggression` system chained
before `process_player_input`.

---

## Phase 2 — Pure types in `crates/shared/src/world/faction.rs`

Add to the existing file (no new file needed):

```rust
pub const STANDING_EXALTED:    i32 =  750;
pub const STANDING_HONORED:    i32 =  500;
pub const STANDING_FRIENDLY:   i32 =  250;
pub const STANDING_NEUTRAL:    i32 =    0;
pub const STANDING_UNFRIENDLY: i32 = -250;
pub const STANDING_HOSTILE:    i32 = -500;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Reflect, Serialize, Deserialize)]
pub enum StandingTier { Exalted, Honored, Friendly, Neutral, Unfriendly, Hostile, Hated }

impl StandingTier {
    pub fn is_aggressive(self) -> bool {
        matches!(self, StandingTier::Hostile | StandingTier::Hated)
    }
}

pub fn standing_tier(score: i32) -> StandingTier { … }  // threshold comparisons

#[derive(Clone, Copy, Debug, PartialEq, Eq, Reflect, Serialize, Deserialize)]
pub enum NpcRank { Grunt, Named, Boss }

pub const KILL_PENALTY_GRUNT: i32 = -50;
pub const KILL_PENALTY_NAMED: i32 = -200;
pub const KILL_PENALTY_BOSS:  i32 = -500;

pub fn kill_standing_delta(rank: NpcRank) -> i32 { … }

pub fn default_standing(faction_id: &FactionId) -> i32 {
    match faction_id.0.as_str() {
        "ash_covenant" | "deep_tide" => STANDING_HOSTILE,
        _ => STANDING_NEUTRAL,
    }
}
```

Extend `Faction` struct with two new fields:

```rust
pub is_aggressive: bool,
pub player_default_standing: i32,
```

Add `PlayerReputationMap` (keyed by player `Uuid` — same UUID as `CombatantId`):

```rust
#[derive(Debug, Default, Clone, Reflect, Resource)]
pub struct PlayerReputationMap(pub HashMap<Uuid, HashMap<FactionId, i32>>);

impl PlayerReputationMap {
    pub fn score(&self, player_id: Uuid, faction: &FactionId) -> i32 { … }
    pub fn apply_delta(&mut self, player_id: Uuid, faction: &FactionId, delta: i32) { … }
    // clamp result to [-999, 1000]
}
```

**Unit tests** (same `#[cfg(test)]` block):
- `standing_tier_exact_boundaries` — test every threshold edge
- `kill_penalty_ordering` — Boss < Named < Grunt < 0
- `aggressive_tiers` — Hostile/Hated are aggressive, Neutral is not
- `default_standing_hostile_factions` — ash_covenant and deep_tide return STANDING_HOSTILE
- `reputation_map_new_player_gets_default`
- `reputation_map_delta_clamps` — apply −2000 stays ≥ −999
- `ten_grunt_kills_reach_hostile` — 10 × −50 = −500 → Hostile tier

---

## Phase 3 — `GameEntityId` as a Bevy `Component`

**`crates/shared/src/world/story.rs`** — add `Component, Reflect` derives:

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash, Component, Reflect)]
pub struct GameEntityId(pub Uuid);
```

**`crates/server/src/main.rs`** — in `on_client_connected`, after spawning the player entity insert
`GameEntityId(participant.id.0)`. This creates the invariant: `CombatantId.0 == GameEntityId.0` for all
player entities, enabling the reverse-lookup needed in Phase 5.

---

## Phase 4 — `FactionNpcRank` component + faction expansion

**`crates/server/src/plugins/ai.rs`**

New server-only component:

```rust
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct FactionNpcRank(pub NpcRank);
```

In `spawn_faction_npcs`, add `FactionNpcRank(NpcRank::Grunt)` to each spawned bundle.

In `seed_factions`, rename IDs to canonical names and expand to 4 factions with the new fields:

| ID | is_aggressive | player_default_standing | food | military |
|---|---|---|---|---|
| `iron_wolves` | false | 0 | 20 | 30 |
| `merchant_guild` | false | 0 | 80 | 10 |
| `ash_covenant` | true | −500 | 15 | 40 |
| `deep_tide` | true | −500 | 10 | 35 |

Update `make_faction` helper in the test block to include the two new fields.

---

## Phase 5 — Fix killer tracking + reputation delta in `combat.rs`

**`crates/server/src/plugins/combat.rs`**

**5a.** Expand `ParticipantQuery` type alias to add:
- `Option<&'static GameEntityId>`
- `Option<&'static FactionMember>`
- `Option<&'static FactionNpcRank>`

**5b.** In Phase 1 of `resolve_interrupts`, build:
```rust
let entity_to_game_id: HashMap<Entity, Uuid> = participants
    .iter()
    .filter_map(|(e, _, _, _, _, gid, ..)| gid.map(|g| (e, g.0)))
    .collect();
```

**5c.** Fix the `Effect::Die` arm — replace `Uuid::nil()` stub:
```rust
let killer_uuid = entity_to_game_id.get(attacker_entity).copied().unwrap_or(Uuid::nil());
// use killer_uuid in PlayerKilledNamed story event
// push to reputation_kills vec for Phase 4b
```

**5d.** New **Phase 4b** (after XP awards, before story emit):
```rust
for kill in &reputation_kills {
    if kill.killer_uuid == Uuid::nil() { continue; }
    if let Ok((.., fm, rank)) = participants.get(kill.target_entity) {
        if let Some(fm) = fm {
            let rank = rank.map(|r| r.0).unwrap_or(NpcRank::Grunt);
            reputation.apply_delta(kill.killer_uuid, &fm.0, kill_standing_delta(rank));
        }
    }
}
```

Add `ResMut<PlayerReputationMap>` to `resolve_interrupts` system params.

Also populate `CombatantSnapshot.faction` from `FactionMember` (currently always `None` at combat.rs:279).

---

## Phase 6 — NPC aggression-check system

**`crates/server/src/plugins/combat.rs`**

New system `check_faction_aggression`, chained **before** `process_player_input`:

```rust
fn check_faction_aggression(
    npc_query: Query<
        (Entity, &FactionMember, &WorldPosition),
        (With<ExperienceReward>, Without<PendingAttack>),
    >,
    player_query: Query<
        (Entity, &WorldPosition, &GameEntityId),
        Without<ExperienceReward>,
    >,
    reputation: Res<PlayerReputationMap>,
    registry: Res<FactionRegistry>,
    mut commands: Commands,
) {
    const AGGRO_RANGE_SQ: f32 = 100.0; // 10 tiles²
    for (npc_entity, fm, npc_pos) in npc_query.iter() {
        let Some(faction) = registry.factions.iter().find(|f| f.id == fm.0) else { continue };
        for (player_entity, player_pos, gid) in player_query.iter() {
            let dx = npc_pos.x - player_pos.x;
            let dy = npc_pos.y - player_pos.y;
            if dx * dx + dy * dy > AGGRO_RANGE_SQ { continue; }
            let tier = standing_tier(reputation.score(gid.0, &fm.0));
            if faction.is_aggressive || tier.is_aggressive() {
                commands.entity(npc_entity).insert(PendingAttack { target: player_entity });
                break;
            }
        }
    }
}
```

Updated system chain:
```rust
(check_faction_aggression, process_player_input, initiate_attacks, initiate_abilities, resolve_interrupts).chain()
```

Register in `AiPlugin::build`:
```rust
app.init_resource::<PlayerReputationMap>();
app.register_type::<PlayerReputationMap>();
```

---

## Phase 7 — Persistence

**New file `migrations/003_reputation.sql`:**
```sql
CREATE TABLE IF NOT EXISTS player_faction_standing (
    player_id  TEXT    NOT NULL,
    faction_id TEXT    NOT NULL,
    score      INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (player_id, faction_id)
);
```

**`crates/server/src/main.rs` / `persistence.rs`:**
- **Load** on `on_client_connected`: query `player_faction_standing` for this player UUID; populate `PlayerReputationMap`.
- **Save** on `on_client_disconnected`: upsert all entries for that player.
- **Periodic flush**: every ~300 `WorldSimSchedule` ticks, flush dirty values (same pattern as `flush_story_log`).

---

## Phase 8 — Doc updates

- `docs/systems/factions.md` — new file (expanded Phase 1 content with full prose)
- `docs/systems/world-sim.md` — note aggression checks run at FixedUpdate; `check_faction_aggression` inserts `PendingAttack`
- `docs/systems/combat.md` — add Phase 4b to phase list; add `check_faction_aggression` to system-chain table; note `GameEntityId` now on player entities
- `docs/architecture.md` — add `PlayerReputationMap` to resources table; document invariant `CombatantId.0 == GameEntityId.0` for players

---

## Critical Files

| File | Changes |
|---|---|
| `crates/shared/src/world/faction.rs` | `StandingTier`, `NpcRank`, `PlayerReputationMap`, new `Faction` fields, unit tests |
| `crates/shared/src/world/story.rs` | Add `Component, Reflect` to `GameEntityId` |
| `crates/server/src/plugins/ai.rs` | `FactionNpcRank` component, 4-faction `seed_factions`, `FactionNpcRank::Grunt` in spawn |
| `crates/server/src/plugins/combat.rs` | `check_faction_aggression`, killer UUID fix, Phase 4b rep delta, `ParticipantQuery` expansion |
| `crates/server/src/main.rs` | Insert `GameEntityId` on player spawn; persistence load/save hooks |
| `migrations/003_reputation.sql` | New: `player_faction_standing` table |
| `docs/systems/factions.md` | New: canonical faction design doc |

---

## Implementation Order

1. Write `docs/systems/factions.md` (document first)
2. Pure types in `faction.rs` + unit tests → `cargo test -p fellytip-shared` green
3. `GameEntityId` component derive + player spawn insertion
4. `FactionNpcRank` + `seed_factions` expansion (4 factions + new fields)
5. `combat.rs` — killer fix + Phase 4b rep delta + `CombatantSnapshot.faction` population
6. `check_faction_aggression` system + chain update
7. `AiPlugin` resource registration
8. Migration + persistence load/save
9. `cargo clippy --workspace -- -D warnings` clean
10. `cargo test --workspace` green
11. Remaining doc updates (world-sim, combat, architecture)

---

## Verification

```bash
cargo test -p fellytip-shared   # new standing/reputation unit tests
cargo test --workspace          # all tests green
cargo clippy --workspace -- -D warnings
cargo build --workspace
cargo run -p ralph -- --scenario all   # basic_movement still passes
```

Manual smoke tests:
- Spawn near an Ash Covenant NPC → it attacks immediately (is_aggressive = true)
- Kill one Iron Wolves grunt → standing drops 0 → −50
- Kill 10 Iron Wolves grunts → −500 → Hostile tier → Iron Wolves NPCs now attack on sight
