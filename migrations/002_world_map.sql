-- World map persistence: stores the full tile grid as JSON so the server
-- can reload the same map after restart without re-generating from seed.
CREATE TABLE IF NOT EXISTS world_map (
    seed       INTEGER PRIMARY KEY,
    tiles_json TEXT    NOT NULL   -- JSON-serialised Vec<TileColumn>
);

-- Add elevation column to players (additive, SQLite-safe).
ALTER TABLE players ADD COLUMN pos_z REAL NOT NULL DEFAULT 0;
