//! Restoring the database from an uploaded backup file.
//!
//! This is the most destructive thing the application can do to itself. The
//! design is therefore mostly about the guardrails, not the restore.
//!
//! ## Why an upload endpoint is not a remote-code-execution hole
//!
//! Restoring means handing a file to `psql`, and a `.sql` file can contain
//! `COPY … FROM PROGRAM 'sh -c …'`, which runs commands as the database user. An
//! endpoint that ran any uploaded SQL would turn one stolen admin session into
//! code execution on the database container.
//!
//! What prevents that here is that the upload must **decrypt with the backup
//! passphrase** before a single byte reaches `psql`. That passphrase is not in
//! the database, is never returned by any endpoint, and is not derivable from an
//! admin session — so an attacker with a session still cannot produce a file
//! this will accept. It is not a perfect boundary (someone holding the
//! passphrase could craft a malicious dump), but it is the same secret that
//! already protects every backup, and it reduces the endpoint from "run my SQL"
//! to "run SQL somebody with the passphrase produced".
//!
//! ## The staging
//!
//! 1. **Decrypt and decompress.** Wrong file, wrong passphrase, or plaintext —
//!    all refused here, before anything else happens.
//! 2. **Sanity-check** that it looks like a dump of *this* application.
//! 3. **Trial restore into a scratch database.** Proves the file actually loads,
//!    and produces the table and row counts the operator sees before deciding.
//!    Live data is untouched at this point — this is the whole of a dry run.
//! 4. **Safety copy** of the current database, encrypted, kept on disk. The
//!    residual risk after all the above is not "the restore fails", it is "the
//!    restore succeeds and it was the wrong file". This is the way back.
//! 5. **Live restore in one transaction.** `--single-transaction` with
//!    `ON_ERROR_STOP=1`: if any statement fails, the whole thing rolls back and
//!    the original data is still there.
//! 6. **Re-run migrations**, because a backup taken before the last deploy has
//!    an older schema, and an app running new code against an old schema is
//!    broken in ways that are tedious to diagnose.
//!
//! Steps 1–3 run with or without confirmation. Steps 4–6 need it.

use std::process::Stdio;

use chrono::Utc;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::{compress_and_encrypt, passphrase, pg_dump, BackupError};

/// The phrase an operator has to type. Deliberately not "yes" or "confirm": it
/// names the consequence, so it cannot be agreed to by reflex.
pub const CONFIRM_PHRASE: &str = "REPLACE ALL DATA";

/// Where safety copies go before a restore overwrites anything. A named volume,
/// so it survives the container being recreated on deploy.
const SAFETY_DIR: &str = "/var/lib/blendbar/safety";

/// How many safety copies to keep. These exist for "I just restored the wrong
/// file"; that is realised within minutes, not months.
const KEEP_SAFETY_COPIES: usize = 5;

/// The scratch database the trial restore loads into.
const TRIAL_DB: &str = "blendbar_restore_trial";

#[derive(Debug, serde::Serialize)]
pub struct Report {
    /// The Postgres version the dump was taken from.
    pub source_version: Option<String>,
    pub tables: i64,
    pub rows: i64,
    /// Per-table counts, so "this is the wrong file" is obvious at a glance.
    pub counts: Vec<TableCount>,
    pub sql_bytes: usize,
}

#[derive(Debug, serde::Serialize)]
pub struct TableCount {
    pub table: String,
    pub rows: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct Outcome {
    pub restored: bool,
    pub report: Report,
    pub safety_copy: Option<String>,
    pub rows_before: i64,
    pub rows_after: i64,
}

/// Refuse anything that is not recognisably a dump of this application.
///
/// Not a security control — the passphrase is that — but it catches the honest
/// mistake of uploading somebody else's dump, which would otherwise be detected
/// only after the live schema had been dropped.
fn sanity_check(sql: &str) -> Result<(), BackupError> {
    if !sql.contains("PostgreSQL database dump") {
        return Err(BackupError::BadArchive(
            "this decrypted, but it is not a PostgreSQL dump. If it is an audit archive \
             (.jsonl), use `blendbar-api import-audit-archive` instead."
                .into(),
        ));
    }

    // Tables that have existed since the first migration. A dump of some other
    // Postgres database will not have them.
    for table in ["customers", "employees", "_sqlx_migrations"] {
        if !sql.contains(&format!("public.{table}")) {
            return Err(BackupError::BadArchive(format!(
                "this is a PostgreSQL dump, but it has no '{table}' table — it does not \
                 look like a Blend Bar backup. Nothing was changed."
            )));
        }
    }
    Ok(())
}

/// The Postgres version the dump came from.
///
/// pg_dump stamps no timestamp, so there is no honest "taken at" to report —
/// the filename carries that, and inventing one from the file's mtime would be
/// a guess presented as a fact. The source version is real and worth showing: a
/// dump from a newer major version will not load into an older server.
fn source_version(sql: &str) -> Option<String> {
    sql.lines()
        .take(20)
        .find_map(|l| l.strip_prefix("-- Dumped from database version "))
        .map(|v| v.trim().to_string())
}

/// Swap the database name in a connection URL.
fn with_database(url: &str, name: &str) -> Result<String, BackupError> {
    let cut = url
        .rfind('/')
        .ok_or_else(|| BackupError::Internal("DATABASE_URL has no database path".into()))?;
    // Preserve any query string (sslmode etc.).
    let tail = url[cut + 1..]
        .find('?')
        .map(|i| url[cut + 1 + i..].to_string())
        .unwrap_or_default();
    Ok(format!("{}/{name}{tail}", &url[..cut]))
}

fn database_url() -> Result<String, BackupError> {
    std::env::var("DATABASE_URL").map_err(|_| BackupError::Internal("DATABASE_URL not set".into()))
}

/// Run psql against `url`, feeding `sql` on stdin.
async fn psql(url: &str, sql: &str, single_transaction: bool) -> Result<String, BackupError> {
    let mut args = vec!["-v", "ON_ERROR_STOP=1", "-q"];
    if single_transaction {
        args.push("--single-transaction");
    }

    let mut child = Command::new("psql")
        .args(&args)
        .arg(url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| BackupError::Internal(format!("could not run psql: {e}")))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| BackupError::Internal("psql stdin unavailable".into()))?;
    let owned = sql.to_string();
    // Written from a task: a large dump can exceed the pipe buffer, and writing
    // it inline would deadlock against a psql that is blocked writing output.
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(owned.as_bytes()).await;
        let _ = stdin.shutdown().await;
    });

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| BackupError::Internal(format!("psql failed: {e}")))?;
    let _ = writer.await;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // The last line is the one that names the failing statement.
        let detail = stderr
            .lines()
            .filter(|l| !l.trim().is_empty())
            .next_back()
            .unwrap_or("no output");
        return Err(BackupError::BadArchive(format!("psql: {detail}")));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

const COUNT_QUERY: &str = "select table_name || '|' || (xpath('/row/c/text()', \
     query_to_xml(format('select count(*) as c from %I.%I', table_schema, table_name), \
     false, true, '')))[1]::text \
     from information_schema.tables \
     where table_schema = 'public' and table_type = 'BASE TABLE' order by table_name";

async fn count_rows(url: &str) -> Result<Vec<TableCount>, BackupError> {
    let out = psql_query(url, COUNT_QUERY).await?;
    Ok(out
        .lines()
        .filter_map(|l| {
            let (table, rows) = l.trim().split_once('|')?;
            Some(TableCount {
                table: table.to_string(),
                rows: rows.trim().parse().unwrap_or(0),
            })
        })
        .collect())
}

async fn psql_query(url: &str, sql: &str) -> Result<String, BackupError> {
    let output = Command::new("psql")
        .args(["-tA", "-v", "ON_ERROR_STOP=1", "-c", sql])
        .arg(url)
        .output()
        .await
        .map_err(|e| BackupError::Internal(format!("could not run psql: {e}")))?;
    if !output.status.success() {
        return Err(BackupError::Internal(format!(
            "psql: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Load the dump into a throwaway database to prove it works, and report what is
/// in it. Never touches the live database.
async fn trial(sql: &str) -> Result<Report, BackupError> {
    let url = database_url()?;
    let admin = with_database(&url, "postgres")?;
    let trial_url = with_database(&url, TRIAL_DB)?;

    // Left over from an interrupted attempt, possibly.
    let _ = psql_query(&admin, &format!("drop database if exists {TRIAL_DB}")).await;
    psql_query(&admin, &format!("create database {TRIAL_DB}")).await?;

    let result = async {
        psql(&trial_url, sql, true).await?;
        count_rows(&trial_url).await
    }
    .await;

    // Always cleaned up, including on failure — a stray scratch database holding
    // a full copy of customer data is exactly the accident to avoid.
    let _ = psql_query(&admin, &format!("drop database if exists {TRIAL_DB}")).await;

    let counts = result?;
    Ok(Report {
        source_version: source_version(sql),
        tables: counts.len() as i64,
        rows: counts.iter().map(|c| c.rows).sum(),
        counts,
        sql_bytes: sql.len(),
    })
}

/// Write an encrypted copy of the current database to the safety volume.
async fn safety_copy() -> Result<String, BackupError> {
    let Some(pass) = passphrase::load() else {
        return Err(BackupError::NotConfigured(
            "no backup passphrase is set, so no safety copy could be taken. Refusing to \
             restore without a way back."
                .into(),
        ));
    };

    let plain = pg_dump().await?;
    let bytes = tokio::task::spawn_blocking(move || compress_and_encrypt(&plain, pass))
        .await
        .map_err(|e| BackupError::Internal(format!("encryption task failed: {e}")))??;

    std::fs::create_dir_all(SAFETY_DIR)
        .map_err(|e| BackupError::Internal(format!("could not create {SAFETY_DIR}: {e}")))?;

    let name = format!(
        "pre-restore-{}.sql.gz.age",
        Utc::now().format("%Y%m%d-%H%M%S")
    );
    let path = format!("{SAFETY_DIR}/{name}");
    std::fs::write(&path, &bytes)
        .map_err(|e| BackupError::Internal(format!("could not write the safety copy: {e}")))?;

    prune_safety_copies();
    tracing::info!(file = %name, bytes = bytes.len(), "safety copy written before restore");
    Ok(name)
}

fn prune_safety_copies() {
    let Ok(entries) = std::fs::read_dir(SAFETY_DIR) else {
        return;
    };
    let mut files: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("pre-restore-"))
        .collect();
    // Names are timestamped, so lexical order is chronological.
    files.sort_by_key(|e| e.file_name());
    while files.len() > KEEP_SAFETY_COPIES {
        let victim = files.remove(0);
        let _ = std::fs::remove_file(victim.path());
    }
}

pub fn list_safety_copies() -> Vec<(String, u64)> {
    let Ok(entries) = std::fs::read_dir(SAFETY_DIR) else {
        return Vec::new();
    };
    let mut out: Vec<(String, u64)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("pre-restore-"))
        .map(|e| {
            (
                e.file_name().to_string_lossy().to_string(),
                e.metadata().map(|m| m.len()).unwrap_or(0),
            )
        })
        .collect();
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out
}

/// Read a safety copy back for download. The name is validated rather than
/// trusted — it arrives from the browser and is used as a path.
pub fn read_safety_copy(name: &str) -> Result<Vec<u8>, BackupError> {
    if !name.starts_with("pre-restore-")
        || !name.ends_with(".sql.gz.age")
        || name.contains('/')
        || name.contains("..")
    {
        return Err(BackupError::BadArchive("no such safety copy".into()));
    }
    std::fs::read(format!("{SAFETY_DIR}/{name}"))
        .map_err(|_| BackupError::BadArchive("no such safety copy".into()))
}

/// Inspect an uploaded backup without changing anything.
pub async fn inspect(sealed: &[u8]) -> Result<Report, BackupError> {
    let Some(pass) = passphrase::load() else {
        return Err(BackupError::NotConfigured(
            "no backup passphrase is set, so an encrypted backup cannot be opened".into(),
        ));
    };
    let sql = super::decrypt_and_decompress(sealed, pass)?;
    sanity_check(&sql)?;
    trial(&sql).await
}

/// Replace the live database with an uploaded backup.
///
/// Every step before the live write is a chance to refuse, and each one does.
pub async fn restore(sealed: &[u8]) -> Result<Outcome, BackupError> {
    let Some(pass) = passphrase::load() else {
        return Err(BackupError::NotConfigured(
            "no backup passphrase is set, so an encrypted backup cannot be opened".into(),
        ));
    };

    let sql = super::decrypt_and_decompress(sealed, pass)?;
    sanity_check(&sql)?;

    // Prove it loads before destroying anything.
    let report = trial(&sql).await?;

    let url = database_url()?;
    let rows_before: i64 = count_rows(&url).await?.iter().map(|c| c.rows).sum();

    // The way back, taken before the point of no return.
    let safety = safety_copy().await?;

    // `drop schema public cascade` first: pg_dump does not emit it (nothing in
    // our dumps creates the schema), so without this the restore would collide
    // with every existing table. Wrapped with the dump in ONE transaction — if
    // any statement fails, the drop rolls back too and the original data is
    // still there.
    let payload = format!("drop schema public cascade;\ncreate schema public;\n{sql}");

    tracing::warn!(
        "RESTORE STARTING — replacing the live database from an uploaded backup \
         (safety copy: {safety})"
    );
    psql(&url, &payload, true).await?;

    // A backup from before the last deploy carries an older schema, and new code
    // against an old schema fails in tedious ways. Migrations are idempotent, so
    // this is a no-op when the backup was current.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .map_err(|e| BackupError::Internal(format!("could not reconnect after restore: {e}")))?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .map_err(|e| BackupError::Internal(format!("restored, but migrations failed: {e}")))?;

    let rows_after: i64 = count_rows(&url).await?.iter().map(|c| c.rows).sum();

    // Write the restore into the *restored* audit log, over this fresh
    // connection.
    //
    // The audit middleware will also try, and will probably fail: it uses the
    // shared pool, whose prepared statements were planned against the schema
    // that has just been dropped. Losing the record of the single most
    // destructive action in the application is not an acceptable outcome, so it
    // is written here explicitly rather than left to that race. A duplicate is
    // the better failure.
    //
    // It chains onto the restored log's head, which is the honest place for it:
    // this database's history now continues from the backup's.
    let logged = sqlx::query(
        "insert into admin_audit_log \
           (actor_email, actor_role, method, path, status, summary, detail) \
         values ('system', 'admin', 'POST', '/api/admin/backup/restore', 200, \
                 'restored the database from an uploaded backup', $1)",
    )
    .bind(serde_json::json!({
        "rows_before": rows_before,
        "rows_after": rows_after,
        "safety_copy": safety,
        "tables_restored": report.tables,
    }))
    .execute(&pool)
    .await;

    if let Err(e) = logged {
        tracing::error!("restore succeeded but could not be written to the audit log: {e}");
    }

    pool.close().await;

    tracing::warn!(
        "RESTORE COMPLETE — {rows_before} rows replaced with {rows_after}; \
         safety copy {safety}"
    );

    Ok(Outcome {
        restored: true,
        report,
        safety_copy: Some(safety),
        rows_before,
        rows_after,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_database_name_is_swapped_without_losing_the_query_string() {
        assert_eq!(
            with_database("postgres://u:p@db:5432/blendbar", "postgres").unwrap(),
            "postgres://u:p@db:5432/postgres"
        );
        assert_eq!(
            with_database("postgres://u:p@db:5432/blendbar?sslmode=require", "trial").unwrap(),
            "postgres://u:p@db:5432/trial?sslmode=require"
        );
    }

    #[test]
    fn a_dump_of_some_other_database_is_refused() {
        // Decrypting proves provenance; this catches the honest mistake of
        // uploading the wrong dump, before the live schema has been dropped.
        let foreign = "-- PostgreSQL database dump\nCREATE TABLE public.widgets (id int);\n";
        let err = sanity_check(foreign).unwrap_err().to_string();
        assert!(err.contains("customers"), "got: {err}");
        assert!(err.contains("Nothing was changed"));
    }

    #[test]
    fn an_audit_archive_uploaded_here_is_pointed_at_the_right_tool() {
        let archive = "{\"type\":\"blendbar-audit-archive\",\"version\":1}\n";
        let err = sanity_check(archive).unwrap_err().to_string();
        assert!(err.contains("import-audit-archive"), "got: {err}");
    }

    #[test]
    fn a_real_looking_dump_passes() {
        let ours = "-- PostgreSQL database dump\n\
                    COPY public.customers (id) FROM stdin;\n\
                    COPY public.employees (id) FROM stdin;\n\
                    COPY public._sqlx_migrations (version) FROM stdin;\n";
        assert!(sanity_check(ours).is_ok());
    }

    #[test]
    fn safety_copy_names_are_validated_rather_than_trusted() {
        // The name comes from the browser and is used as a path.
        assert!(read_safety_copy("../../etc/passwd").is_err());
        assert!(read_safety_copy("pre-restore-x/../../etc/passwd").is_err());
        assert!(read_safety_copy("something-else.sql.gz.age").is_err());
        assert!(read_safety_copy("pre-restore-20260101-000000.sql.gz.age.evil").is_err());
    }

    #[test]
    fn the_confirmation_phrase_names_the_consequence() {
        // Not "yes" or "ok" — it has to be a sentence somebody would not type by
        // reflex.
        assert!(CONFIRM_PHRASE.contains("REPLACE"));
        assert!(CONFIRM_PHRASE.len() > 8);
    }
}
