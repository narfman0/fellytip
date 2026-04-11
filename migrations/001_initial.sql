-- Fellytip initial schema

CREATE TABLE IF NOT EXISTS players (
    id           TEXT    PRIMARY KEY,
    name         TEXT    NOT NULL,
    faction_id   TEXT,
    class        TEXT    NOT NULL,
    level        INTEGER NOT NULL DEFAULT 1,
    health_current INTEGER NOT NULL,
    health_max     INTEGER NOT NULL,
    pos_x        REAL    NOT NULL DEFAULT 0,
    pos_y        REAL    NOT NULL DEFAULT 0,
    last_seen    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS story_events (
    id           TEXT    PRIMARY KEY,
    tick         INTEGER NOT NULL,
    world_day    INTEGER NOT NULL,
    kind         TEXT    NOT NULL,   -- JSON StoryEventKind
    participants TEXT    NOT NULL,   -- JSON array of UUIDs
    loc_x        INTEGER,
    loc_y        INTEGER,
    lore_tags    TEXT    NOT NULL    -- JSON array of strings
);

CREATE TABLE IF NOT EXISTS factions (
    id        TEXT PRIMARY KEY,
    name      TEXT NOT NULL,
    resources TEXT NOT NULL,  -- JSON FactionResources
    territory TEXT NOT NULL,  -- JSON [IVec2]
    goals     TEXT NOT NULL   -- JSON [FactionGoal]
);

CREATE TABLE IF NOT EXISTS ecology_state (
    species_id TEXT    NOT NULL,
    region_id  TEXT    NOT NULL,
    count      INTEGER NOT NULL,
    PRIMARY KEY (species_id, region_id)
);

CREATE TABLE IF NOT EXISTS world_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
