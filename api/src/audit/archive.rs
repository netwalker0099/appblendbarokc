//! Retention for the audit log: archive, deliver, then prune.
//!
//! The ordering is the whole design. Entries are only removed from the table
//! after they have been serialised, checksummed, encrypted and accepted by at
//! least one backup destination. If nothing can be delivered to — no
//! destinations, Google down, mailer unconfigured — nothing is pruned and the
//! table simply keeps growing. A log that grows too big is an operational
//! nuisance; a log that deleted itself because the upload failed is the thing
//! you needed and cannot get back.
//!
//! ## What the archive file is
//!
//! JSON Lines, one entry per line, in id order, with a header line carrying the
//! chain anchors. Plain text before compression, so anyone with `age`, `gunzip`
//! and a text editor can read it — the same reasoning as the database backups.
//! A recovered archive that needs this program to interpret it is not much of a
//! recovery.
//!
//! ```text
//! age -d segment-….jsonl.gz.age | gunzip | head -1     # the header
//! ```
//!
//! ## Why archives are not `backup_runs`
//!
//! Database backups rotate: keep the last 30, delete the rest. An archive
//! segment holds the only remaining copy of that slice of history, so rotating
//! one away would destroy it. Archives are recorded in `audit_archive_segments`,
//! are never pruned, and are uploaded outside the `backup_runs` bookkeeping that
//! retention walks.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::backup::{self, destination, Artifact, BackupError, DestinationRow};
use crate::AppState;

/// One archived entry, as it appears in the file.
#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct ArchivedEntry {
    pub id: i64,
    pub at: DateTime<Utc>,
    pub actor_email: String,
    pub actor_role: String,
    pub method: String,
    pub path: String,
    pub status: i32,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub summary: Option<String>,
    pub detail: Option<Value>,
    pub prev_hash: String,
    pub entry_hash: String,
}

pub struct Outcome {
    pub segment_id: i64,
    pub entry_count: i64,
    pub filename: String,
    pub bytes: usize,
    pub delivered_to: Vec<String>,
}

/// Serialise entries to the archive format: a header line, then one line each.
///
/// Deterministic — the same entries always produce the same bytes, which is what
/// makes `content_sha256` meaningful as a check that a recovered file is the one
/// that was taken.
pub fn serialise(entries: &[ArchivedEntry]) -> Result<String, BackupError> {
    let first = entries
        .first()
        .ok_or_else(|| BackupError::Internal("nothing to serialise".into()))?;
    let last = entries.last().expect("non-empty");

    let header = json!({
        "type": "blendbar-audit-archive",
        "version": 1,
        "from_id": first.id,
        "to_id": last.id,
        "entry_count": entries.len(),
        "from_at": first.at,
        "to_at": last.at,
        // The chain either side of this segment, so the file can be checked
        // against the segment record — and against the surviving log — without
        // consulting anything else.
        "anchor_prev_hash": first.prev_hash,
        "last_entry_hash": last.entry_hash,
        "note": "One JSON object per line after this header. Entries are hash-chained: \
                 each entry_hash covers prev_hash plus that entry's contents.",
    });

    let mut out = String::with_capacity(entries.len() * 256);
    out.push_str(
        &serde_json::to_string(&header)
            .map_err(|e| BackupError::Internal(format!("could not write header: {e}")))?,
    );
    out.push('\n');
    for entry in entries {
        out.push_str(
            &serde_json::to_string(entry)
                .map_err(|e| BackupError::Internal(format!("could not write entry: {e}")))?,
        );
        out.push('\n');
    }
    Ok(out)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// How many days of audit history to keep. 0 means keep everything.
pub async fn retention_days(db: &sqlx::PgPool) -> i32 {
    sqlx::query_scalar::<_, i32>("select audit_retention_days from settings where id = true")
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .unwrap_or(0)
}

/// Archive everything older than the retention window.
///
/// `Ok(None)` means there was nothing to do — retention is off, or no entry is
/// old enough. That is the overwhelmingly common case and is not worth logging.
pub async fn run(state: &AppState) -> Result<Option<Outcome>, BackupError> {
    let days = retention_days(&state.db).await;
    if days <= 0 {
        return Ok(None);
    }
    let cutoff = Utc::now() - Duration::days(days as i64);

    // A contiguous prefix, not merely "everything older than the cutoff".
    //
    // The chain is an ordering, so only a run of entries from the very start can
    // be archived — take a slice out of the middle and the surviving rows either
    // side no longer link, which is indistinguishable from tampering. `at` and
    // `id` normally advance together, so the two definitions usually pick the
    // same rows; they diverge the moment anything is inserted with a backdated
    // timestamp, and then "where at < cutoff" quietly selects a hole.
    //
    // So: everything strictly before the first entry that is too new to archive.
    let entries: Vec<ArchivedEntry> = sqlx::query_as(
        "select id, at, actor_email, actor_role, method, path, status, ip, user_agent, \
                summary, detail, prev_hash, entry_hash \
           from admin_audit_log \
          where id < coalesce( \
                    (select min(id) from admin_audit_log where at >= $1), \
                    (select max(id) + 1 from admin_audit_log)) \
          order by id",
    )
    .bind(cutoff)
    .fetch_all(&state.db)
    .await
    .map_err(|e| BackupError::Internal(e.to_string()))?;

    if entries.is_empty() {
        return Ok(None);
    }

    // Refuse to archive a span that is not contiguous with what came before it.
    // If the chain is already broken, archiving would bake the break into a
    // permanent record and make it look sanctioned.
    let expected_anchor: String =
        sqlx::query_scalar("select coalesce(max(last_entry_hash), repeat('0', 64)) \
                            from audit_archive_segments \
                            where from_id = (select max(from_id) from audit_archive_segments)")
            .fetch_optional(&state.db)
            .await
            .map_err(|e| BackupError::Internal(e.to_string()))?
            .unwrap_or_else(|| "0".repeat(64));

    if entries[0].prev_hash != expected_anchor {
        return Err(BackupError::Blocked(format!(
            "refusing to archive: entry {} does not continue the chain from the last \
             archive, so archiving it would bake an existing break into a permanent \
             record and make it look sanctioned. Run Verify first and resolve the \
             break. Nothing was pruned.",
            entries[0].id
        )));
    }

    let first_id = entries[0].id;
    let last_id = entries[entries.len() - 1].id;
    let from_at = entries[0].at;
    let to_at = entries[entries.len() - 1].at;
    let anchor_prev_hash = entries[0].prev_hash.clone();
    let last_entry_hash = entries[entries.len() - 1].entry_hash.clone();

    let plain = serialise(&entries)?;
    let content_sha256 = sha256_hex(plain.as_bytes());

    // Same encryption as the database backups: an archive is every bit as
    // sensitive, since it is a record of who did what and to whom.
    let Some(pass) = backup::passphrase::load() else {
        return Err(BackupError::NotConfigured(
            "no backup passphrase is set, so audit archives cannot be encrypted — \
             nothing was pruned."
                .into(),
        ));
    };

    let filename = format!(
        "blendbar-audit-{}-{:06}-{:06}.jsonl.gz.age",
        Utc::now().format("%Y%m%d-%H%M%S"),
        first_id,
        last_id
    );

    let plain_len = plain.len();
    let bytes = tokio::task::spawn_blocking(move || {
        backup::compress_and_encrypt(plain.as_bytes(), pass)
    })
    .await
    .map_err(|e| BackupError::Internal(format!("encryption task failed: {e}")))??;

    let artifact = Artifact {
        filename: filename.clone(),
        bytes,
        plain_bytes: plain_len,
    };

    // Deliver to every enabled destination. One success is enough to prune, but
    // all are attempted — more copies of an irreplaceable file is strictly
    // better.
    let destinations: Vec<DestinationRow> = sqlx::query_as(
        "select id, label, kind, config, schedule, timezone, retain_count \
           from backup_destinations where enabled",
    )
    .fetch_all(&state.db)
    .await
    .map_err(|e| BackupError::Internal(e.to_string()))?;

    if destinations.is_empty() {
        return Err(BackupError::NotConfigured(
            "audit retention is on but there is no enabled backup destination to send \
             the archive to. Nothing was pruned — old entries are kept rather than \
             deleted with nowhere to go."
                .into(),
        ));
    }

    let mut delivered = Vec::new();
    let mut failures = Vec::new();
    for dest in &destinations {
        match destination::build(state, dest).await {
            Ok(backend) => match backend.upload(&artifact).await {
                Ok(remote_id) => {
                    delivered.push(json!({
                        "destination": dest.label,
                        "kind": dest.kind,
                        "remote_id": remote_id,
                    }));
                    tracing::info!(
                        destination = %dest.label, file = %artifact.filename,
                        "audit archive delivered"
                    );
                }
                Err(e) => failures.push(format!("{}: {e}", dest.label)),
            },
            Err(e) => failures.push(format!("{}: {e}", dest.label)),
        }
    }

    if delivered.is_empty() {
        // The safe failure. Entries stay put and the table grows.
        return Err(BackupError::Transport(format!(
            "audit archive could not be delivered anywhere, so nothing was pruned: {}",
            failures.join("; ")
        )));
    }
    if !failures.is_empty() {
        tracing::warn!("audit archive: some destinations failed: {}", failures.join("; "));
    }

    // Record the segment and prune, in one transaction. If the delete fails the
    // segment record must not survive claiming entries that are still present —
    // that would make the verifier expect a gap that is not there.
    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|e| BackupError::Internal(e.to_string()))?;

    let segment_id: i64 = sqlx::query_scalar(
        "insert into audit_archive_segments \
           (from_id, to_id, entry_count, from_at, to_at, anchor_prev_hash, last_entry_hash, \
            content_sha256, filename, destinations) \
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) returning id",
    )
    .bind(first_id)
    .bind(last_id)
    .bind(entries.len() as i64)
    .bind(from_at)
    .bind(to_at)
    .bind(&anchor_prev_hash)
    .bind(&last_entry_hash)
    .bind(&content_sha256)
    .bind(&filename)
    .bind(Value::Array(delivered.clone()))
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| BackupError::Internal(e.to_string()))?;

    // The one sanctioned delete. Transaction-scoped, so it cannot leak into any
    // other statement on this connection.
    sqlx::query("select set_config('blendbar.audit_archiving', 'on', true)")
        .execute(&mut *tx)
        .await
        .map_err(|e| BackupError::Internal(e.to_string()))?;

    let deleted = sqlx::query("delete from admin_audit_log where id <= $1")
        .bind(last_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| BackupError::Internal(e.to_string()))?
        .rows_affected();

    if deleted != entries.len() as u64 {
        // Something was written between the select and the delete. Roll back
        // rather than record a segment that misdescribes what was removed.
        return Err(BackupError::Internal(format!(
            "expected to prune {} entries but matched {deleted}; nothing was changed",
            entries.len()
        )));
    }

    tx.commit()
        .await
        .map_err(|e| BackupError::Internal(e.to_string()))?;

    let delivered_to: Vec<String> = delivered
        .iter()
        .filter_map(|d| d.get("destination").and_then(|v| v.as_str()).map(String::from))
        .collect();

    tracing::info!(
        segment = segment_id, entries = entries.len(), file = %filename,
        "audit archive complete: {} entries pruned after delivery to {}",
        entries.len(),
        delivered_to.join(", ")
    );

    Ok(Some(Outcome {
        segment_id,
        entry_count: entries.len() as i64,
        filename,
        bytes: artifact.bytes.len(),
        delivered_to,
    }))
}

/// Read a decrypted archive file back into the table.
///
/// The counterpart to [`run`], and the reason the archive format is plain JSON
/// Lines. An archive nobody can restore is not an archive, it is a deletion with
/// extra steps.
///
/// Entries are re-inserted with their **original** ids and hashes, so the
/// restored rows verify against the same chain they always did. That needs the
/// insert trigger held off — it would otherwise recompute the hashes — which is
/// what `blendbar.audit_restoring` does. It is transaction-scoped and set
/// nowhere else.
///
/// Refuses to touch anything that is already present: importing on top of live
/// entries could only produce duplicates or a contradiction.
pub async fn import(db: &sqlx::PgPool, contents: &str) -> Result<i64, BackupError> {
    let mut lines = contents.lines().filter(|l| !l.trim().is_empty());

    let first_line = lines
        .next()
        .ok_or_else(|| BackupError::BadArchive("the file is empty".into()))?;

    // A file whose first line is not JSON at all is almost always the database
    // backup handed over by mistake — both come out of the same `age -d` step,
    // minutes apart, during a recovery. Name that rather than reporting a JSON
    // parse error, which tells someone under pressure nothing useful.
    let header: Value = serde_json::from_str(first_line).map_err(|_| {
        BackupError::BadArchive(
            "this is not a Blend Bar audit archive — its first line is not the expected \
             header. If it starts with \"-- PostgreSQL database dump\" it is the database \
             backup; restore that with psql instead."
                .into(),
        )
    })?;

    if header.get("type").and_then(|v| v.as_str()) != Some("blendbar-audit-archive") {
        return Err(BackupError::BadArchive(
            "this is JSON, but not a Blend Bar audit archive — check you decrypted the \
             right file"
                .into(),
        ));
    }

    let entries: Vec<ArchivedEntry> = lines
        .map(|l| {
            serde_json::from_str::<ArchivedEntry>(l)
                .map_err(|e| BackupError::BadArchive(format!("unreadable entry: {e}")))
        })
        .collect::<Result<_, _>>()?;

    if entries.is_empty() {
        return Err(BackupError::BadArchive("the archive has no entries".into()));
    }

    // The checksum in the segment record is over the whole file, so recomputing
    // it here lets an operator confirm the recovered file is the one that was
    // taken — reported rather than enforced, since a segment record may not
    // exist on the machine being restored onto.
    tracing::info!(
        "importing audit archive: {} entries, ids {}..{}, sha256 {}",
        entries.len(),
        entries[0].id,
        entries[entries.len() - 1].id,
        sha256_hex(contents.as_bytes())
    );

    let mut tx = db.begin()
        .await
        .map_err(|e| BackupError::Internal(e.to_string()))?;

    let clash: Option<i64> = sqlx::query_scalar(
        "select id from admin_audit_log where id between $1 and $2 limit 1",
    )
    .bind(entries[0].id)
    .bind(entries[entries.len() - 1].id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| BackupError::Internal(e.to_string()))?;

    if let Some(id) = clash {
        return Err(BackupError::BadArchive(format!(
            "entry {id} is already in the table — this archive covers entries that have \
             not been pruned. Nothing was imported."
        )));
    }

    sqlx::query("select set_config('blendbar.audit_restoring', 'on', true)")
        .execute(&mut *tx)
        .await
        .map_err(|e| BackupError::Internal(e.to_string()))?;

    for e in &entries {
        sqlx::query(
            "insert into admin_audit_log \
               (id, at, actor_email, actor_role, method, path, status, ip, user_agent, \
                summary, detail, prev_hash, entry_hash) \
             values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
        )
        .bind(e.id)
        .bind(e.at)
        .bind(&e.actor_email)
        .bind(&e.actor_role)
        .bind(&e.method)
        .bind(&e.path)
        .bind(e.status)
        .bind(e.ip.as_deref())
        .bind(e.user_agent.as_deref())
        .bind(e.summary.as_deref())
        .bind(e.detail.as_ref())
        .bind(&e.prev_hash)
        .bind(&e.entry_hash)
        .execute(&mut *tx)
        .await
        .map_err(|err| BackupError::Internal(format!("could not import entry {}: {err}", e.id)))?;
    }

    tx.commit()
        .await
        .map_err(|e| BackupError::Internal(e.to_string()))?;

    Ok(entries.len() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn entry(id: i64, prev: &str, own: &str) -> ArchivedEntry {
        ArchivedEntry {
            id,
            at: Utc.with_ymd_and_hms(2026, 8, 24, 12, 0, 0).unwrap(),
            actor_email: "a@b.com".into(),
            actor_role: "admin".into(),
            method: "POST".into(),
            path: "/api/settings".into(),
            status: 200,
            ip: Some("10.0.0.1".into()),
            user_agent: None,
            summary: Some("changed settings".into()),
            detail: Some(json!({ "x": 1 })),
            prev_hash: prev.into(),
            entry_hash: own.into(),
        }
    }

    #[test]
    fn the_archive_is_a_header_line_plus_one_line_per_entry() {
        let out = serialise(&[entry(1, "aa", "bb"), entry(2, "bb", "cc")]).unwrap();
        let lines: Vec<&str> = out.trim_end().split('\n').collect();
        assert_eq!(lines.len(), 3, "header + 2 entries");

        let header: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(header["type"], "blendbar-audit-archive");
        assert_eq!(header["from_id"], 1);
        assert_eq!(header["to_id"], 2);
        assert_eq!(header["entry_count"], 2);
        // The anchors are what let a pruned log still be verified.
        assert_eq!(header["anchor_prev_hash"], "aa");
        assert_eq!(header["last_entry_hash"], "cc");

        // Every entry parses on its own, so a truncated file still yields
        // everything up to the truncation.
        let first: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(first["id"], 1);
        assert_eq!(first["entry_hash"], "bb");
    }

    #[test]
    fn serialisation_is_deterministic() {
        // content_sha256 is only meaningful if the same entries always produce
        // the same bytes.
        let entries = vec![entry(1, "aa", "bb"), entry(2, "bb", "cc")];
        assert_eq!(serialise(&entries).unwrap(), serialise(&entries).unwrap());
    }

    #[test]
    fn the_checksum_changes_when_anything_does() {
        let a = serialise(&[entry(1, "aa", "bb")]).unwrap();
        let b = serialise(&[entry(1, "aa", "bZ")]).unwrap();
        assert_ne!(sha256_hex(a.as_bytes()), sha256_hex(b.as_bytes()));
    }

    #[test]
    fn an_empty_archive_is_an_error_not_an_empty_file() {
        // Writing a zero-entry segment would record a gap that does not exist
        // and leave the verifier expecting one.
        assert!(serialise(&[]).is_err());
    }

    /// A pool that is never actually connected to. The header checks all happen
    /// before any database access, which is the property being pinned: a wrong
    /// file is rejected without opening a transaction.
    fn unusable_pool() -> sqlx::PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgres://nobody@127.0.0.1:1/none")
            .unwrap()
    }

    #[tokio::test]
    async fn the_database_backup_fed_in_by_mistake_is_named() {
        // The likeliest error during a real recovery: both files come out of the
        // same `age -d` step, minutes apart. A JSON parse error would tell
        // someone under pressure nothing.
        let err = import(&unusable_pool(), "--\n-- PostgreSQL database dump\n--\n")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a Blend Bar audit archive"), "got: {err}");
        assert!(err.contains("psql"), "should point at the right tool: {err}");
    }

    #[tokio::test]
    async fn json_that_is_not_ours_is_refused() {
        let err = import(&unusable_pool(), "{\"type\":\"something-else\"}\n")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a Blend Bar audit archive"), "got: {err}");
    }

    #[tokio::test]
    async fn an_empty_file_is_refused_before_touching_the_database() {
        let err = import(&unusable_pool(), "   \n").await.unwrap_err().to_string();
        assert!(err.contains("empty"), "got: {err}");
    }

    #[tokio::test]
    async fn a_header_with_no_entries_is_refused() {
        // A truncated recovery — the header survived and the body did not.
        // Importing zero entries would silently look like success.
        let err = import(&unusable_pool(), "{\"type\":\"blendbar-audit-archive\"}\n")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no entries"), "got: {err}");
    }

    /// Export the live audit log through the real serialiser, so the file can be
    /// imported back by the real CLI and the chain re-verified afterwards.
    ///
    /// Ignored by default: it needs a database and writes a file. It exists
    /// because a round trip inside one process proves only that this code agrees
    /// with itself — and "the archive can be restored" is a claim about a
    /// different process, on a different day, probably on a different machine.
    ///
    /// ```text
    /// AUDIT_EXPORT_OUT=/out/segment.jsonl \
    ///   cargo test -- --ignored exports_the_live_audit_log --nocapture
    /// ```
    #[tokio::test]
    #[ignore]
    async fn exports_the_live_audit_log_for_external_round_trip() {
        let out = std::env::var("AUDIT_EXPORT_OUT").expect("set AUDIT_EXPORT_OUT");
        let url = std::env::var("DATABASE_URL").expect("set DATABASE_URL");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .expect("could not connect");

        let entries: Vec<ArchivedEntry> = sqlx::query_as(
            "select id, at, actor_email, actor_role, method, path, status, ip, user_agent, \
                    summary, detail, prev_hash, entry_hash from admin_audit_log order by id",
        )
        .fetch_all(&pool)
        .await
        .expect("could not read the audit log");

        let text = serialise(&entries).expect("could not serialise");
        std::fs::write(&out, &text).expect("could not write");
        println!(
            "wrote {out} — {} entries, sha256 {}",
            entries.len(),
            sha256_hex(text.as_bytes())
        );
    }
}
