# Fellytip — Product Requirements

## What it is

Fellytip is a multiplayer action RPG where **the world is the protagonist**. Factions expand, ecology fluctuates, and stories accumulate whether or not any player is connected. Players enter a living world that has already been running, observe the ongoing dynamics, and choose where to intervene.

## Core requirements

### World simulation
- The world ticks continuously and independently of player presence.
- Factions pursue goals, fight each other, and form alliances over time.
- Ecology populations fluctuate via predator/prey dynamics; collapses and recoveries become story events.
- The world has a pre-simulated history before the first player joins.

### World structure
- The world is a 512×512 tile grid with true 3-D elevation (`x, y, z`).
- Terrain is generated procedurally and deterministically from a seed — same seed, same world.
- Biomes reflect climate (temperature + precipitation): desert, savanna, tropical forest, grassland, temperate forest, taiga, tundra, polar, and others.
- Rivers form naturally from terrain drainage.
- Surface settlements are placed based on habitability; underground cities exist in large Underdark caverns.
- Settlements are connected by a road network.
- The world has three vertical tiers: surface, shallow caves (~15 m below), and Underdark (~65 m below).

### Players
- Up to 4 players can form a party and play simultaneously.
- Players move in real time using WASD; elevation follows terrain automatically.
- Players can attack NPCs and earn XP toward level advancement.
- Player position and health are replicated to all connected clients.

### Combat
- Attack resolution uses dice mechanics (d20 attack roll, d8 damage).
- Combat is a pure function of game state — never rolls dice internally, always injects them so tests can drive deterministic traces.
- NPCs die, drop XP, and generate story events on death.
- Combat is interruptible (reactions can nest via an interrupt stack).

### Persistence
- World state survives server restarts: player positions, story log, faction state, ecology.
- The SQLite database is the only external dependency.

### Observability
- The server exposes a BRP (Bevy Remote Protocol) HTTP API on port 15702.
- A headless client can run without a window for automated testing.
- The `ralph` tool drives end-to-end scenario tests via BRP.

### Rendering
- Default view is top-down 2-D.
- The coordinate system supports upgrading to isometric rendering without touching simulation or networking code.

## Non-requirements (explicitly out of scope for now)

- Real-time voice or text chat
- More than 4 players per session
- Player-to-player trading or economy
- Procedural quest generation
- Mobile or console targets
