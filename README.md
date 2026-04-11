# Fellytip

Multiplayer action RPG in Rust/Bevy where the world simulates itself independent of player presence.

**Status:** Early development — server and client connect, `WorldPosition` is replicated, SQLite persistence skeleton is in place.

## Prerequisites

- Rust stable (edition 2021)
- A C linker (MSVC on Windows, `gcc`/`clang` on Linux/macOS)

## Run

```bash
# Terminal 1 — game server
cargo run -p fellytip-server

# Terminal 2 — headless client (connects automatically)
cargo run -p fellytip-client -- --headless
```

## Verify with ralph

`ralph` is an automated test driver that asserts live world state via the Bevy Remote Protocol (BRP).

```bash
# With server + headless client already running:
cargo run -p ralph -- --scenario basic_movement
```

## Tests

```bash
cargo test -p fellytip-shared   # pure logic unit tests
cargo clippy --workspace -- -D warnings
```

## Crates

| Crate | Purpose |
|---|---|
| `crates/shared` | Types, protocol, combat rules — no I/O |
| `crates/server` | Bevy server: networking, world sim, persistence |
| `crates/client` | Bevy client: networking, rendering (WIP) |
| `tools/ralph` | BRP test driver |
| `tools/combat_sim` | proptest harness for combat rules |
