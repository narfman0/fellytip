CREATE TABLE IF NOT EXISTS player_faction_standing (
    player_id  TEXT    NOT NULL,
    faction_id TEXT    NOT NULL,
    score      INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (player_id, faction_id)
);
