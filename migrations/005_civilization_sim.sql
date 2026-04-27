-- Civilization simulation depth: settlement economy, history, faction relations.
-- Issues #107–#111.

-- Settlement economy ledger: one row per settlement, updated each world-sim tick.
CREATE TABLE IF NOT EXISTS settlement_economy (
    settlement_id TEXT    PRIMARY KEY,
    faction_id    TEXT    NOT NULL,
    food_supply   REAL    NOT NULL DEFAULT 0,
    gold          REAL    NOT NULL DEFAULT 0,
    trade_income  REAL    NOT NULL DEFAULT 0,
    adult_count   INTEGER NOT NULL DEFAULT 0,
    updated_tick  INTEGER NOT NULL DEFAULT 0
);

-- Settlement lifecycle history (issue #111).
CREATE TABLE IF NOT EXISTS settlement_history (
    settlement_id   TEXT    NOT NULL,
    name            TEXT    NOT NULL,
    faction_id      TEXT    NOT NULL,
    founded_tick    INTEGER NOT NULL,
    death_tick      INTEGER,        -- NULL while still alive
    cause           TEXT,           -- "starvation" | "warfare" | NULL
    PRIMARY KEY (settlement_id)
);

-- Faction-to-faction relation scores (issue #110).
CREATE TABLE IF NOT EXISTS faction_relations (
    faction_a   TEXT    NOT NULL,
    faction_b   TEXT    NOT NULL,
    score       INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (faction_a, faction_b)
);
