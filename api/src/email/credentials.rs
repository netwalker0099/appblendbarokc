//! Storing the Google service-account key that an admin uploads.
//!
//! The key goes to a file on a mounted volume, never to a database column, so it
//! stays out of `GET /api/admin/backup` — that endpoint hands an admin a full
//! pg_dump, and a credential in a table would ride along in every copy of it.
//!
//! Precedence is: a key set in the environment wins over one uploaded through the
//! browser. An ops-managed deployment should not be silently overridden by
//! someone pasting into a form, and if both exist the environment is the one the
//! operator can actually see.

use std::io::Write;
use std::path::PathBuf;

use super::gmail::ServiceAccount;
use super::MailError;

/// Where an uploaded key is written. A volume in compose, so it survives the
/// container being recreated on deploy.
const STORED_KEY_PATH: &str = "/var/lib/blendbar/secrets/google-sa.json";

/// True when the environment provides a key, in which case the UI is read-only.
pub fn env_managed() -> bool {
    std::env::var("GOOGLE_SA_KEY_FILE")
        .ok()
        .is_some_and(|s| !s.trim().is_empty())
        || std::env::var("GOOGLE_SA_KEY_JSON")
            .ok()
            .is_some_and(|s| !s.trim().is_empty())
}

pub fn stored_path() -> PathBuf {
    PathBuf::from(STORED_KEY_PATH)
}

pub fn stored_key_exists() -> bool {
    stored_path().exists()
}

/// Parse and sanity-check a pasted key without keeping it.
///
/// Rejects at upload time rather than at 2am on a customer's sign-in email:
/// wrong file type (an OAuth *client* JSON is the usual mistake), or a private
/// key the signer cannot actually use.
pub fn validate(raw: &str) -> Result<ServiceAccount, MailError> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| MailError::NotConfigured(format!("that isn't valid JSON: {e}")))?;

    // An OAuth client-secret file has a top-level "web" or "installed" key and no
    // private_key. It is the file people download by mistake, so name it.
    if value.get("web").is_some() || value.get("installed").is_some() {
        return Err(MailError::NotConfigured(
            "that looks like an OAuth client-secret file. This needs a *service \
             account* key: Google Cloud → IAM & Admin → Service Accounts → Keys → \
             Add key → JSON."
                .into(),
        ));
    }

    match value.get("type").and_then(|t| t.as_str()) {
        Some("service_account") => {}
        Some(other) => {
            return Err(MailError::NotConfigured(format!(
                "expected a service account key, but this file says type \"{other}\""
            )))
        }
        None => {
            return Err(MailError::NotConfigured(
                "this JSON has no \"type\" field — it doesn't look like a service account key"
                    .into(),
            ))
        }
    }

    let account: ServiceAccount = serde_json::from_str(raw).map_err(|e| {
        MailError::NotConfigured(format!("the key is missing something it needs: {e}"))
    })?;

    if account.client_email.trim().is_empty() {
        return Err(MailError::NotConfigured(
            "the key has no client_email".into(),
        ));
    }

    // Proves the private key actually signs before it is stored.
    jsonwebtoken::EncodingKey::from_rsa_pem(account.private_key.as_bytes()).map_err(|e| {
        MailError::NotConfigured(format!("the private key in that file is unusable: {e}"))
    })?;

    Ok(account)
}

/// Write the key to disk, replacing any previous one.
pub fn store(raw: &str) -> Result<ServiceAccount, MailError> {
    let account = validate(raw)?;
    let path = stored_path();

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| {
            MailError::NotConfigured(format!("could not create {}: {e}", dir.display()))
        })?;
    }

    // Written 0600 and only then moved into place, so there is never a window
    // where the key is readable by anything else on the box.
    let tmp = path.with_extension("tmp");
    let mut file = std::fs::File::create(&tmp)
        .map_err(|e| MailError::NotConfigured(format!("could not write the key: {e}")))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
    }

    file.write_all(raw.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|e| MailError::NotConfigured(format!("could not write the key: {e}")))?;
    drop(file);

    std::fs::rename(&tmp, &path)
        .map_err(|e| MailError::NotConfigured(format!("could not save the key: {e}")))?;

    Ok(account)
}

/// Load a previously stored key, if there is one.
pub fn load_stored() -> Option<Result<ServiceAccount, MailError>> {
    let path = stored_path();
    if !path.exists() {
        return None;
    }
    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(e) => {
            return Some(Err(MailError::NotConfigured(format!(
                "could not read the stored key: {e}"
            ))))
        }
    };
    Some(validate(&raw))
}

pub fn remove_stored() -> Result<bool, MailError> {
    let path = stored_path();
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(&path)
        .map(|_| true)
        .map_err(|e| MailError::NotConfigured(format!("could not remove the key: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    // A throwaway 2048-bit key, generated for this test only. Never used to sign
    // anything real — it exists so the validator can be exercised end to end.
    const TEST_PEM: &str = include_str!("testdata/test_key.pem");

    fn key_json(extra: &str) -> String {
        format!(
            r#"{{"type":"service_account","project_id":"p","private_key_id":"kid",
                 "private_key":{},"client_email":"bot@p.iam.gserviceaccount.com",
                 "client_id":"123"{extra}}}"#,
            serde_json::to_string(TEST_PEM).unwrap()
        )
    }

    #[test]
    fn accepts_a_real_service_account_key() {
        let account = validate(&key_json("")).expect("should accept");
        assert_eq!(account.client_email, "bot@p.iam.gserviceaccount.com");
    }

    #[test]
    fn names_the_oauth_client_file_mistake() {
        // The file people actually download by accident.
        let err = validate(r#"{"web":{"client_id":"x","client_secret":"y"}}"#).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("service account"), "{msg}");
        assert!(msg.contains("OAuth client"), "{msg}");
    }

    #[test]
    fn rejects_the_wrong_account_type() {
        let err = validate(r#"{"type":"authorized_user","client_id":"x"}"#).unwrap_err();
        assert!(err.to_string().contains("authorized_user"));
    }

    #[test]
    fn rejects_junk_and_missing_fields() {
        assert!(validate("not json at all").is_err());
        assert!(validate("{}").is_err());
    }

    #[test]
    fn rejects_a_key_that_cannot_sign() {
        // Right shape, unusable private key. Must fail at upload, not silently
        // at 2am on the first customer sign-in email. Built directly rather than
        // by string-replacing into key_json(), because the PEM is JSON-escaped
        // in there and a naive replace matches nothing.
        let bad = format!(
            r#"{{"type":"service_account","project_id":"p","private_key_id":"kid",
                 "private_key":{},"client_email":"bot@p.iam.gserviceaccount.com",
                 "client_id":"123"}}"#,
            serde_json::to_string("-----BEGIN PRIVATE KEY-----\nnope\n-----END PRIVATE KEY-----")
                .unwrap()
        );
        assert!(validate(&bad).is_err(), "an unusable key was accepted");
    }
}
