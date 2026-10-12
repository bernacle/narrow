-- Notification policy v2 (BRU-389 slice A): email becomes a daily digest.
-- See docs/email-digest-policy-v2.md.
--
-- Additive only. Every existing subscriber keeps their address, its
-- confirmation and their on/off switch, and gets the defaults below: a
-- digest of at most 3 jobs at 09:00, Monday to Friday, never between 21:00
-- and 08:00, in their timezone (UTC until they set one).
--
-- `cadence` ('immediate' or 'daily') is kept as it is but no longer read:
-- both now mean the daily digest. 'immediate' used to send any new strong
-- fit at most every 4 hours; instant alerts are a separate, gated feature
-- (BRU-389 §13.1) that may read it later.

ALTER TABLE notification_settings
    -- IANA name ('America/Sao_Paulo'); NULL: not set yet, UTC is used.
    ADD COLUMN timezone       text,
    -- The local time the digest is composed.
    ADD COLUMN digest_time    time NOT NULL DEFAULT '09:00',
    -- Days a digest may be sent, a bitmask from Monday (1) to Sunday
    -- (64); 31 is Monday to Friday.
    ADD COLUMN digest_days    smallint NOT NULL DEFAULT 31
                              CHECK (digest_days BETWEEN 1 AND 127),
    -- Jobs in one digest at most.
    ADD COLUMN digest_size    smallint NOT NULL DEFAULT 3
                              CHECK (digest_size BETWEEN 1 AND 5),
    -- Nothing proactive from quiet_start to quiet_end (local; may wrap
    -- past midnight; equal values mean no quiet hours).
    ADD COLUMN quiet_start    time NOT NULL DEFAULT '21:00',
    ADD COLUMN quiet_end      time NOT NULL DEFAULT '08:00',
    -- Nothing proactive before this; missed digests are not sent later.
    ADD COLUMN paused_until   timestamptz,
    -- The local date of the last digest slot used (sent, or found
    -- nothing to send): at most one digest per local calendar day.
    ADD COLUMN last_digest_on date;

COMMENT ON COLUMN notification_settings.cadence IS
    'Legacy (BRU-295). Not read since notification policy v2: immediate and daily both mean the daily digest.';

-- A delivery not accepted by the provider by then is abandoned instead of
-- sent late (a digest is never sent inside quiet hours or on another
-- local day). NULL: only the general staleness limit applies.
ALTER TABLE notification_deliveries
    ADD COLUMN expires_at timestamptz;

-- For measuring delivered digests (BRU-389 slice I): where the item was in
-- the email (1 = first), and how fresh it was said to be.
ALTER TABLE notification_items
    ADD COLUMN position  smallint,
    ADD COLUMN freshness text
        CHECK (freshness IN ('posted', 'posted_approx', 'newly_listed', 'backfill', 'unknown',
                             'reopened'));
