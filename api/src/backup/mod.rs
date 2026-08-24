//! Scheduled, encrypted, off-box database backups.
//!
//! ## The pipeline
//!
//! ```text
//!   pg_dump  ->  gzip  ->  age (passphrase)  ->  destination
//! ```
//!
//! Compression before encryption, in that order and not the other way round:
//! ciphertext is incompressible, so gzipping afterwards would achieve nothing.
//! The dump of a text-heavy schema shrinks by roughly 10x, which is the
//! difference between an emailable attachment and a bounce.
//!
//! The artefact is `age`'s standard format (`age-encryption.org/v1`), so the
//! restore path does not depend on this program still existing:
//!
//! ```text
//!   age -d backup.sql.gz.age | gunzip | psql "$DATABASE_URL"
//! ```
//!
//! That property is the whole point. A backup that only this application can
//! read is not a backup, because the scenario it exists for is the one where
//! this application is gone.
//!
//! ## Where the passphrase lives
//!
//! On the secrets volume, never in Postgres. This is not a stylistic
//! preference: the passphrase encrypts a dump *of the database*, so a passphrase
//! stored in a table would be included in every backup it protects. Anyone
//! holding a backup file would hold its own key and the encryption would protect
//! nobody. Same reasoning as the Google service-account key in
//! `email::credentials`, and the same volume.
//!
//! ## Memory
//!
//! The dump is assembled in memory rather than streamed to a temp file. At the
//! current size (tens of KB, compressing to a few) that is not worth a spill
//! file, and holding it in memory means a failed run leaves nothing on disk to
//! forget about — a plaintext dump in /tmp is exactly the accident this feature
//! is meant to prevent. [`MAX_ARTIFACT_BYTES`] is the backstop if the database
//! ever outgrows the assumption.

pub mod destination;
pub mod drive;
pub mod schedule;

use std::io::Write;
use std::path::PathBuf;

use chrono::Utc;
use secrecy::SecretString;
use uuid::Uuid;

use crate::AppState;

/// Refuse to build an artefact larger than this rather than exhaust the box's
/// memory. A VPS that OOMs while backing up has turned a precaution into an
/// outage. If this is ever hit the fix is to stream to a temp file, not to raise
/// the number.
const MAX_ARTIFACT_BYTES: usize = 512 * 1024 * 1024;

/// Where the encryption passphrase is kept. On the same named volume as the
/// Google key, so it survives the container being recreated on deploy.
const PASSPHRASE_PATH: &str = "/var/lib/blendbar/secrets/backup-passphrase";

#[derive(Debug)]
pub enum BackupError {
    /// Missing passphrase, missing credentials, unusable schedule. Retrying
    /// changes nothing until a human fixes the configuration.
    NotConfigured(String),
    /// The destination refused it — too large, no permission, gone.
    Rejected(String),
    /// Network or transient server problem. Worth another attempt.
    Transport(String),
    Internal(String),
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackupError::NotConfigured(m) => write!(f, "not configured: {m}"),
            BackupError::Rejected(m) => write!(f, "rejected: {m}"),
            BackupError::Transport(m) => write!(f, "transport error: {m}"),
            BackupError::Internal(m) => write!(f, "{m}"),
        }
    }
}

impl From<BackupError> for crate::error::AppError {
    fn from(err: BackupError) -> Self {
        match err {
            // A misconfiguration is the operator's to fix and the message says
            // how, so it goes back to the browser intact rather than becoming an
            // opaque 500.
            BackupError::NotConfigured(m) => crate::error::AppError::BadRequest(m),
            BackupError::Rejected(m) => crate::error::AppError::BadRequest(m),
            BackupError::Transport(m) => crate::error::AppError::Unavailable(m),
            BackupError::Internal(m) => crate::error::AppError::Internal(m),
        }
    }
}

// --- Passphrase -------------------------------------------------------------

pub mod passphrase {
    use super::*;

    /// True when the environment supplies the passphrase, in which case the UI
    /// shows it as ops-managed and refuses to overwrite it — the same
    /// precedence rule as the Google key. Two sources of truth for a credential
    /// is how they drift apart.
    pub fn env_managed() -> bool {
        std::env::var("BACKUP_PASSPHRASE")
            .ok()
            .is_some_and(|s| !s.trim().is_empty())
    }

    pub fn path() -> PathBuf {
        PathBuf::from(PASSPHRASE_PATH)
    }

    pub fn is_set() -> bool {
        env_managed() || path().exists()
    }

    /// The passphrase, environment first.
    pub fn load() -> Option<SecretString> {
        if let Ok(v) = std::env::var("BACKUP_PASSPHRASE") {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return Some(SecretString::from(v));
            }
        }
        // An empty or whitespace-only file is treated as absent. Otherwise a
        // truncated write would encrypt every future backup to the empty
        // passphrase — which decrypts for anyone — while the UI happily
        // reported the passphrase as set.
        std::fs::read_to_string(path())
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(SecretString::from)
    }

    /// Write a new passphrase to the volume, 0600, via a temp file and a rename
    /// so there is no window in which a half-written file could be read.
    pub fn store(raw: &str) -> Result<(), BackupError> {
        let trimmed = raw.trim();
        // Not a strength meter — just a floor. This key protects every customer
        // record the business holds, and a four-character passphrase against an
        // offline attacker who has stolen the file is no protection at all.
        if trimmed.len() < 12 {
            return Err(BackupError::NotConfigured(
                "use at least 12 characters — this passphrase is the only thing standing \
                 between a stolen backup file and every customer record in it"
                    .into(),
            ));
        }
        if env_managed() {
            return Err(BackupError::NotConfigured(
                "the passphrase is set in the environment (BACKUP_PASSPHRASE); change it \
                 there rather than here"
                    .into(),
            ));
        }

        let target = path();
        let dir = target
            .parent()
            .ok_or_else(|| BackupError::Internal("bad passphrase path".into()))?;
        std::fs::create_dir_all(dir)
            .map_err(|e| BackupError::Internal(format!("could not create {dir:?}: {e}")))?;

        let tmp = dir.join(".backup-passphrase.tmp");
        let mut file = std::fs::File::create(&tmp)
            .map_err(|e| BackupError::Internal(format!("could not write passphrase: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| BackupError::Internal(format!("could not chmod: {e}")))?;
        }
        file.write_all(trimmed.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|e| BackupError::Internal(format!("could not write passphrase: {e}")))?;
        drop(file);
        std::fs::rename(&tmp, &target)
            .map_err(|e| BackupError::Internal(format!("could not store passphrase: {e}")))?;
        Ok(())
    }
}

// --- The artefact -----------------------------------------------------------

/// One encrypted backup, ready to send somewhere.
pub struct Artifact {
    pub filename: String,
    pub bytes: Vec<u8>,
    /// Size of the raw dump before compression and encryption. Logged and shown
    /// so a sudden change in the *real* data size is visible, which a
    /// post-compression number would hide.
    pub plain_bytes: usize,
}

/// Run `pg_dump` and return the raw SQL.
///
/// Shared with the manual download endpoint so the scheduled and on-demand
/// backups can never drift into producing different dumps — the restore
/// instructions have to be true for both.
///
/// `--no-owner --no-privileges` makes the dump portable across instances and
/// roles. No caller input reaches the command line, so there is no injection
/// surface.
pub async fn pg_dump() -> Result<Vec<u8>, BackupError> {
    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| BackupError::Internal("DATABASE_URL not set".into()))?;

    let output = tokio::process::Command::new("pg_dump")
        .args(["--no-owner", "--no-privileges"])
        .arg(&database_url)
        .output()
        .await
        .map_err(|e| BackupError::Internal(format!("could not run pg_dump: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!("pg_dump failed: {stderr}");
        // The stderr can name the real problem (disk full, auth) and an operator
        // staring at a red row needs it. It cannot contain row data — pg_dump
        // writes the dump to stdout.
        return Err(BackupError::Internal(format!(
            "pg_dump failed: {}",
            stderr.lines().last().unwrap_or("no output").trim()
        )));
    }

    if output.stdout.len() > MAX_ARTIFACT_BYTES {
        return Err(BackupError::Internal(format!(
            "the dump is {} bytes, over the {MAX_ARTIFACT_BYTES}-byte in-memory limit",
            output.stdout.len()
        )));
    }

    Ok(output.stdout)
}

/// gzip, then encrypt to a passphrase. CPU-bound, so it runs on the blocking
/// pool: doing this inline would stall every other request on the runtime for
/// the duration.
pub fn compress_and_encrypt(plain: &[u8], pass: SecretString) -> Result<Vec<u8>, BackupError> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(plain)
        .map_err(|e| BackupError::Internal(format!("compression failed: {e}")))?;
    let compressed = gz
        .finish()
        .map_err(|e| BackupError::Internal(format!("compression failed: {e}")))?;

    let encryptor = age::Encryptor::with_user_passphrase(pass);
    let mut out = Vec::with_capacity(compressed.len() + 1024);
    let mut writer = encryptor
        .wrap_output(&mut out)
        .map_err(|e| BackupError::Internal(format!("encryption failed: {e}")))?;
    writer
        .write_all(&compressed)
        .map_err(|e| BackupError::Internal(format!("encryption failed: {e}")))?;
    // Without this the file is truncated and will not decrypt — and the failure
    // would only surface on the day someone tried to restore it.
    writer
        .finish()
        .map_err(|e| BackupError::Internal(format!("encryption failed: {e}")))?;

    Ok(out)
}

/// Dump, compress, encrypt. The whole artefact, ready to upload.
pub async fn build_artifact() -> Result<Artifact, BackupError> {
    let Some(pass) = passphrase::load() else {
        return Err(BackupError::NotConfigured(
            "no backup passphrase is set — set one under Admin → Data before scheduling \
             backups. Backups are never written unencrypted."
                .into(),
        ));
    };

    let plain = pg_dump().await?;
    let plain_bytes = plain.len();

    let bytes = tokio::task::spawn_blocking(move || compress_and_encrypt(&plain, pass))
        .await
        .map_err(|e| BackupError::Internal(format!("encryption task failed: {e}")))??;

    Ok(Artifact {
        filename: format!(
            "blendbar-backup-{}.sql.gz.age",
            Utc::now().format("%Y%m%d-%H%M%S")
        ),
        bytes,
        plain_bytes,
    })
}

// --- Running one destination ------------------------------------------------

/// A destination row as the worker needs it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DestinationRow {
    pub id: Uuid,
    pub label: String,
    pub kind: String,
    pub config: serde_json::Value,
    pub schedule: String,
    pub timezone: String,
    pub retain_count: i32,
}

/// Back up to one destination and record the attempt.
///
/// Every exit path writes a terminal row: a backup system that only records its
/// successes is how a business discovers, on the worst possible day, that it
/// stopped running in March.
pub async fn run_once(
    state: &AppState,
    dest: &DestinationRow,
    artifact: &Artifact,
    trigger: &str,
) -> Result<(), BackupError> {
    let run_id: Uuid = sqlx::query_scalar(
        "insert into backup_runs (destination_id, trigger, status, filename, bytes) \
         values ($1, $2, 'running', $3, $4) returning id",
    )
    .bind(dest.id)
    .bind(trigger)
    .bind(&artifact.filename)
    .bind(artifact.bytes.len() as i64)
    .fetch_one(&state.db)
    .await
    .map_err(|e| BackupError::Internal(e.to_string()))?;

    let outcome = match destination::build(state, dest).await {
        Ok(backend) => backend.upload(artifact).await,
        Err(e) => Err(e),
    };

    match &outcome {
        Ok(remote_id) => {
            sqlx::query(
                "update backup_runs set status = 'ok', finished_at = now(), remote_id = $2 \
                 where id = $1",
            )
            .bind(run_id)
            .bind(remote_id.as_deref())
            .execute(&state.db)
            .await
            .map_err(|e| BackupError::Internal(e.to_string()))?;

            sqlx::query(
                "update backup_destinations set last_run_at = now(), last_status = 'ok', \
                 last_error = null, updated_at = now() where id = $1",
            )
            .bind(dest.id)
            .execute(&state.db)
            .await
            .map_err(|e| BackupError::Internal(e.to_string()))?;

            tracing::info!(
                destination = %dest.label,
                file = %artifact.filename,
                bytes = artifact.bytes.len(),
                plain_bytes = artifact.plain_bytes,
                "backup uploaded"
            );
        }
        Err(e) => {
            let message = e.to_string();
            sqlx::query(
                "update backup_runs set status = 'failed', finished_at = now(), error = $2 \
                 where id = $1",
            )
            .bind(run_id)
            .bind(&message)
            .execute(&state.db)
            .await
            .map_err(|e| BackupError::Internal(e.to_string()))?;

            sqlx::query(
                "update backup_destinations set last_run_at = now(), last_status = 'failed', \
                 last_error = $2, updated_at = now() where id = $1",
            )
            .bind(dest.id)
            .bind(&message)
            .execute(&state.db)
            .await
            .map_err(|e| BackupError::Internal(e.to_string()))?;

            tracing::error!(destination = %dest.label, "backup failed: {message}");
        }
    }

    outcome.map(|_| ())
}

/// Delete backups beyond `retain_count` at this destination.
///
/// Only files this scheduler uploaded and still has a `remote_id` for are ever
/// touched, so a shared Drive folder cannot lose somebody else's document to
/// retention.
///
/// Pruning failures are logged, not propagated: a successful backup that could
/// not tidy up afterwards is still a successful backup, and turning the run red
/// would hide that.
pub async fn prune(state: &AppState, dest: &DestinationRow) {
    let Ok(backend) = destination::build(state, dest).await else {
        return;
    };
    if !backend.supports_delete() {
        return;
    }

    // Successful runs with a remote id, newest first, skipping the ones to keep.
    let stale: Vec<(Uuid, String)> = match sqlx::query_as(
        "select id, remote_id from backup_runs \
         where destination_id = $1 and status = 'ok' and remote_id is not null \
         order by started_at desc offset $2",
    )
    .bind(dest.id)
    .bind(dest.retain_count as i64)
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(destination = %dest.label, "could not list old backups: {e}");
            return;
        }
    };

    for (run_id, remote_id) in stale {
        match backend.delete(&remote_id).await {
            Ok(()) => {
                // Clearing remote_id (rather than deleting the row) keeps the
                // history of what ran while marking the file as gone, so the
                // next prune does not try again.
                let _ = sqlx::query("update backup_runs set remote_id = null where id = $1")
                    .bind(run_id)
                    .execute(&state.db)
                    .await;
                tracing::info!(destination = %dest.label, %remote_id, "pruned old backup");
            }
            Err(e) => {
                tracing::warn!(
                    destination = %dest.label, %remote_id,
                    "could not prune old backup: {e}"
                );
            }
        }
    }
}

/// Advance `next_run_at` from the schedule. Disables the destination if the
/// expression can never fire again, rather than leaving a row that looks
/// scheduled and silently never runs.
pub async fn reschedule(state: &AppState, dest: &DestinationRow) {
    match schedule::next_after(&dest.schedule, &dest.timezone, Utc::now()) {
        Ok(next) => {
            let _ = sqlx::query(
                "update backup_destinations set next_run_at = $2, updated_at = now() where id = $1",
            )
            .bind(dest.id)
            .bind(next)
            .execute(&state.db)
            .await;
        }
        Err(e) => {
            tracing::error!(destination = %dest.label, "unusable schedule, disabling: {e}");
            let _ = sqlx::query(
                "update backup_destinations set enabled = false, next_run_at = null, \
                 last_status = 'failed', last_error = $2, updated_at = now() where id = $1",
            )
            .bind(dest.id)
            .bind(format!("schedule is unusable: {e}"))
            .execute(&state.db)
            .await;
        }
    }
}

// --- The worker -------------------------------------------------------------

/// How often to look for due backups. A minute is the resolution of a cron
/// expression, so polling faster cannot fire anything sooner.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// Run forever, backing up whatever is due. Spawned once at startup, alongside
/// the sync worker and on the same single-process assumption.
pub async fn run_worker(state: AppState) {
    tracing::info!(
        "backup worker started (passphrase {})",
        if passphrase::is_set() {
            "configured"
        } else {
            "NOT SET — scheduled backups will not run"
        }
    );

    // Anything left without a next run — a row written before this worker
    // existed, or by a failed edit — gets one now. Scheduled, not run: booting
    // should never be a reason to fire a backup.
    if let Ok(rows) = sqlx::query_as::<_, DestinationRow>(
        "select id, label, kind, config, schedule, timezone, retain_count \
         from backup_destinations where enabled and next_run_at is null",
    )
    .fetch_all(&state.db)
    .await
    {
        for dest in rows {
            reschedule(&state, &dest).await;
        }
    }

    loop {
        if let Err(e) = poll(&state).await {
            tracing::error!("backup worker poll failed: {e}");
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn poll(state: &AppState) -> Result<(), sqlx::Error> {
    let due: Vec<DestinationRow> = sqlx::query_as(
        "select id, label, kind, config, schedule, timezone, retain_count \
         from backup_destinations \
         where enabled and next_run_at is not null and next_run_at <= now() \
         order by next_run_at",
    )
    .fetch_all(&state.db)
    .await?;

    if due.is_empty() {
        return Ok(());
    }

    // One dump for the whole tick. Two destinations due at the same minute — the
    // common "Drive nightly plus email nightly" setup — should not mean two
    // pg_dumps and two encryptions, and sharing one artefact also means both
    // copies are byte-identical.
    let artifact = match build_artifact().await {
        Ok(a) => a,
        Err(e) => {
            // The dump failed, so no destination can succeed. Record it against
            // each due destination and move their schedules on, rather than
            // retrying every 60 seconds until someone notices.
            let message = e.to_string();
            tracing::error!("backup: could not build the artefact: {message}");
            for dest in &due {
                let _ = sqlx::query(
                    "insert into backup_runs (destination_id, trigger, status, finished_at, error) \
                     values ($1, 'scheduled', 'failed', now(), $2)",
                )
                .bind(dest.id)
                .bind(&message)
                .execute(&state.db)
                .await;
                let _ = sqlx::query(
                    "update backup_destinations set last_run_at = now(), last_status = 'failed', \
                     last_error = $2, updated_at = now() where id = $1",
                )
                .bind(dest.id)
                .bind(&message)
                .execute(&state.db)
                .await;
                reschedule(state, dest).await;
            }
            return Ok(());
        }
    };

    for dest in &due {
        // Sequential on purpose: pg_dump has already run, but uploads on a small
        // VPS are better queued than raced, and the ordering makes the log
        // readable.
        let result = run_once(state, dest, &artifact, "scheduled").await;
        if result.is_ok() {
            prune(state, dest).await;
        }
        // Always advance, success or failure. A destination that fails every
        // time must not become a hot loop dumping the database every 60 seconds.
        reschedule(state, dest).await;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::SecretString;
    use std::io::Read;

    #[test]
    fn the_artifact_round_trips_through_gzip_and_age() {
        let plain = b"create table t (id int);\ninsert into t values (1);\n".repeat(20);
        let pass = SecretString::from("a-sufficiently-long-passphrase".to_string());

        let sealed = compress_and_encrypt(&plain, pass.clone()).unwrap();

        // Standard age format: the header is what the `age` CLI looks for, and
        // it is the reason a restore does not need this program.
        assert!(sealed.starts_with(b"age-encryption.org/v1"));

        let decryptor = age::Decryptor::new(&sealed[..]).unwrap();
        assert!(decryptor.is_scrypt());
        let identity = age::scrypt::Identity::new(pass);
        let mut reader = decryptor
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .unwrap();
        let mut gzipped = Vec::new();
        reader.read_to_end(&mut gzipped).unwrap();

        let mut decoder = flate2::read::GzDecoder::new(&gzipped[..]);
        let mut back = Vec::new();
        decoder.read_to_end(&mut back).unwrap();

        assert_eq!(back, plain, "what comes out must be exactly what went in");
    }

    #[test]
    fn the_wrong_passphrase_does_not_decrypt() {
        let sealed = compress_and_encrypt(
            b"secret",
            SecretString::from("the-right-passphrase-here".to_string()),
        )
        .unwrap();
        let decryptor = age::Decryptor::new(&sealed[..]).unwrap();
        let wrong = age::scrypt::Identity::new(SecretString::from("not-that-one-at-all".to_string()));
        assert!(decryptor
            .decrypt(std::iter::once(&wrong as &dyn age::Identity))
            .is_err());
    }

    #[test]
    fn compression_actually_helps_on_dump_shaped_data() {
        // The email destination lives or dies on this: a dump that compresses
        // is an attachment, one that does not is a bounce.
        let plain = b"insert into orders values (1, 'a', 'b', 'c');\n".repeat(500);
        let sealed = compress_and_encrypt(
            &plain,
            SecretString::from("a-sufficiently-long-passphrase".to_string()),
        )
        .unwrap();
        assert!(
            sealed.len() < plain.len() / 5,
            "expected real compression, got {} from {}",
            sealed.len(),
            plain.len()
        );
    }

    #[test]
    fn a_short_passphrase_is_refused() {
        assert!(passphrase::store("short").is_err());
    }

    /// Writes a real artefact, built by the real pipeline, so it can be checked
    /// against the *stock* `age` CLI and reloaded into a scratch Postgres.
    ///
    /// Ignored by default: it needs a live database and it writes a file. It
    /// exists because the round-trip test above proves only that this code can
    /// read what this code wrote — precisely the assurance that is worthless on
    /// the day you actually need a restore. The claim worth testing is that a
    /// stranger with `age`, `gunzip` and `psql` can get the data back.
    ///
    /// ```text
    /// BACKUP_PASSPHRASE=… BACKUP_VERIFY_OUT=/out/backup.sql.gz.age \
    ///   cargo test -- --ignored writes_a_real_artifact --nocapture
    /// ```
    #[tokio::test]
    #[ignore]
    async fn writes_a_real_artifact_for_external_verification() {
        let out =
            std::env::var("BACKUP_VERIFY_OUT").expect("set BACKUP_VERIFY_OUT to the path to write");
        let artifact = build_artifact().await.expect("could not build the artefact");
        std::fs::write(&out, &artifact.bytes).expect("could not write the artefact");
        println!(
            "wrote {out} — {} bytes encrypted, {} raw, filename {}",
            artifact.bytes.len(),
            artifact.plain_bytes,
            artifact.filename
        );
    }
}
