-- Player character persistence: stores per-player class, level, XP, HP, and position.
-- Extends the existing players table with xp and faction_standings columns.

-- Add XP column (additive, SQLite-safe).
ALTER TABLE players ADD COLUMN xp INTEGER NOT NULL DEFAULT 0;

-- Add XP-to-next column.
ALTER TABLE players ADD COLUMN xp_to_next INTEGER NOT NULL DEFAULT 300;

-- Add faction standings JSON blob.
ALTER TABLE players ADD COLUMN faction_standings TEXT NOT NULL DEFAULT '{}';
