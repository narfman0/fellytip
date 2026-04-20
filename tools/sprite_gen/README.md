# sprite_gen

CLI that reads `assets/bestiary.toml` and emits a 2.5D billboard atlas
(PNG) + a Bevy-friendly grid manifest (RON) per entity.

## Usage

```bash
# Deterministic mock backend (default). Safe for CI and unit tests.
cargo run -p sprite_gen -- --all --output-dir crates/client/assets/sprites/

# Dry-run prints every prompt the backend would receive, no API calls.
cargo run -p sprite_gen -- --all --dry-run

# DALL-E 3 backend.
SPRITE_GEN_API_KEY=sk-... cargo run -p sprite_gen -- \
    --backend openai --entity goblin_scout \
    --output-dir crates/client/assets/sprites/

# 4-way parallel, skip entities whose atlas is newer than bestiary.toml.
cargo run -p sprite_gen -- --all --incremental --workers 4
```

## Flags

| Flag | Default | Meaning |
|---|---|---|
| `--all` | — | Generate every entity in the bestiary. |
| `--entity ID` | — | Generate one entity by id. |
| `--output-dir DIR` | `crates/client/assets/sprites` | Where PNG + RON land. |
| `--bestiary PATH` | `assets/bestiary.toml` | Source file. |
| `--dry-run` | off | Print prompts only, no images. |
| `--incremental` | off | Skip entities whose atlas mtime > bestiary mtime. |
| `--backend mock\|openai` | `mock` | Image source. |
| `--workers N` | `1` | Max parallel generator calls. |
| `--no-quantise` | off | Disable the 16-color palette quantiser. |

## Environment (for `--backend openai`)

| Var | Default | Purpose |
|---|---|---|
| `SPRITE_GEN_API_KEY` | **required** | OpenAI bearer token. |
| `SPRITE_GEN_ENDPOINT` | `https://api.openai.com/v1/images/generations` | Swap for a compatible service. |
| `SPRITE_GEN_MODEL` | `dall-e-3` | Image model id. |

A missing `SPRITE_GEN_API_KEY` with `--backend openai` (and no `--dry-run`)
exits non-zero with a clear message.

## Licensing note

OpenAI DALL-E outputs may be used commercially under the current OpenAI
Terms of Use, but users are responsible for verifying current policy and
for any third-party IP implications (e.g. resemblance to copyrighted
characters). Re-verify before shipping generated sprites.

## Determinism

- Atlas layout is a pure function of the bestiary entry.
- `MockGenerator` is FNV-hashed over `(entity_id, direction, frame)`.
- The OpenAI backend folds a `frame_seed(entity_id, direction, frame)` into
  the prompt (DALL-E 3 has no seed parameter; Stable-Diffusion backends
  will consume it directly when that backend lands).
- Palette quantisation (`NeuQuant`) is deterministic given a fixed input.

Together these mean a rerun over the same bestiary + same backend produces
byte-identical output — so the CI `--incremental` gate is reliable.
