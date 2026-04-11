# System: World Simulation

The world simulation runs independently of player presence. Two separate schedules tick at different rates; neither waits for the other.

Exact tick rates and timing constants are defined in `crates/shared/src/lib.rs` (`TICK_HZ`) and `crates/server/src/plugins/world_sim.rs`.

## Tick rates

| Schedule | Rate | Systems |
|---|---|---|
| `FixedUpdate` | `TICK_HZ` (see `shared/src/lib.rs`) | Player input, movement, combat resolution |
| `WorldSimSchedule` | 1 Hz (real time) | Faction AI, ecology, story flush |

`WorldSimSchedule` is a custom Bevy schedule driven by a 1-second repeating timer in `Update`. It is not a `FixedUpdate` variant — it fires based on wall-clock time, not simulation ticks.

`WorldSimTick` is a monotonic counter incremented each time `WorldSimSchedule` fires. It is used as a timestamp for story events and as the basis for autosave intervals.

## History warp

On server startup, `MapGenPlugin` runs `WorldSimSchedule` synchronously for `HISTORY_WARP_TICKS` iterations (see `crates/server/src/plugins/map_gen.rs`) before accepting network connections. This pre-ages the world: factions have had time to expand and compete, ecology populations have stabilised or collapsed, and story events have accumulated. Players join a world with history, not a blank slate.

## Ecology (`world/ecology.rs`)

Each region tracks predator and prey population counts. Each world-sim tick, populations update using a discrete Lotka-Volterra model:

```
new_prey     = prey × (1 + r × (1 − prey/K)) − α × predator × prey
new_predator = predator × (β × α × prey − δ)
```

The model coefficients (`r`, `K`, `α`, `β`, `δ`) and the collapse threshold are defined in `ecology.rs`.

When a prey population falls below the collapse threshold, a `StoryEvent::EcologyCollapse` is emitted. This propagates to faction AI: food-scarce factions re-evaluate their goals and may shift toward raiding.

Populations are clamped to non-negative values. The model is pure — the ECS bridge passes current counts in and receives updated counts back.

## Faction AI (`world/faction.rs`)

Each faction holds a set of goals, scored by a utility function each world-sim tick. The goal with the highest utility becomes active. Goals include:

- **ExpandTerritory** — claim adjacent regions; scores high when food is plentiful and military is strong
- **DefendSettlement** — fortify when threatened; scores high when a settlement is contested
- **RaidResource** — send raiding parties; scores high when food is scarce and military is moderate
- **FormAlliance** — seek alliances; scores high when outnumbered by enemies
- **Survive** — fallback; always scores above 0

At equilibrium, a well-resourced faction expands; a food-stressed faction raids; a weak faction seeks alliances. Players can shift these balances by killing NPCs, depleting prey populations, or destroying settlements.

NPC entities wander toward their faction's active goal each world-sim tick. Individual NPC pathfinding runs in `FixedUpdate`.

## Story log (`world/story.rs`)

Any server system can emit a `WriteStoryEvent` Bevy message. The story plugin collects these and appends them to the `StoryLog` resource — an in-memory ordered list of `StoryEvent` values indexed by lore tags.

A `StoryEvent` records:
- Unique ID and world-sim tick timestamp
- A `StoryEventKind` variant (faction war declared, ecology collapse, player killed named NPC, etc.)
- Participant `GameEntityId` list
- Optional tile-space location
- Lore tags for filtering (e.g. `"death"`, `"faction"`, `"ecology"`)

The log is flushed to SQLite periodically (autosave not yet wired; stub in place). It will be streamed to clients as UI events once the egui HUD is implemented.

Story events feed back into faction AI: a named NPC death may cause the victim's faction to spawn a `DefendSettlement` or retaliation goal.
