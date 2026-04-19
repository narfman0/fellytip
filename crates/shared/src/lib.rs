pub mod combat;
pub mod world;

pub mod components;
pub mod inputs;
pub mod math;
pub mod protocol;
pub mod resources;

// ── Network constants shared by server and client ────────────────────────────

/// Game-level protocol identifier; must match on both sides.
pub const PROTOCOL_ID: u64 = 0x0000_FE11_1950_0001;

/// Deterministic world seed shared between server and client.
///
/// The client regenerates the `WorldMap` locally from this seed so the server
/// does not need to replicate terrain data.
pub const WORLD_SEED: u64 = 42;

/// Shared symmetric key used by netcode.io.
/// Replace with a securely generated key before shipping.
pub const PRIVATE_KEY: [u8; 32] = [0u8; 32];

/// UDP port the server listens on.
pub const NET_PORT: u16 = 5000;

/// WebSocket port — used by browser (WASM) clients.
pub const WS_PORT: u16 = 5001;

/// Fixed-update tick rate (Hz) for combat / movement.
pub const TICK_HZ: f64 = 62.5;

/// Player movement speed in world units per second.
/// Shared between server authoritative movement and client-side prediction.
pub const PLAYER_SPEED: f32 = 2.5;
