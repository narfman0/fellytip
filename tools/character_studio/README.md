# character_studio

Desktop GUI for browsing, generating, and approving sprites for every entity in `assets/bestiary.toml`.

## Usage

```bash
cargo run -p character_studio
```

The studio opens a two-panel window:
- **Left panel** — entity list from the bestiary, with generation status indicators.
- **Right panel** — generate 4 variants for the selected entity, preview them, and approve one to save as `base.png`.

## Controls

1. Select an entity from the left list.
2. Choose a backend (Mock / OpenAI / Stability AI) and optionally toggle **Remove background**.
3. Click **Generate 4 variants** — thumbnails appear as each thread completes.
4. Click a thumbnail to select it; click **Approve selected → save base.png** to write it to disk.

## Environment (for real backends)

| Var | Purpose |
|---|---|
| `SPRITE_GEN_API_KEY` | OpenAI bearer token for DALL-E 3 |
| `SPRITE_GEN_ENDPOINT` | Optional: override the OpenAI endpoint |
| `SPRITE_GEN_MODEL` | Optional: override the model id (default `dall-e-3`) |
| `STABILITY_API_KEY` | Stability AI bearer token |

## Output

Approved images are written to `assets/sprites/{entity_id}/base.png`.

## Licensing note

OpenAI DALL-E outputs may be used commercially under the current OpenAI Terms of Use, but users are responsible for verifying current policy and for any third-party IP implications (e.g. resemblance to copyrighted characters). Re-verify before shipping generated sprites.
