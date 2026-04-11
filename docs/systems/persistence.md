# System: Persistence

World state is stored in a SQLite database (`fellytip.db`) via `sqlx 0.8`. The database is opened and migrated synchronously on server startup by `PersistencePlugin`, before the ECS world starts ticking.

The `Db` resource wraps the live connection pool and is available to any server system.

## Migrations

Migrations live in `migrations/` and are embedded at compile time via `sqlx::migrate!`. They run automatically on startup; the server will not start if a migration fails.

| File | Contents |
|---|---|
| `001_initial.sql` | `players`, `story_events`, `factions`, `ecology_state`, `world_meta` tables |
| `002_world_map.sql` | `world_map` table (seed + JSON tile grid); adds `pos_z` column to `players` |

## Schema overview

**`players`** — one row per connected player account. Stores class, level, health, position, last-seen tick.

**`story_events`** — append-only log of world events. `kind` is a JSON-serialised `StoryEventKind`; `lore_tags` is a JSON array for filtering.

**`factions`** — faction goals, resources, and territory as JSON blobs. Reloaded on startup to restore world-sim state.

**`ecology_state`** — current predator/prey counts per (species, region) pair.

**`world_meta`** — key/value store for world-level metadata (e.g. current world seed, server version).

**`world_map`** — the full tile grid serialised as JSON, keyed by seed. Allows the server to reload a previously generated map rather than regenerating it. Not yet used; the server currently regenerates from seed on every startup.

## What is and isn't persisted yet

| Data | Persisted | Notes |
|---|---|---|
| Story events | Partial — flush not yet wired | `StoryLog` resource accumulates in memory |
| Player position / health | Schema exists | Save not yet called each session |
| Faction state | Schema exists | Load/save not yet wired |
| Ecology state | Schema exists | Load/save not yet wired |
| World map | Schema exists | Regenerated from seed each startup |

The persistence stub is in place and migrations run correctly. The autosave loop (flush every 300 world-sim ticks ≈ 5 minutes) is the next persistence milestone.
