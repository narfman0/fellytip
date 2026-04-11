# System: Combat

Combat is built in three layers: pure rules, an interrupt stack, and a thin ECS bridge. The layers are explicitly separated so the rules can be tested independently of Bevy.

Exact dice sizes, damage modifiers, and XP thresholds are defined in `crates/shared/src/combat/` — that directory is the authority.

## Layer 1 — Pure rules (`crates/shared/src/combat/`)

Combat rules are ordinary Rust functions. They take game state and explicit dice rolls as inputs and return effects as outputs. They never generate randomness internally.

**Key functions:**
- `resolve_attack_roll(attacker, defender, roll)` — compares the roll against the defender's armor class; returns hit or miss
- `resolve_damage(result, attacker, defender, dmg_roll)` — applies strength modifiers to the damage roll; returns a `TakeDamage` effect
- `resolve_ability(ability_id, caster, target, rolls)` — resolves an activated ability; pre-rolled dice passed as a slice. Ability 1 (StrongAttack) deals 2×d8 damage and applies `"weakened"` status on hit. Unknown IDs return empty effects.
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
Reads `PlayerInput` messages from each connected client. Applies movement to `WorldPosition`. When `BasicAttack` is present, inserts `PendingAttack`; when `UseAbility(id)` is present, inserts `PendingAbility`.

**`initiate_attacks`**
Converts `PendingAttack` markers into `InterruptFrame::ResolvingAttack` values pushed onto the attacker's `InterruptStack`. Removes the marker.

**`initiate_abilities`**
Converts `PendingAbility` markers into `InterruptFrame::ResolvingAbility` with pre-rolled dice (`[attack_d20, dmg_d8_1, dmg_d8_2]`) in `AbilityContext.rolls`. Removes the marker.

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

XP required to reach the next level is computed by `xp_to_next_level(level)` in `crates/shared/src/combat/`. Multiple level-ups from a single kill are applied in a loop.

## Server-only combat components

| Component | Description |
|---|---|
| `CombatParticipant` | Holds `CombatantId`, `InterruptStack`, armor, and strength |
| `ExperienceReward(u32)` | XP granted to the killer; only on NPCs and bosses |
| `PendingAttack { target }` | Transient marker; consumed by `initiate_attacks` |
| `PendingAbility { target, ability_id }` | Transient marker; consumed by `initiate_abilities` |
| `PlayerEntity(Entity)` | Links a `ClientOf` entity to its spawned player entity |

## Current state

Basic attack (Space) → damage → death → XP loop is fully functional. StrongAttack (Q) is implemented as ability 1: same attack roll, 2×d8 damage, applies `"weakened"` status on hit. The dungeon boss ("The Hollow King") can be killed by either attack. Movement reactions (`ResolvingMovement`) are scaffolded but unimplemented.
