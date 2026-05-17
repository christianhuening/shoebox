-- Per-(user, variant) star rating, flag, and color label.

CREATE TABLE variant_user_state (
    variant_id  TEXT NOT NULL REFERENCES variants(id),
    user_id     TEXT NOT NULL REFERENCES users(id),
    rating      INTEGER,
    flag        TEXT,
    color_label TEXT,
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (variant_id, user_id)
);

CREATE INDEX variant_user_state_user_idx ON variant_user_state(user_id);
