//! Applying a managed-entries change to the hosts file. Runs either directly (when the user
//! can write the file) or inside the elevated helper — the same code either way.
//!
//! The request carries only the desired *managed entries* and the SHA-256 of the file the
//! user previewed. The file itself is re-read here, so nothing outside Devluma's block can be
//! changed even by a tampered request.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::domain::hosts::{HostsFile, ManagedEntry};

pub const VERB_APPLY: &str = "hosts.apply";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostsRequest {
    pub verb: String,
    pub expected_sha256_before: String,
    pub entries: Vec<ManagedEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostsResult {
    pub ok: bool,
    /// One of the IPC error codes when `ok` is false.
    pub error_code: Option<String>,
    pub message: String,
    pub sha256_before: Option<String>,
    pub sha256_after: Option<String>,
    pub backup_path: Option<String>,
    pub changed: bool,
}

impl HostsResult {
    fn fail(code: &str, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error_code: Some(code.into()),
            message: message.into(),
            sha256_before: None,
            sha256_after: None,
            backup_path: None,
            changed: false,
        }
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// `%SystemRoot%\System32\drivers\etc\hosts`, resolved through the API rather than the
/// environment (which a caller could influence).
pub fn system_hosts_path() -> std::io::Result<PathBuf> {
    use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut buf = [0u16; 512];
    // SAFETY: buffer is writable for its full length.
    let n = unsafe { GetSystemDirectoryW(Some(&mut buf)) } as usize;
    if n == 0 || n >= buf.len() {
        return Err(std::io::Error::last_os_error());
    }
    Ok(PathBuf::from(String::from_utf16_lossy(&buf[..n]))
        .join("drivers")
        .join("etc")
        .join("hosts"))
}

/// Replaces `target` with `replacement`, keeping the original's ACL and attributes.
fn replace_file(target: &Path, replacement: &Path) -> std::io::Result<()> {
    use windows::Win32::Storage::FileSystem::{REPLACEFILE_WRITE_THROUGH, ReplaceFileW};
    let w = |p: &Path| -> Vec<u16> {
        OsStr::new(p)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    let (t, r) = (w(target), w(replacement));
    // SAFETY: NUL-terminated paths outlive the call; no backup file, no reserved params.
    unsafe {
        ReplaceFileW(
            windows::core::PCWSTR(t.as_ptr()),
            windows::core::PCWSTR(r.as_ptr()),
            windows::core::PCWSTR::null(),
            REPLACEFILE_WRITE_THROUGH,
            None,
            None,
        )
    }
    .map_err(|e| std::io::Error::other(e.message()))
}

/// Applies `req` to the hosts file at `hosts_path`, writing a backup into `backups_dir`.
pub fn apply(hosts_path: &Path, backups_dir: &Path, req: &HostsRequest) -> HostsResult {
    if req.verb != VERB_APPLY {
        return HostsResult::fail(
            "VALIDATION_ERROR",
            format!("Unsupported request {:?}.", req.verb),
        );
    }
    let bytes = match std::fs::read(hosts_path) {
        Ok(b) => b,
        Err(e) => {
            return HostsResult::fail(
                "PERMISSION_DENIED",
                format!("Could not read the hosts file: {e}"),
            );
        }
    };
    let before = sha256_hex(&bytes);
    if before != req.expected_sha256_before {
        return HostsResult::fail(
            "VALIDATION_ERROR",
            "The hosts file changed after you reviewed the change. Nothing was written; review it again.",
        );
    }
    let Ok(text) = String::from_utf8(bytes.clone()) else {
        return HostsResult::fail(
            "VALIDATION_ERROR",
            "The hosts file contains text that isn't UTF-8; Devluma won't rewrite it. Edit it manually.",
        );
    };
    let file = match HostsFile::parse(&text) {
        Ok(f) => f,
        Err(e) => return HostsResult::fail("VALIDATION_ERROR", e.to_string()),
    };
    let new_text = match file.with_managed(&req.entries) {
        Ok(t) => t,
        Err(e) => return HostsResult::fail("VALIDATION_ERROR", e.to_string()),
    };
    if new_text == text {
        return HostsResult {
            ok: true,
            error_code: None,
            message: "The hosts file already has these entries.".into(),
            sha256_before: Some(before.clone()),
            sha256_after: Some(before),
            backup_path: None,
            changed: false,
        };
    }

    // Backup first; never overwrite an existing backup.
    if let Err(e) = std::fs::create_dir_all(backups_dir) {
        return HostsResult::fail(
            "INTERNAL",
            format!("Could not create the backup folder: {e}"),
        );
    }
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ");
    let backup = backups_dir.join(format!("hosts-{stamp}.bak"));
    if let Err(e) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backup)
        .and_then(|mut f| std::io::Write::write_all(&mut f, &bytes))
    {
        return HostsResult::fail(
            "INTERNAL",
            format!("Could not back up the hosts file; nothing was changed: {e}"),
        );
    }

    // Atomic replace from a temp file next to the target.
    let tmp = hosts_path.with_file_name("hosts.devluma-tmp");
    let _ = std::fs::remove_file(&tmp);
    let written = std::fs::write(&tmp, new_text.as_bytes()).and_then(|_| {
        replace_file(hosts_path, &tmp).or_else(|_| std::fs::rename(&tmp, hosts_path))
    });
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return HostsResult::fail(
            "PERMISSION_DENIED",
            format!("Could not write the hosts file; it was not changed: {e}"),
        );
    }

    // Verify by re-reading.
    let after = std::fs::read(hosts_path)
        .map(|b| sha256_hex(&b))
        .unwrap_or_default();
    let expected = sha256_hex(new_text.as_bytes());
    if after != expected {
        return HostsResult::fail(
            "INTERNAL",
            format!(
                "The hosts file didn't contain the expected content after writing (another program may have changed it). A backup is at {}.",
                backup.display()
            ),
        );
    }
    HostsResult {
        ok: true,
        error_code: None,
        message: "Hosts file updated.".into(),
        sha256_before: Some(before),
        sha256_after: Some(after),
        backup_path: Some(backup.display().to_string()),
        changed: true,
    }
}

// ---- elevated helper entry point ------------------------------------------------------

pub const REQUEST_FILE: &str = "elevate-request.json";
pub const RESULT_FILE: &str = "elevate-result.json";

/// The helper's whole job (`devluma-elevate.exe <request path> <request sha256>`):
/// verify the request file's hash, check it lives in a Devluma staging folder, apply it to
/// `hosts_path`, and write the result next to the request. Returns the process exit code.
pub fn helper_main(args: &[String], hosts_path: &Path) -> i32 {
    let (Some(request_path), Some(expected_sha)) = (args.get(1), args.get(2)) else {
        return 2;
    };
    let request_path = PathBuf::from(request_path);
    let write_result = |r: &HostsResult| {
        if let Some(dir) = request_path.parent() {
            let _ = std::fs::write(
                dir.join(RESULT_FILE),
                serde_json::to_vec(r).unwrap_or_default(),
            );
        }
    };

    // <root>/staging/<operation>/elevate-request.json → backups in <root>/backups/hosts.
    let layout_ok = request_path.file_name() == Some(OsStr::new(REQUEST_FILE))
        && request_path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            == Some(OsStr::new("staging"));
    if !layout_ok {
        return 3;
    }
    let root = request_path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .unwrap_or(Path::new("."));
    let backups = root.join("backups").join("hosts");

    let bytes = match std::fs::read(&request_path) {
        Ok(b) => b,
        Err(_) => return 4,
    };
    if sha256_hex(&bytes) != *expected_sha {
        write_result(&HostsResult::fail(
            "VALIDATION_ERROR",
            "The elevation request was modified after it was created; nothing was changed.",
        ));
        return 5;
    }
    let req: HostsRequest = match serde_json::from_slice(&bytes) {
        Ok(r) => r,
        Err(e) => {
            write_result(&HostsResult::fail(
                "VALIDATION_ERROR",
                format!("Malformed request: {e}"),
            ));
            return 6;
        }
    };
    let result = apply(hosts_path, &backups, &req);
    write_result(&result);
    if result.ok { 0 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::hosts::BEGIN;

    const ORIGINAL: &str = "# Copyright (c) Microsoft\r\n127.0.0.1 localhost\r\n";

    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let hosts = dir.path().join("hosts");
        std::fs::write(&hosts, ORIGINAL).unwrap();
        let backups = dir.path().join("backups");
        (dir, hosts, backups)
    }

    fn req(entries: &[&str], sha: &str) -> HostsRequest {
        HostsRequest {
            verb: VERB_APPLY.into(),
            expected_sha256_before: sha.into(),
            entries: entries
                .iter()
                .map(|d| ManagedEntry {
                    ip: "127.0.0.1".into(),
                    domain: (*d).into(),
                })
                .collect(),
        }
    }

    #[test]
    fn backs_up_writes_and_verifies() {
        let (_d, hosts, backups) = setup();
        let r = apply(
            &hosts,
            &backups,
            &req(&["shop.test"], &sha256_hex(ORIGINAL.as_bytes())),
        );
        assert!(r.ok && r.changed, "{r:?}");
        let text = std::fs::read_to_string(&hosts).unwrap();
        assert!(text.starts_with(ORIGINAL) && text.contains("127.0.0.1 shop.test\r\n"));
        assert_eq!(
            std::fs::read_to_string(r.backup_path.unwrap()).unwrap(),
            ORIGINAL
        );
        assert_eq!(r.sha256_after.unwrap(), sha256_hex(text.as_bytes()));

        // Applying the same entries again is a no-op without another backup.
        let r2 = apply(
            &hosts,
            &backups,
            &req(&["shop.test"], &sha256_hex(text.as_bytes())),
        );
        assert!(r2.ok && !r2.changed && r2.backup_path.is_none());
        assert_eq!(std::fs::read_dir(&backups).unwrap().count(), 1);
    }

    #[test]
    fn refuses_if_the_file_changed_since_preview() {
        let (_d, hosts, backups) = setup();
        let r = apply(&hosts, &backups, &req(&["shop.test"], &"0".repeat(64)));
        assert!(!r.ok);
        assert!(r.message.contains("changed after you reviewed"));
        assert_eq!(std::fs::read_to_string(&hosts).unwrap(), ORIGINAL);
        assert!(!backups.exists());
    }

    #[test]
    fn refuses_non_loopback_and_non_utf8() {
        let (_d, hosts, backups) = setup();
        let mut bad = req(&["bank.test"], &sha256_hex(ORIGINAL.as_bytes()));
        bad.entries[0].ip = "203.0.113.9".into();
        assert!(!apply(&hosts, &backups, &bad).ok);
        assert_eq!(std::fs::read_to_string(&hosts).unwrap(), ORIGINAL);

        let latin1 = b"127.0.0.1 caf\xe9.test\r\n";
        std::fs::write(&hosts, latin1).unwrap();
        let r = apply(&hosts, &backups, &req(&["x.test"], &sha256_hex(latin1)));
        assert!(!r.ok && r.message.contains("UTF-8"));
        assert_eq!(std::fs::read(&hosts).unwrap(), latin1);
    }

    #[test]
    fn helper_checks_request_hash_and_location() {
        let (d, hosts, _backups) = setup();
        let op_dir = d.path().join("staging").join("op1");
        std::fs::create_dir_all(&op_dir).unwrap();
        let request = op_dir.join(REQUEST_FILE);
        let body =
            serde_json::to_vec(&req(&["shop.test"], &sha256_hex(ORIGINAL.as_bytes()))).unwrap();
        std::fs::write(&request, &body).unwrap();
        let args = |sha: &str| {
            vec![
                "devluma-elevate".to_owned(),
                request.display().to_string(),
                sha.to_owned(),
            ]
        };

        // Wrong hash: refused, file untouched, reason reported.
        assert_eq!(helper_main(&args(&"f".repeat(64)), &hosts), 5);
        assert_eq!(std::fs::read_to_string(&hosts).unwrap(), ORIGINAL);
        let result: HostsResult =
            serde_json::from_slice(&std::fs::read(op_dir.join(RESULT_FILE)).unwrap()).unwrap();
        assert!(result.message.contains("modified"));

        // Correct hash: applied, backup under <root>/backups/hosts.
        assert_eq!(helper_main(&args(&sha256_hex(&body)), &hosts), 0);
        assert!(std::fs::read_to_string(&hosts).unwrap().contains(BEGIN));
        assert_eq!(
            std::fs::read_dir(d.path().join("backups").join("hosts"))
                .unwrap()
                .count(),
            1
        );

        // A request outside a staging folder is refused outright.
        let stray = d.path().join(REQUEST_FILE);
        std::fs::write(&stray, &body).unwrap();
        let stray_args = vec![
            "x".to_owned(),
            stray.display().to_string(),
            sha256_hex(&body),
        ];
        assert_eq!(helper_main(&stray_args, &hosts), 3);
    }

    #[test]
    fn rejects_unknown_fields_and_verbs() {
        let (_d, hosts, backups) = setup();
        let mut r = req(&["shop.test"], &sha256_hex(ORIGINAL.as_bytes()));
        r.verb = "hosts.write_raw".into();
        assert!(!apply(&hosts, &backups, &r).ok);
        let extra =
            r#"{"verb":"hosts.apply","expectedSha256Before":"x","entries":[],"content":"evil"}"#;
        assert!(serde_json::from_str::<HostsRequest>(extra).is_err());
    }

    #[test]
    fn system_hosts_path_points_into_system32() {
        let p = system_hosts_path().unwrap();
        assert!(
            p.to_string_lossy()
                .to_lowercase()
                .ends_with(r"system32\drivers\etc\hosts")
        );
    }
}
