# Fellytip: Rust/Bevy Multiplayer RPG — Implementation Plan

## Context

The goal is a multiplayer action RPG where the world is the protagonist. Inspired by model-based testing discipline from D&D rule engines, the game separates simulation logic (pure, testable) from rendering (Bevy ECS). The "minimum viable fun" is: a party of up to 4 players can enter a living world, observe ongoing faction/ecology dynamics, and meaningfully intervene in the story. The world continues ticking regardless of player presence.

Start top-down 2D; upgrade path to isometric is baked into the coordinate transform layer.

---

## Version Anchors (verified on crates.io 2026-04-10)

- **Bevy**: `0.18.1`
- **Lightyear**: `0.26.4` (targets Bevy 0.18)
- **sqlx**: `0.8` stable (0.9 is alpha)
- **bevy_egui**: `0.39.1`
- **bevy-inspector-egui**: `0.36.0`
- **Rust edition**: 2021

---

## Workspace Layout

```
fellytip/
  Cargo.toml                   ← workspace root + shared dep versions
  Cargo.lock
  .cargo/config.toml           ← linker tweaks, dev opt-levels
  docs/
    PLAN.md                    ← this file
  crates/
    shared/                    ← pure types, protocol, combat rules, world types
      src/
        lib.rs
        components.rs
        resources.rs
        protocol.rs            ← lightyear channel + component registration
        inputs.rs
        combat/
          mod.rs
          types.rs
          rules.rs             ← pure fn(State) -> (State, Vec<Effect>)
          interrupt.rs         ← interrupt chain state machine
        world/
          mod.rs
          faction.rs
          ecology.rs
          story.rs
          schedule.rs
          map.rs            ← TileKind (20 variants), TileLayer, TileColumn, WorldMap
          civilization.rs   ← Settlement, generate_settlements, assign_territories, generate_roads
        math.rs             ← tile_index/frac, bilerp, fbm (smooth_step, lattice_hash, value_noise)
    server/
      src/
        main.rs
        plugins/
          network.rs           ← lightyear ServerPlugin wiring
          world_sim.rs         ← custom WorldSimSchedule (1 Hz)
          map_gen.rs           ← generate_map + 200-tick history warp on Startup
          ai.rs
          ecology.rs
          story.rs
          persistence.rs       ← sqlx save/load
          combat.rs
        systems/
          npc_ai.rs
          ecology_tick.rs
          story_writer.rs
          combat_resolution.rs
    client/
      src/
        main.rs                ← --headless flag skips window/rendering
        plugins/
          network.rs           ← lightyear ClientPlugin wiring
          rendering.rs
          ui.rs                ← egui HUD, lore log
          input.rs
          prediction.rs
        systems/
          camera.rs
          animation.rs
          hud.rs
  tools/
    combat_sim/                ← proptest harness for combat rules (no ECS)
    world_gen/                 ← standalone world seed generator
    ralph/                     ← "ralph loop" test driver CLI (HTTP → BRP commands)
      src/
        main.rs
        brp.rs                 ← reqwest-based BRP HTTP client
        scenarios/
          mod.rs
          basic_movement.rs
          combat_resolves.rs
          world_sim_ticks.rs
          ecology_loop.rs
          story_log_written.rs
```

---

## Key Dependencies

### Root `Cargo.toml`

```toml
[workspace]
resolver = "2"
members = [
  "crates/shared",
  "crates/server",
  "crates/client",
  "tools/combat_sim",
  "tools/world_gen",
  "tools/ralph",
]

[workspace.dependencies]
bevy              = { version = "0.18", default-features = false }
lightyear         = { version = "0.26.4", default-features = false }
serde             = { version = "1", features = ["derive"] }
serde_json        = "1"
sqlx              = { version = "0.8", features = ["runtime-tokio", "sqlite", "macros", "migrate"] }
tokio             = { version = "1", features = ["full"] }
rand              = "0.9"
rand_chacha       = "0.3"
tracing           = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
anyhow            = "1"
thiserror         = "2"
smol_str          = "0.3"
uuid              = { version = "1", features = ["v4", "serde"] }
proptest          = "1"
proptest-derive   = "0.4"
bevy_egui         = "0.39"
bevy-inspector-egui = "0.36"
reqwest           = { version = "0.12", features = ["json", "blocking"] }

[profile.dev.package."*"]
opt-level = 3   # optimize bevy/wgpu even in dev builds
```

### Per-crate feature selections

| Crate | Key lightyear features | Other notable deps |
|---|---|---|
| `shared` | `replication`, `prediction`, `interpolation`, `input_native` | bevy `bevy_reflect` + `serialize` |
| `server` | `server`, `netcode`, `udp`, `replication` | sqlx, tokio, `bevy_remote` (built-in) |
| `client` | `client`, `netcode`, `udp`, `prediction`, `interpolation`, `replication` | bevy_egui, `bevy_remote`; bevy-inspector-egui behind `debug` feature |
| `ralph` | — | reqwest blocking, serde_json |

---

## ECS Component Ownership

### Shared (replicated) — `crates/shared/src/components.rs`

```rust
WorldPosition { x: f32, y: f32, z: f32 }  // replicated + predicted + interpolated; z = elevation
FacingDir(f32)                         // replicated + interpolated
Health { current: i32, max: i32 }      // replicated, simple interpolation
Stamina { current: i32, max: i32 }
CharacterSheet { class, level, stats: CoreStats }
FactionMembership(FactionId)           // replicated Once (rarely changes)
NpcTag { archetype, schedule_id }      // replicated Once on spawn
Species(SpeciesId)
EcologyRole(EcologyRoleKind)
SpriteKey(SmolStr)                     // tells client which sprite atlas row to use
GameEntityId(Uuid)                     // stable cross-session identity
```

### Server-only (never replicated)

```rust
AiState { goal_stack: Vec<AiGoal>, path: VecDeque<IVec2>, cooldown: f32 }
ScheduleState { current_phase: SchedulePhase, phase_timer: f32 }
CombatParticipant { interrupt_stack: InterruptStack, pending_reactions: Vec<ReactionWindow> }
PersistenceTag(Uuid)
```

### Client-only

```rust
SpriteAnimState { atlas_handle, frame: usize, timer: Timer }
LocalPlayerTag                         // marker for the entity this client controls
HealthBarUi(Entity)                    // reference to associated UI entity
```

---

## Lightyear Protocol (`crates/shared/src/protocol.rs`)

```rust
pub struct FellytipProtocolPlugin;
impl Plugin for FellytipProtocolPlugin {
    fn build(&self, app: &mut App) {
        // Channels
        app.add_channel::<WorldStateChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(..),
            direction: ChannelDirection::ServerToClient, ..
        });
        app.add_channel::<PlayerInputChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            direction: ChannelDirection::ClientToServer, ..
        });
        app.add_channel::<CombatEventChannel>(ChannelSettings {
            mode: ChannelMode::SequencedReliable(..),
            direction: ChannelDirection::ServerToClient, ..
        });
        app.add_channel::<ChatChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(..),
            direction: ChannelDirection::Bidirectional, ..
        });

        // Components
        app.register_component::<WorldPosition>()
           .add_prediction(ComponentSyncMode::Full)
           .add_linear_interpolation();
        app.register_component::<Health>()
           .add_interpolation(ComponentSyncMode::Simple);
        app.register_component::<FactionMembership>()
           .add_interpolation(ComponentSyncMode::Once);
        app.register_component::<SpriteKey>()
           .add_interpolation(ComponentSyncMode::Once);
        // ... repeat for all shared components

        // Messages
        app.register_message::<CombatEventMsg>(MessageDirection::ServerToClient);
        app.register_message::<StoryEventMsg>(MessageDirection::ServerToClient);
        app.register_message::<ChatMsg>(MessageDirection::Bidirectional);

        // Inputs
        app.add_plugins(InputPlugin::<PlayerInput>::default());
    }
}
```

**Input type (`crates/shared/src/inputs.rs`):**
```rust
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Reflect)]
pub struct PlayerInput {
    pub move_dir: Vec2,
    pub action: Option<ActionIntent>,
    pub target: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Reflect)]
pub enum ActionIntent { BasicAttack, UseAbility(u8), Interact, Dodge }
```

**Transport:** UDP on port 5000. Server replication send interval: 50ms (20 Hz). Sim tick: 16ms (62.5 Hz).

**Interest management:** `ReplicationGroup` + per-player visibility radius (~60 tiles).

---

## World Simulation Architecture

### Two independent tick rates

| Schedule | Frequency | What runs |
|---|---|---|
| `FixedUpdate` | 62.5 Hz | Combat, movement, input application, collision |
| `WorldSimSchedule` (custom) | 1 Hz | Faction AI, ecology, story events, NPC schedule transitions |

### Faction System (`crates/shared/src/world/faction.rs`)

```rust
pub struct Faction {
    pub id: FactionId,
    pub disposition: HashMap<FactionId, Disposition>,  // Hostile/Neutral/Friendly/Allied
    pub goals: Vec<FactionGoal>,       // priority-ordered, re-evaluated each world sim tick
    pub resources: FactionResources,   // food, gold, military_strength
    pub territory: Vec<IVec2>,
}

pub enum FactionGoal {
    ExpandTerritory { target_region: RegionId },
    DefendSettlement { settlement: GameEntityId },
    RaidResource { resource_node: GameEntityId },
    FormAlliance { with: FactionId, min_trust: f32 },
    Survive,
}
```

Each world sim tick: utility-score all goals → pick highest → update `AiState.goal_stack` for all NPCs of that faction.

### NPC Daily Schedule (`crates/shared/src/world/schedule.rs`)

```rust
pub struct NpcSchedule { pub phases: Vec<SchedulePhase> }
pub struct SchedulePhase {
    pub start_hour: f32,
    pub end_hour: f32,
    pub activity: NpcActivity,
    pub location: LocationHint,
}
pub enum NpcActivity { Sleep, Work, Patrol, Trade, Guard, Socialize }
```

Combat interrupts the schedule. After combat, the NPC resumes the phase matching the current world hour.

### Ecology System (`crates/shared/src/world/ecology.rs`)

Discrete-time Lotka-Volterra per region (each world sim tick):

```
new_prey     = prey * (1 + r * (1 - prey/K)) - α * predator * prey
new_predator = predator * (β * α * prey - δ)
```

- Population below threshold → emit `StoryEvent::EcologyCollapse { species, region }`
- Resource node over-hunted → reduce `FactionResources.food` → faction re-evaluates goals
- Players affect the world without explicit scripting

### Story Event Log (`crates/shared/src/world/story.rs`)

```rust
pub struct StoryEvent {
    pub id: Uuid,
    pub tick: u64,
    pub world_day: u32,
    pub kind: StoryEventKind,
    pub participants: Vec<GameEntityId>,
    pub location: Option<IVec2>,
    pub lore_tags: Vec<SmolStr>,
}

pub enum StoryEventKind {
    // World sim events
    FactionWarDeclared { attacker: FactionId, defender: FactionId },
    SettlementFounded  { faction: FactionId, name: SmolStr },
    SettlementRazed    { by: FactionId },
    EcologyCollapse    { species: SpeciesId, region: RegionId },
    AllianceFormed     { a: FactionId, b: FactionId },
    // Player-triggered
    PlayerKilledNamed  { victim: GameEntityId, killer: GameEntityId },
    PartyDefeatedBoss  { boss: GameEntityId },
    QuestCompleted     { quest_id: SmolStr },
    PlayerJoinedFaction { player: GameEntityId, faction: FactionId },
    // Emergent
    NpcDefected        { npc: GameEntityId, from: FactionId, to: FactionId },
    MonsterMigrated    { species: SpeciesId, from: RegionId, to: RegionId },
}
```

**How player actions feed the story:**
- Any system emits `WriteStoryEvent(ev)` as a Bevy event
- `story_writer` system appends to `StoryLog` resource and indexes by `lore_tag`
- Named NPC death → faction may spawn a `FactionGoal::DefendSettlement` or retaliatory raid
- Log flushed to SQLite every 5 minutes; streamed to clients as `StoryEventMsg`

---

## Combat System (Model-Based)

Key design: rules are **pure functions in `crates/shared`**, ECS bridge is thin.

### Layer 1: Pure rules (`crates/shared/src/combat/rules.rs`)

```rust
// Dice values injected — never rolled internally (testable)
pub fn resolve_attack_roll(
    attacker: &CombatantSnapshot, defender: &CombatantSnapshot, roll: i32
) -> AttackRollResult

pub fn resolve_damage(
    result: AttackRollResult, attacker: &CombatantSnapshot,
    defender: &CombatantSnapshot, dmg_roll: i32
) -> Vec<Effect>

pub fn apply_effects(state: CombatState, effects: Vec<Effect>) -> (CombatState, Vec<Effect>)
```

### Layer 2: Interrupt chain (`crates/shared/src/combat/interrupt.rs`)

Stack-based, mirrors the blog's approach for handling nested reactions:

```rust
pub enum InterruptFrame {
    ResolvingAttack   { ctx: AttackContext },
    ResolvingDamage   { ctx: DamageContext },
    ResolvingAbility  { ctx: AbilityContext },
    ResolvingMovement { ctx: MovementContext },
}
pub struct InterruptStack(pub Vec<InterruptFrame>);
impl InterruptStack {
    pub fn step(
        &mut self, state: &CombatState, rng: &mut impl Iterator<Item=i32>
    ) -> (Vec<Effect>, bool)   // (effects_to_apply, is_done)
}
```

Exhaustive `match` — no `_` wildcard — forces every frame type to be handled, catching silent fallthrough bugs.

### Layer 3: ECS bridge (`crates/server/src/systems/combat_resolution.rs`)

Build `CombatantSnapshot` from ECS → call `interrupt_stack.step()` → apply `Vec<Effect>` back to ECS + emit story events.

---

## World Map (`crates/shared/src/world/map.rs`)

### Tile structure

Each grid cell `(ix, iy)` is a `TileColumn` — a sorted `Vec<TileLayer>`. Multiple layers coexist vertically (ground + cave + Underdark). Height queries are pure functions with no ECS dependency.

```
MAP_WIDTH = MAP_HEIGHT = 512
STEP_HEIGHT  = 0.6   (max upward snap per tick)
Z_FOLLOW_RATE = 12.0  (lerp speed toward terrain surface)
FALL_SPEED   = 40.0  (max fall speed, world units/s)

Surface Z: 0.0 – 6.0   (Z_SCALE = 6.0)
Shallow cave Z: ~-15    (CA: 48% fill, 5 steps, threshold 5)
Underdark Z:    ~-65    (CA: 30% fill, 3 steps, threshold 6)
```

### Generation pipeline (called by `MapGenPlugin` on `Startup`)

```
generate_map(seed):
  1. fBm surface heights    (6 octaves, BASE_FREQ=4/512, seed-offset)
  2. fBm moisture           (4 octaves, MOISTURE_FREQ=6/512, separate seed-offset)
  3. classify_biome(temp, moisture) per walkable tile → TileKind
  4. river_pass             steepest-descent flow accumulation → River tiles
  5. cave_pass (shallow)    CA cellular automata → Cavern layers at Z≈-15
  6. cave_pass (Underdark)  CA → LuminousGrotto layers at Z≈-65
  7. shaft_pass             ~1/40 eligible columns get Tunnel connectors

generate_settlements(map, seed):
  Surface: Poisson-disk grid (32×32 cell, min dist 30 tiles, score by habitability)
  Underground: BFS connected components of LuminousGrotto ≥500 tiles → UndergroundCity

assign_territories(map, settlements) → TerritoryMap  [BFS flood-fill]

generate_roads(map, settlements):
  Kruskal MST on Euclidean distances → Bresenham segments → map.road_tiles[…] = true
```

### Fluid movement

After each XY move tick, `process_player_input` calls `smooth_surface_at(map, x, y, current_z)` and lerps `pos.z` toward the result. The bilinear interpolation uses per-corner height offsets pre-computed from the 4 adjacent tile centers.

---

## Persistence (`crates/server/src/plugins/persistence.rs`)

SQLite via sqlx. Autosave every 5 minutes (300 world sim ticks).

```sql
-- migrations/001_initial.sql
CREATE TABLE players (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, faction_id TEXT,
    class TEXT NOT NULL, level INTEGER NOT NULL DEFAULT 1,
    health_current INTEGER NOT NULL, health_max INTEGER NOT NULL,
    pos_x REAL NOT NULL DEFAULT 0, pos_y REAL NOT NULL DEFAULT 0,
    last_seen INTEGER NOT NULL
);
CREATE TABLE story_events (
    id TEXT PRIMARY KEY, tick INTEGER NOT NULL, world_day INTEGER NOT NULL,
    kind TEXT NOT NULL,           -- JSON StoryEventKind
    participants TEXT NOT NULL,   -- JSON array of UUIDs
    loc_x INTEGER, loc_y INTEGER,
    lore_tags TEXT NOT NULL       -- JSON array of strings
);
CREATE TABLE factions (
    id TEXT PRIMARY KEY, name TEXT NOT NULL,
    resources TEXT NOT NULL, territory TEXT NOT NULL, goals TEXT NOT NULL
);
CREATE TABLE ecology_state (
    species_id TEXT NOT NULL, region_id TEXT NOT NULL, count INTEGER NOT NULL,
    PRIMARY KEY (species_id, region_id)
);
CREATE TABLE world_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
```

---

## Rendering & Isometric Upgrade Path

### Top-down MVP (`crates/client/src/systems/camera.rs`)

```rust
fn sync_transform(mut q: Query<(&WorldPosition, &mut Transform)>) {
    for (wp, mut tf) in q.iter_mut() {
        tf.translation = Vec3::new(wp.x * TILE_SIZE, wp.y * TILE_SIZE, z_layer(wp.y));
    }
}
```

### Isometric upgrade (swap only this function, gate with `isometric` feature)

```rust
fn sync_transform_iso(mut q: Query<(&WorldPosition, &mut Transform)>) {
    for (wp, mut tf) in q.iter_mut() {
        let sx = (wp.x - wp.y) * (TILE_W / 2.0);
        let sy = (wp.x + wp.y) * (TILE_H / 4.0);
        tf.translation = Vec3::new(sx, sy, sy * -0.001);
    }
}
```

Simulation, networking, combat — **all unchanged**. Feature flags: `topdown` (default) / `isometric`.

---

## Observability & Automated Feedback Loop ("Ralph Loop")

### Bevy Remote Protocol (BRP)

`bevy_remote` is a first-party Bevy crate exposing a JSON-RPC HTTP API over the live ECS world.

```rust
// server — port 15702 (BRP default)
.add_plugins(bevy_remote::RemotePlugin::default())
.add_plugins(bevy_remote::http::RemoteHttpPlugin::default())

// headless client — port 15703
.add_plugins(bevy_remote::RemotePlugin::default())
.add_plugins(bevy_remote::http::RemoteHttpPlugin::with_port(15703))
```

Built-in methods: `bevy/query`, `bevy/get`, `bevy/insert`, `bevy/spawn`, `bevy/destroy`, `bevy/list`

Custom game methods registered on server:
```rust
app.register_remote_method("fellytip/story_by_tag",    story_by_tag_handler);
app.register_remote_method("fellytip/faction_state",   faction_state_handler);
app.register_remote_method("fellytip/inject_input",    inject_player_input_handler);
app.register_remote_method("fellytip/advance_ticks",   advance_world_sim_handler);
```

### Headless Client Mode

```rust
// crates/client/src/main.rs
fn main() {
    let headless = std::env::args().any(|a| a == "--headless");
    let mut app = App::new();
    if headless {
        app.add_plugins(MinimalPlugins)
           .add_plugins(bevy_remote::RemotePlugin::default())
           .add_plugins(bevy_remote::http::RemoteHttpPlugin::with_port(15703))
           .add_plugins(FellytipProtocolPlugin)
           .add_plugins(ClientPlugin { config: build_client_config() });
    } else {
        app.add_plugins(DefaultPlugins.set(WindowPlugin { .. }))
           .add_plugins(FellytipProtocolPlugin)
           .add_plugins(ClientPlugin { config: build_client_config() })
           .add_plugins(RenderingPlugin)
           .add_plugins(UiPlugin);
        #[cfg(feature = "debug")]
        app.add_plugins(bevy_inspector_egui::quick::WorldInspectorPlugin::default());
    }
    app.add_plugins(InputPlugin).run();
}
```

### bevy-inspector-egui (Debug Builds)

```toml
# crates/client/Cargo.toml
[features]
debug = ["dep:bevy-inspector-egui"]
```

Run: `cargo run -p fellytip-client --features debug`

### Ralph Loop (`tools/ralph/`)

```rust
// tools/ralph/src/brp.rs
pub struct BrpClient { base_url: String }
impl BrpClient {
    pub fn server() -> Self { Self { base_url: "http://localhost:15702".into() } }
    pub fn headless_client() -> Self { Self { base_url: "http://localhost:15703".into() } }
    pub fn query(&self, components: &[&str]) -> Vec<serde_json::Value> { ... }
    pub fn call(&self, method: &str, params: serde_json::Value) -> serde_json::Value { ... }
}
```

Run scenarios:
```bash
cargo run -p fellytip-server &
cargo run -p fellytip-client -- --headless &
cargo run -p ralph -- --scenario all
```

Claude's self-driven loop: launch → run ralph → read output → edit → rebuild → repeat.

---

## MVP Milestones

| Milestone | Deliverable | Status |
|---|---|---|
| **0 - Bones** | Two binaries connect; `WorldPosition` replicated | ✅ Done |
| **0b - Ralph** | BRP wired; ralph `basic_movement` scenario | ✅ Done |
| **1 - Living World** | World sim; factions; ecology; story log | ✅ Done (egui viewer + BRP custom methods pending) |
| **2 - First Blood** | Combat resolves; proptest passing | ✅ Done (ralph `combat_resolves` scenario pending) |
| **3 - Party Play** | 4 simultaneous clients; party system | ✅ Done (visibility culling + HUD pending) |
| **4 - MVF** | 3 classes, 1 dungeon, faction consequences | 🚧 Scaffold done — classes, abilities, full ralph suite remaining |
| **World Gen** | fBm terrain, biomes, rivers, settlements, roads, history warp | ✅ Done |

---

## Plugin Wiring

### Server `main.rs`
```rust
App::new()
    .add_plugins(MinimalPlugins)
    .add_plugins(bevy_remote::RemotePlugin::default())
    .add_plugins(bevy_remote::http::RemoteHttpPlugin::default())
    .add_plugins(FellytipProtocolPlugin)
    .add_plugins(ServerPlugin { config: build_server_config() })
    .add_plugins(WorldSimPlugin)
    .add_plugins(AiPlugin)
    .add_plugins(EcologyPlugin)
    .add_plugins(StoryPlugin)
    .add_plugins(CombatPlugin)
    .add_plugins(PersistencePlugin)
    .add_plugins(NetworkServerPlugin)
    .run();
```

---

## Implementation Order

1. ✅ Workspace scaffold — all crates stub-compile clean
2. ✅ Lightyear wiring — connect, send one message, disconnect
3. ✅ `WorldPosition` replication end-to-end → **Milestone 0**
4. ✅ BRP on server (15702) + headless client (15703); ralph `basic_movement` scenario → **Milestone 0b**
5. ✅ SQLite migrations + persistence skeleton
6. ✅ `WorldSimSchedule` infrastructure (custom 1 Hz Bevy schedule)
7. ✅ Ecology system + proptest tests
8. ✅ Faction data + NPC goal AI + wander movement
9. ✅ Story log writer (`WriteStoryEvent` message, `StoryLog` resource) → **Milestone 1** scaffold
10. ✅ Pure combat rules + proptest harness (`rules.rs`, `interrupt.rs`)
11. ✅ ECS combat bridge + interrupt stack (`CombatPlugin`, `FixedUpdate`) → **Milestone 2** scaffold
12. ✅ Party system + tests (`PartyRegistry`, max-4 enforcement) → **Milestone 3** scaffold
13. ✅ Dungeon boss spawn, `PlayerInput`, `WorldClock` → **Milestone 4** foundation
14. ✅ Tile map — `WorldPosition.z`, `TileLayer`/`TileColumn`/`WorldMap`, 512×512, stacked underground layers
15. ✅ Procedural noise — `smooth_step`, `lattice_hash`, `value_noise`, `fbm` in `math.rs`
16. ✅ fBm terrain — replaces box-blur white noise; continent-scale base frequency; seed-offset for uniqueness
17. ✅ Whittaker biome classification — temperature (latitude + altitude) × precipitation → 10 biome `TileKind` variants
18. ✅ River generation — steepest-descent flow accumulation; drainage ≥ 800 → `TileKind::River`
19. ✅ Settlement placement — Poisson-disk surface grid + BFS Underdark connected-component siting
20. ✅ Territory + roads — BFS flood-fill territories; Kruskal MST + Bresenham road network → `WorldMap::road_tiles`
21. ✅ History pre-simulation — `MapGenPlugin` runs `WorldSimSchedule` × 200 on `Startup` before clients connect
22. ✅ `world_gen` tool — ASCII map preview with biome chars, settlement overlays, road network

### Remaining work (next up)

- egui story log viewer + custom BRP methods (`fellytip/story_by_tag`, `fellytip/faction_state`)
- ralph `ecology_loop` and `combat_resolves` scenarios
- Visibility culling per party (interest management)
- 3 character classes wired to `CharacterClass` enum
- Ability system hooked into `InterruptFrame::ResolvingAbility`
- Dungeon room transitions
- SQLite autosave flush (story events, player state)
- Full ralph suite → **Milestone 4** shipped

---

## Verification

```bash
cargo test -p fellytip-shared          # pure logic unit tests
cargo test -p combat_sim               # 100k+ proptest combat traces
cargo test -p fellytip-server          # integration tests (MockTransport)
cargo clippy --workspace -- -D warnings
cargo run -p ralph -- --scenario all   # live end-to-end via BRP
cargo run -p fellytip-client --features debug  # visual ECS inspector
```
