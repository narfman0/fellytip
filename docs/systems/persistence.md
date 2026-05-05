# System: Persistence

World state is stored in a SQLite database (`fellytip.db`) via `sqlx 0.8`. The database is opened and migrated synchronously on server startup by `PersistencePlugin`, before the ECS world starts ticking.

The `Db` resource wraps the live connection pool and is available to any server system.

## Migrations

Migrations live in `migrations/` and are embedded at compile time via `sqlx::migrate!`. They run automatically on startup; the server will not start if a migration fails. The migration files are the authority on the current schema.

| File | Contents |
|---|---|
| `migrations/001_initial.sql` | `players`, `story_events`, `factions`, `ecology_state`, `world_meta` tables |
| `migrations/002_world_map.sql` | `world_map` table; adds `pos_z` column to `players` |
| `migrations/003_reputation.sql` | `player_faction_standing` table for per-player faction reputation scores |

## What is and isn't persisted

| Data | Persisted | Notes |
|---|---|---|
| Story events | Yes — flushed every `FLUSH_INTERVAL_TICKS` world-sim ticks | See `story.rs` for the interval constant |
| Player state | Yes — saved on disconnect | `WorldPosition`, `Health`, `Experience`, class; UUID used as name placeholder |
| Faction state | Yes — flushed once at startup (after `seed_factions` + `history_warp`) | `ai.rs::flush_factions_to_db`; JSON-encoded `resources`, `goals`, `territory` |
| Ecology state | Yes — flushed every `ECOLOGY_FLUSH_INTERVAL` (30) world-sim ticks | `ecology.rs::flush_ecology_to_db`; prey/predator counts per region |
| World map | Yes — cached on first generation | Serialised to `world_{seed}_{width}x{height}.bin` (bincode); path stored in `world_meta`. Subsequent restarts skip `generate_map` + `generate_roads`. Falls back to regeneration on any I/O or deserialise error. |
| Player faction reputation | Stub — schema exists, load/save hooks not yet wired | `player_faction_standing` table in `003_reputation.sql` |

## Story event flush (`crates/game/src/plugins/story.rs`)

Every `FLUSH_INTERVAL_TICKS` world-sim ticks, all accumulated `StoryEvent` entries in `StoryLog` are drained and written to the `story_events` table via `sqlx::query`. Event `kind` is stored as a debug-formatted string; `participants` and `lore_tags` as JSON arrays.

## Faction flush (`crates/game/src/plugins/ai.rs`)

`flush_factions_to_db` runs once at `Startup`, after `history_warp`. It serializes each `Faction` in `FactionRegistry` and UPSERTs it into the `factions` table. `resources` and `goals` are stored as JSON strings. `territory` is stored as a JSON array of region ID strings. Requires `serde::Serialize` on `Faction` and related types in `crates/world-types/src/faction.rs`.

## Ecology flush (`crates/game/src/plugins/ecology.rs`)

`flush_ecology_to_db` runs on `WorldSimSchedule` every `ECOLOGY_FLUSH_INTERVAL` (30) ticks. For each `RegionEcology` in `EcologyState`, it UPSERTs prey and predator counts into `ecology_state (species_id, region_id, count)`. Requires `serde::Serialize` on ecology types in `crates/world-types/src/ecology.rs`.

## WorldWatch observer tool (`tools/worldwatch`)

Standalone eframe+tray-icon binary that monitors the server. Reads from:
- BRP (`http://localhost:15702`): entity counts, player count, `WorldSimTick` resource
- SQLite (`fellytip.db`): factions, ecology, story events

Run with `cargo run -p worldwatch`. DB path can be overridden via `WORLDWATCH_DB` env var.

## World map cache (`crates/game/src/plugins/map_gen.rs`)

On first startup `generate_world` serialises the full `WorldMap` (columns + road_tiles) using `bincode` to a file named `world_{seed}_{width}x{height}.bin` in the working directory. The path is then upserted into `world_meta` under the key `"world_map_file"`. On subsequent startups the path is read from `world_meta`, the file is loaded and deserialised, and `generate_map` + `generate_roads` are skipped (the expensive fBm passes, ~200–500 ms). `generate_settlements` and `assign_territories` always run since they are fast, deterministic, and required for the `Settlements` ECS resource.

A cached map is rejected and regeneration runs if:
- No `world_map_file` entry exists in `world_meta` (first run or DB wiped).
- The file is missing or unreadable.
- Deserialisation fails (corrupt file).
- The loaded map's `seed` doesn't match `WORLD_SEED`, or `road_tiles` is empty.

Save and load failures are non-fatal: a warning is logged and the server always proceeds.

## Character autosave (`crates/game/src/plugins/character_persistence.rs`)

`CharacterPersistencePlugin` autosaves the local player every ~60 wall-clock seconds (`AUTOSAVE_INTERVAL`), UPSERTing class, level, XP, HP, and position into the `players` table. On spawn, an existing row is restored in preference to the class selection screen (see `spawn_player_on_class_choice` in `crates/game/src/lib.rs`). The persisted UUID is stored in `world_meta` so the same character is reloaded across restarts.
