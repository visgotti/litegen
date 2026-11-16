-- The last 10-minute mark of a generation's age (0 from its creation, 1 from
-- ten minutes on, and so on) at which the poller warned that it skips the row
-- for want of a key of the vendor account its job ran on. NULL means it never
-- warned. The poller sets it with an UPDATE guarded on a lower mark, which
-- changes the row once per mark: each mark warns once, whichever replica polls
-- the row and however its polls fall, and a restart repeats none.
ALTER TABLE generations ADD COLUMN poll_skip_warned_mark INTEGER NULL;
