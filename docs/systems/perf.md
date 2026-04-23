# System: Adaptive performance throttling

`crates/server/src/plugins/perf.rs` samples how long each world-sim tick takes and derives an `AdaptiveScheduler` level that AI, pathfinding, and ecology systems read to decide whether to throttle their work. The goal is to keep the 1 Hz `WorldSimSchedule` inside its tick budget even under spikes without visibly degrading gameplay near the player.

## Tick budget

`AI_TICK_BUDGET_MS = 50.0` — the target tick budget for `WorldSimSchedule`. Anything above this is considered over-budget.

## TickTimings

A rolling 60-entry ring buffer of tick durations in milliseconds. Written by the `record_tick_start` / `record_tick_end` bookends which run in the `PerfRecordStart` and `PerfRecordEnd` system sets on `WorldSimSchedule`.

`TickTimings` also tracks:

- `consecutive_over` — ticks in a row over budget, reset to 0 when a tick comes in under budget.
- `consecutive_under` — ticks in a row under budget, reset to 0 when one goes over.
- `p95_ms()` — upper-quantile latency, used as the signal to derive the throttle level (P95 is robust against a single slow tick).

## ThrottleLevel

Four-level enum ranked from least to most throttled:

| Level | When | Expected downstream effect |
|---|---|---|
| `Full` | P95 ≤ `budget × 0.75` | Everyone runs full rate |
| `Reduced` | `budget × 0.75 < P95 ≤ budget` | Cheap throttling — skip some Warm work |
| `Minimal` | `budget < P95 ≤ budget × 1.5` | Only Hot-zone AI ticks; Warm/Frozen stall |
| `Suspended` | P95 > `budget × 1.5` | Skip everything except critical gameplay near players |

`ThrottleLevel` derives `Ord` so `max(a, b)` escalates in the "more throttled" direction — callers can combine signals safely. `deescalate_one()` drops exactly one step toward `Full`.

## AdaptiveScheduler (hysteresis)

`AdaptiveScheduler.level` is the single throttle level every downstream system reads. `update_throttle_level` runs in the `PerfThrottleUpdate` set (between the record-start and record-end bookends) and applies this policy:

- **Escalation is immediate.** If the newly derived level from `derive_level(p95)` is more throttled than the current level, jump there on the next tick. A single spike should slow AI right away.
- **De-escalation is hysteretic.** Dropping one step requires both:
  1. `derive_level(p95)` is below the current level, AND
  2. `TickTimings::consecutive_under >= DEESCALATE_AFTER` (currently `10`).

This prevents oscillation: a brief dip below budget isn't enough to flip back to `Full` while the rest of the window is still hot.

The de-escalation is always **exactly one step** per qualifying tick, so `Suspended → Minimal → Reduced → Full` takes at least three qualifying de-escalation windows.

## ClientFrameTimings

When the client is running in host mode (windowed + server logic in-process), the render thread can be the bottleneck rather than the world sim. `ClientFrameTimings` is a second rolling 60-entry buffer of `Update` delta-seconds written by the client's `track_frame_time` system. `under_pressure` flips to `true` when the rolling average frame time exceeds `HOST_FRAME_FLOOR_SECS = 1.0 / 30.0`.

When `under_pressure` is set, `update_throttle_level` bumps the derived level one step up before the escalation/hysteresis logic runs. So a heavy render frame can push `Full → Reduced` even when the server-side P95 is comfortable, giving the CPU back to rendering.

Headless runs don't install `track_frame_time` (it lives only in `add_windowed_plugins`), so `ClientFrameTimings` stays empty and `under_pressure` stays `false`.

## System set plumbing

`PerfPlugin` wires three `SystemSet`s onto `WorldSimSchedule`:

1. `PerfRecordStart` — `record_tick_start` stamps `TickStartTime`.
2. `PerfThrottleUpdate` — `update_throttle_level` publishes the new level. Runs **after** record-start so downstream systems in the same tick see the freshest level.
3. `PerfRecordEnd` — `record_tick_end` pushes the elapsed duration into `TickTimings`.

Downstream systems (AI, flow-field sampling, ecology) sit between `PerfThrottleUpdate` and `PerfRecordEnd` and read `Res<AdaptiveScheduler>` to decide whether to tick.

## Tests

`perf.rs` ships a dense unit-test suite covering: buffer trimming to 60 samples, consecutive-run counters, P95 on empty / sorted inputs, each threshold band in `derive_level`, `deescalate_one` transitions, immediate escalation, the 10-tick hysteresis de-escalation window, and the client-frame-pressure one-step bump.
