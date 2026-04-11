# System: Persistence

World state is stored in a SQLite database (`fellytip.db`) via `sqlx 0.8`. The database is opened and migrated synchronously on server startup by `PersistencePlugin`, before the ECS world starts ticking.

The `Db` resource wraps the live connection pool and is available to any server system.

## Migrations

Migrations live in `migrations/` and are embedded at compile time via `sqlx::migrate!`. They run automatically on startup; the server will not start if a migration fails. The migration files are the authority on the current schema.

| File | Contents |
|---|---|
| `migrations/001_initial.sql` | `players`, `story_events`, `factions`, `ecology_state`, `world_meta` tables |
| `migrations/002_world_map.sql` | `world_map` table; adds `pos_z` column to `players` |

## What is and isn't persisted

| Data | Persisted | Notes |
|---|---|---|
| Story events | Yes — flushed every `FLUSH_INTERVAL_TICKS` world-sim ticks | See `story.rs` for the interval constant |
| Player state | Yes — saved on disconnect | `WorldPosition`, `Health`, `Experience`, class; UUID used as name placeholder |
| Faction state | Schema exists | Load/save not yet wired |
| Ecology state | Schema exists | Load/save not yet wired |
| World map | Schema exists | Regenerated from seed each startup |

## Story event flush (`crates/server/src/plugins/story.rs`)

Every `FLUSH_INTERVAL_TICKS` world-sim ticks, all accumulated `StoryEvent` entries in `StoryLog` are drained and written to the `story_events` table via `sqlx::query`. Event `kind` is stored as a debug-formatted string; `participants` and `lore_tags` as JSON arrays.

## Player save on disconnect (`crates/server/src/main.rs`)

An observer on `Add<Disconnected>` looks up the `PlayerEntity` linked to the disconnecting client and UPSERTs its `WorldPosition`, `Health`, and `Experience` into the `players` table.
