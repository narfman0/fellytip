# System: Persistence

World state is stored in a SQLite database (`fellytip.db`) via `sqlx 0.8`. The database is opened and migrated synchronously on server startup by `PersistencePlugin`, before the ECS world starts ticking.

The `Db` resource wraps the live connection pool and is available to any server system.

## Migrations

Migrations live in `migrations/` and are embedded at compile time via `sqlx::migrate!`. They run automatically on startup; the server will not start if a migration fails. The migration files are the authority on the current schema.

| File | Contents |
|---|---|
| `migrations/001_initial.sql` | `players`, `story_events`, `factions`, `ecology_state`, `world_meta` tables |
| `migrations/002_world_map.sql` | `world_map` table; adds `pos_z` column to `players` |

## What is and isn't persisted yet

| Data | Persisted | Notes |
|---|---|---|
| Story events | Partial — flush not yet wired | `StoryLog` resource accumulates in memory |
| Player position / health | Schema exists | Save not yet called each session |
| Faction state | Schema exists | Load/save not yet wired |
| Ecology state | Schema exists | Load/save not yet wired |
| World map | Schema exists | Regenerated from seed each startup |

The persistence stub is in place and migrations run correctly. The autosave loop (flush every N world-sim ticks — see `world_sim.rs`) is the next persistence milestone.
