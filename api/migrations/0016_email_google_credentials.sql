-- Let an admin connect Google from the browser instead of editing .env on the box.
--
-- Only the *non-secret* half lives here: which mailbox the app sends as. The
-- service-account private key is deliberately NOT a database column.
--
-- The reason is the backup button. `GET /api/admin/backup` hands an admin a full
-- pg_dump of this database; a private key stored in a table would ride along in
-- every one of those files, and in any replica or dump made later. Keeping it on
-- disk means the credential and the data have separate blast radii. It is written
-- to a mounted volume at /var/lib/blendbar/secrets, 0600, and never read back out
-- to a browser.

alter table email_settings
    add column google_impersonate text;
