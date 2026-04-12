# WorldWatch

Windows system-tray app for live Fellytip server monitoring.

## Usage

```bash
# With server already running:
cargo run -p worldwatch
```

The app starts **hidden**. A tray icon appears in the taskbar notification area — click it (or right-click → **Show / Hide**) to open the window. Right-click → **Quit** to exit.

## Tabs

| Tab | What it shows |
|---|---|
| **Overview** | Server online/offline, current world tick, total entities, player count, NPC count (refreshes every 2 s via BRP) |
| **Factions** | Name, food, gold, military strength, and top current goal for every faction (read from SQLite) |
| **Ecology** | Per-region prey/predator species, population counts, and collapse status (read from SQLite, flushes every 30 s) |
| **Story** | Last 50 story events: world day, tick, event kind, and lore tags (read from SQLite, flushes every 300 ticks / ~5 min) |
| **Query** | Type a full component type path (e.g. `fellytip_shared::components::WorldPosition`) and click **Query** to run a live `world.query` BRP call and inspect raw JSON results |

## DB path resolution

First match wins:

1. `WORLDWATCH_DB` environment variable
2. `./fellytip.db` (default when launched from workspace root)

```bash
WORLDWATCH_DB=/path/to/fellytip.db cargo run -p worldwatch
```

## Data sources

- **BRP** (port 15702) — entity counts, world tick; polled live every 2 s
- **SQLite** (`fellytip.db`) — factions, ecology, story; reflects last server flush
