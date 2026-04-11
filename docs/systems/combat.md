# System: Combat

Combat is built in three layers: pure rules, an interrupt stack, and a thin ECS bridge. The layers are explicitly separated so the rules can be tested independently of Bevy.

## Layer 1 — Pure rules (`crates/shared/src/combat/`)

Combat rules are ordinary Rust functions. They take game state and explicit dice rolls as inputs and return effects as outputs. They never generate randomness internally.

**Key functions:**
- `resolve_attack_roll(attacker, defender, roll)` — compares the d20 roll against the defender's armor class; returns hit or miss
- `resolve_damage(result, attacker, defender, dmg_roll)` — applies strength modifiers to the d8 damage roll; returns a `TakeDamage` effect
- `apply_effects(state, effects)` — applies a list of effects to a `CombatState` snapshot; may generate follow-on effects (e.g. a killing blow also emits `Die`)

`CombatantSnapshot` is a plain data struct copied from ECS components into the pure layer for each combat step.

## Layer 2 — Interrupt stack (`crates/shared/src/combat/interrupt.rs`)

The interrupt stack enables reactions to nest. An attack can be interrupted by a parry, which can itself be interrupted by a riposte, and so on. Each frame on the stack is one pending resolution.

Frame variants:
- `ResolvingAttack` — an attack in progress
- `ResolvingDamage` — damage calculation
- `ResolvingAbility` — an ability activation
- `ResolvingMovement` — a triggered movement

`InterruptStack::step(state, rng)` pops the top frame, calls the appropriate pure rule function with dice from the injected `rng` iterator, and returns the resulting effects. The ECS bridge calls `step()` once per tick for each non-empty stack.

The `InterruptFrame` match is exhaustive — no `_` wildcard. Every variant must be handled, which prevents silent fallthrough when new variants are added.

## Layer 3 — ECS bridge (`crates/server/src/plugins/combat.rs`)

The bridge runs in `FixedUpdate` as three chained systems:

**`process_player_input`**
Reads `PlayerInput` messages from each connected client. Applies movement to `WorldPosition`. When the `BasicAttack` action is present, looks up the target entity and inserts a `PendingAttack` marker component on the player entity.

**`initiate_attacks`**
Converts `PendingAttack` markers into `InterruptFrame::ResolvingAttack` values pushed onto the attacker's `InterruptStack`. Removes the marker.

**`resolve_interrupts`**
Runs in seven phases each tick:
1. Build `id → Entity` and `Entity → XP reward` lookup maps.
2. Build a `CombatState` snapshot from current `Health` and `CombatParticipant` components.
3. Step each non-empty `InterruptStack` once; collect effects.
4. Apply effects via `get_mut` (avoids borrow conflicts with the step phase).
5. Award XP to attackers whose target died.
6. Emit `WriteStoryEvent` for each death.
7. Despawn dead entities.

## Levelling

XP thresholds follow a simple linear formula: `xp_to_next_level(level) = 100 × level`. Level 1 needs 100 XP, level 2 needs 200, and so on. Multiple level-ups from a single kill are applied in a loop.

## Server-only combat components

| Component | Description |
|---|---|
| `CombatParticipant` | Holds `CombatantId`, `InterruptStack`, armor, and strength |
| `ExperienceReward(u32)` | XP granted to the killer; only on NPCs and bosses |
| `PendingAttack { target }` | Transient marker; consumed by `initiate_attacks` |
| `PlayerEntity(Entity)` | Links a `ClientOf` entity to its spawned player entity |

## Current state

Basic attack → damage → death → XP loop is fully functional. The dungeon boss ("The Hollow King", 500 HP) can be killed by a player. The ability system (`ResolvingAbility` frame) and movement reactions (`ResolvingMovement`) are scaffolded but no concrete abilities are implemented yet.
