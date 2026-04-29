# System: Combat

Combat is built in three layers: pure rules, an interrupt stack, and a thin ECS bridge. The layers are explicitly separated so the rules can be tested independently of Bevy.

Exact dice sizes, damage modifiers, and XP thresholds are defined in `crates/shared/src/combat/` — that directory is the authority.

## Layer 1 — Pure rules (`crates/shared/src/combat/`)

Combat rules are ordinary Rust functions. They take game state and explicit dice rolls as inputs and return effects as outputs. They never generate randomness internally.

**Key functions:**
- `resolve_attack_roll(attacker, defender, roll)` — `d20 + ability_mod + proficiency_bonus(level) >= defender.armor_class`; natural 20 = crit, natural 1 = always miss. See `docs/dnd5e-srd-reference.md`.
- `resolve_damage(result, attacker, defender, dmg_roll)` — applies ability modifier to the damage roll; returns a `TakeDamage` effect. No flat damage reduction — AC governs whether you're hit, not how much damage you take (5e SRD base rules).
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

The bridge runs in `FixedUpdate`. System ordering:

**`check_faction_aggression`** (runs before `process_player_input`)
Queries all faction NPCs without a pending attack and all players. For each NPC within 10 tiles of a player, inserts `PendingAttack` if the faction's `is_aggressive` flag is set or the player's standing tier is Hostile/Hated. Uses `PlayerReputationMap` and `FactionRegistry` resources. See `docs/systems/factions.md` for aggression rules.

**`tick_action_cooldowns`** (runs before `process_player_input`)
Decrements real-time CD timers on `ActionCooldowns` and restores the corresponding `ActionBudget` boolean when each timer reaches zero. Round duration = 6 s (`ROUND_SECONDS`).

**`process_player_input`**
Reads `PlayerInput` messages from each connected client. Applies movement to `WorldPosition`. When `BasicAttack` is present and the player's `ActionBudget.action` is available, consumes the slot, starts a 6 s `ActionCooldowns.action_cd`, emits `ActionUsedEvent`, and inserts `PendingAttack`. Actions requested while a slot is spent are silently discarded. If no `ActionBudget` is present the action always proceeds (backward-compatible). When `UseAbility(id)` is present the same Action-slot check applies before inserting `PendingAbility`.

**`initiate_attacks`**
Converts `PendingAttack` markers into `InterruptFrame::ResolvingAttack` values pushed onto the attacker's `InterruptStack`. Removes the marker.

**`initiate_abilities`**
Converts `PendingAbility` markers into `InterruptFrame::ResolvingAbility` with pre-rolled dice (`[attack_d20, dmg_d8_1, dmg_d8_2]`) in `AbilityContext.rolls`. Removes the marker.

**`resolve_interrupts`**
Runs in several phases each tick:
1. Build `id → Entity`, `Entity → XP reward`, and `Entity → player UUID` lookup maps.
2. Build a `CombatState` snapshot from current `Health`, `CombatParticipant`, and `FactionMember` components. `CombatantSnapshot.faction` is populated from `FactionMember` when present.
3. Peek at each non-empty `InterruptStack`'s top frame; if it's `ResolvingAttack`, record `(entity, defender_id, attack_roll)` in `attack_meta` for miss/crit detection. Then call `step()` once per stack; collect effects.
4. Apply effects via `get_mut` (avoids borrow conflicts with the step phase). For `TakeDamage`, emits `ClientDamageMsg` with `damage`, `is_critical` (attack_roll == 20), and `is_miss: false`. Resolve killer UUID from `GameEntityId` on the attacker.
4b. Emit miss `ClientDamageMsg` (with `is_miss: true`, `damage: 0`) for any attack that produced no `TakeDamage` effect. The miss is positioned at the defender's world location.
4c. Apply faction standing deltas for each kill.
5. Award XP to attackers whose target died.
6. Emit `WriteStoryEvent` for each death.
7. Despawn dead entities.

## Client-side combat feedback (`crates/client/src/plugins/`)

**Entity picking (`target_select.rs`)**
Each frame, projects every hostile entity's world position to viewport coordinates using `Camera::world_to_viewport`. The closest enemy within `PICK_RADIUS_PX` (60 px) of the cursor is stored in `HoveredTarget`. Left-clicking with a `HoveredTarget` sets the `target_uuid` in the attack intent, directing the server to attack that specific enemy.

**Context menu (`action_menu.rs`)**
Right-clicking captures the current `HoveredTarget` into `ActionMenuState.context`. If a hostile entity is hovered (`TargetContext::Hostile { uuid }`), the menu shows targeted combat options (Attack, Shove stub, Grapple stub, Class Action). Otherwise, a generic tile/empty-space menu is shown (Attack nearest, Ability, Dodge, Examine stub).

**Floating combat text (`floating_text.rs`)**
Listens to `ClientDamageMsg`. Each message spawns a `FloatEntry` in `FloatingTextQueue`:
- Hit: white number, 14 px, fades over 1.2 s
- Miss: grey "Miss!", 13 px, fades over 1.0 s
- Critical hit: gold `"<dmg>!"`, 20 px, fades over 1.5 s

Text positions are projected to screen each frame and float upward 40 px over the entry's lifetime.

## Levelling

XP required to reach the next level is computed by `xp_to_next_level(level)` in `crates/shared/src/combat/rules.rs`, using the official 5e SRD table (see `docs/dnd5e-srd-reference.md`). Multiple level-ups from a single kill are applied in a loop.

On each level-up, HP increases by rolling the class hit die + CON modifier (minimum 1), implemented in `hp_on_level_up()`. The player receives a full heal on level-up. Level is kept in sync between `Experience.level` and `CombatParticipant.level` so the proficiency bonus in attack rolls stays current.

### Ability Score Improvements (ASI)

When an entity reaches an ASI level, a `PendingAsi` marker component is inserted by the level-up logic in `resolve_interrupts`. ASI levels per class (SRD):

| Class | ASI levels |
|---|---|
| Fighter / Warrior | 4, 6, 8, 12, 14, 16, 19 |
| Rogue | 4, 8, 10, 12, 16, 19 |
| All others | 4, 8, 12, 16, 19 |

`asi_levels_for_class(class)` in `crates/shared/src/combat/types.rs` returns these sets.

The `apply_npc_asi` system (FixedUpdate, after `resolve_interrupts`) immediately resolves `PendingAsi` for NPC entities (those carrying `EntityKind`) by calling `AbilityScores::with_npc_asi(class)`, which applies +2 to the class's primary stat (capped at 20). Player entities are left with `PendingAsi` pending UI resolution (future work).

## Server-only combat components

| Component | Description |
|---|---|
| `CombatParticipant` | Holds `CombatantId`, `InterruptStack`, `class`, `level`, `armor_class` (AC), `strength`, `dexterity`, `constitution` |
| `ExperienceReward(u32)` | XP granted to the killer; only on NPCs and bosses. Set from CR table in `docs/dnd5e-srd-reference.md`. |
| `PendingAttack { target }` | Transient marker; consumed by `initiate_attacks` |
| `PendingAbility { target, ability_id }` | Transient marker; consumed by `initiate_abilities` |
| `PendingAsi` | Marker inserted on ASI levels; resolved next tick for NPCs by `apply_npc_asi`; stays pending for players until UI |
| `ActionCooldowns` | Server-only CD timers (`action_cd`, `bonus_action_cd`, `reaction_cd` in seconds) that drive `ActionBudget` restoration |
| `PlayerEntity(Entity)` | Links a `ClientOf` entity to its spawned player entity |
| `GameEntityId(Uuid)` | Stable cross-session identity on player entities; `CombatantId.0 == GameEntityId.0` for all players |

## Shared action-economy components (replicated)

| Component | Description |
|---|---|
| `ActionBudget` | Per-round booleans: `action`, `bonus_action`, `reaction` (true = available) plus `movement_remaining: f32`. Default = all true, 15.0 movement. Registered in `FellytipProtocolPlugin` for BRP inspection. |
| `ActionSlot` | Enum: `Action`, `BonusAction`, `Reaction` — which slot an ability consumes. |
| `ActionUsedEvent` | `Message` broadcast each time a player spends a slot; carries `entity` and `slot`. Hook for animation/audio. |

## Character classes

Three classes are implemented in `CharacterClass` (Warrior, Rogue, Mage). Each class has:

- **Distinct attack roll modifier**: Warrior uses STR, Rogue uses DEX (finesse), Mage uses INT.
- **Distinct damage modifier**: same class-appropriate modifier applied to damage rolls.
- **Class-specific ability**:

| Class   | Ability ID | Name         | Description |
|---------|-----------|--------------|-------------|
| Warrior | 1         | StrongAttack | 2×d8 damage + "weakened" status on hit |
| Rogue   | 2         | SneakAttack  | d6+d6 damage + "poisoned" status on hit |
| Mage    | 3         | ArcaneBlast  | Auto-hit d8+INT mod + "scorched" status (ignores AC) |

## Dungeon boss phased abilities

The Hollow King has three combat phases tracked by `BossPhase` component on the boss entity. Transitions are strictly one-way (Phase1 → Phase2 → Phase3) and logged as warnings. `DungeonPlugin` runs `tick_boss_phase_transitions` on every `FixedUpdate` tick.

| Phase | HP threshold | Ability ID | Behaviour |
|-------|-------------|-----------|-----------|
| 1     | > 50 %      | 1         | StrongAttack: 2×d8 + "weakened" on target |
| 2     | 25–50 %     | 5         | BossRage: d10+3 heavy strike + "enraged" self-buff |
| 3     | < 25 %      | 6         | BossFrenzy: two d6 strikes + "weakened" on target if any damage landed |

`BossPhase::ability_id()` maps each phase variant to its ability ID. The ECS bridge reads this during `initiate_attacks` to route the boss to the correct `resolve_ability` branch.

## Current state

Basic attack (Space) → damage → death → XP loop is fully functional. StrongAttack (Q) is ability 1. Three character classes are implemented with class-appropriate modifiers and distinct abilities (ids 1–3). The dungeon boss ("The Hollow King") has phased abilities at 50% and 25% HP thresholds. Movement reactions (`ResolvingMovement`) are scaffolded but unimplemented. Faction alert state (`FactionAlertState`) raises NPC patrol radius and speed after any battle; see `docs/systems/factions.md`.

Action economy is implemented in real-time mode: the player entity carries `ActionBudget` (all slots true on spawn) and `ActionCooldowns`. Each basic attack or ability use consumes the `Action` slot and starts a 6 s CD; attempts while the slot is spent are discarded. The HUD shows three pip indicators (● A, ● B, ◆ R) below the XP bar: bright = available, grey = spent. Bonus-action and reaction economy tracking is wired but not yet consumed by any specific ability.
