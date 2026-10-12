//! Windows hosts file model (app_flow.md §10, ADR 0007).
//!
//! Devluma only ever edits its own marked block:
//!
//! ```text
//! # BEGIN Devluma (managed entries; edit them in Devluma)
//! 127.0.0.1 shop.test
//! # END Devluma
//! ```
//!
//! Every other byte of the file — comments, external entries, blank lines, line endings —
//! is preserved exactly. Managed entries may only point at loopback addresses.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

pub const BEGIN: &str = "# BEGIN Devluma (managed entries; edit them in Devluma)";
pub const END: &str = "# END Devluma";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedEntry {
    pub ip: String,
    pub domain: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostsLine {
    /// 1-based line number in the file.
    pub line: usize,
    pub ip: String,
    pub hostnames: Vec<String>,
    pub comment: Option<String>,
    pub managed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostsFile {
    /// Original text, unchanged.
    pub text: String,
    pub entries: Vec<HostsLine>,
    /// Line index range `[begin, end]` of the managed block, inclusive of markers.
    block: Option<(usize, usize)>,
    newline: &'static str,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HostsError {
    #[error(
        "the hosts file has a Devluma section without a matching end marker; fix it manually first"
    )]
    UnbalancedBlock,
    #[error("{0} is not a loopback address; Devluma only maps domains to this computer")]
    NotLoopback(String),
    #[error("invalid domain {0}: {1}")]
    InvalidDomain(String, String),
    #[error(
        "{domain} is already mapped to {ip} outside Devluma's section (line {line}); change or remove that line first"
    )]
    ExternalConflict {
        domain: String,
        ip: String,
        line: usize,
    },
}

impl HostsFile {
    pub fn parse(text: &str) -> Result<Self, HostsError> {
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let lines: Vec<&str> = text.lines().collect();
        let begin = lines.iter().position(|l| l.trim() == BEGIN);
        let end = lines.iter().position(|l| l.trim() == END);
        let block = match (begin, end) {
            (None, None) => None,
            (Some(b), Some(e)) if e > b => Some((b, e)),
            _ => return Err(HostsError::UnbalancedBlock),
        };
        let mut entries = Vec::new();
        for (i, raw) in lines.iter().enumerate() {
            let (content, comment) = match raw.split_once('#') {
                Some((c, rest)) => (c, Some(rest.trim().to_owned()).filter(|s| !s.is_empty())),
                None => (*raw, None),
            };
            let mut parts = content.split_whitespace();
            let Some(ip) = parts.next() else { continue };
            let hostnames: Vec<String> = parts.map(|h| h.to_lowercase()).collect();
            if hostnames.is_empty() || ip.parse::<IpAddr>().is_err() {
                continue;
            }
            entries.push(HostsLine {
                line: i + 1,
                ip: ip.to_owned(),
                hostnames,
                comment,
                managed: block.is_some_and(|(b, e)| i > b && i < e),
            });
        }
        Ok(Self {
            text: text.to_owned(),
            entries,
            block,
            newline,
        })
    }

    pub fn managed(&self) -> Vec<ManagedEntry> {
        self.entries
            .iter()
            .filter(|e| e.managed)
            .flat_map(|e| {
                e.hostnames.iter().map(|h| ManagedEntry {
                    ip: e.ip.clone(),
                    domain: h.clone(),
                })
            })
            .collect()
    }

    /// The file with Devluma's block replaced by `entries` (removed if empty). Everything
    /// outside the block is copied verbatim.
    pub fn with_managed(&self, entries: &[ManagedEntry]) -> Result<String, HostsError> {
        let entries = validate(entries)?;
        for entry in &entries {
            if let Some(ext) = self
                .entries
                .iter()
                .find(|e| !e.managed && e.hostnames.iter().any(|h| h == &entry.domain))
                && ext.ip != entry.ip
            {
                return Err(HostsError::ExternalConflict {
                    domain: entry.domain.clone(),
                    ip: ext.ip.clone(),
                    line: ext.line,
                });
            }
        }
        let nl = self.newline;
        let mut block = String::new();
        if !entries.is_empty() {
            block.push_str(BEGIN);
            block.push_str(nl);
            for e in &entries {
                block.push_str(&format!("{} {}{nl}", e.ip, e.domain));
            }
            block.push_str(END);
            block.push_str(nl);
        }

        // Split the original into raw lines keeping their terminators.
        let raw: Vec<&str> = self.text.split_inclusive('\n').collect();
        let mut out = String::with_capacity(self.text.len() + block.len());
        match self.block {
            Some((b, e)) => {
                for l in &raw[..b] {
                    out.push_str(l);
                }
                out.push_str(&block);
                for l in &raw[e + 1..] {
                    out.push_str(l);
                }
            }
            None if entries.is_empty() => out.push_str(&self.text),
            None => {
                out.push_str(&self.text);
                if !self.text.is_empty() && !self.text.ends_with('\n') {
                    out.push_str(nl);
                }
                out.push_str(&block);
            }
        }
        Ok(out)
    }
}

/// Normalizes, de-duplicates (by domain) and validates managed entries.
pub fn validate(entries: &[ManagedEntry]) -> Result<Vec<ManagedEntry>, HostsError> {
    let mut out: Vec<ManagedEntry> = Vec::new();
    for e in entries {
        let ip: IpAddr =
            e.ip.parse()
                .map_err(|_| HostsError::NotLoopback(e.ip.clone()))?;
        if !ip.is_loopback() {
            return Err(HostsError::NotLoopback(e.ip.clone()));
        }
        let domain = super::domain_name::check(&e.domain)
            .map_err(|m| HostsError::InvalidDomain(e.domain.clone(), m))?
            .normalized;
        out.retain(|x| x.domain != domain);
        out.push(ManagedEntry {
            ip: ip.to_string(),
            domain,
        });
    }
    out.sort_by(|a, b| a.domain.cmp(&b.domain));
    Ok(out)
}

/// Unified diff for previews.
pub fn diff(old: &str, new: &str, path: &str) -> String {
    similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(2)
        .header(path, path)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOWS_DEFAULT: &str = "# Copyright (c) 1993-2009 Microsoft Corp.\r\n#\r\n# localhost name resolution is handled within DNS itself.\r\n#\t127.0.0.1       localhost\r\n#\t::1             localhost\r\n";

    fn entry(domain: &str) -> ManagedEntry {
        ManagedEntry {
            ip: "127.0.0.1".into(),
            domain: domain.into(),
        }
    }

    #[test]
    fn adds_a_block_and_preserves_everything_else_byte_for_byte() {
        let f = HostsFile::parse(WINDOWS_DEFAULT).unwrap();
        let out = f.with_managed(&[entry("Shop.test")]).unwrap();
        assert!(out.starts_with(WINDOWS_DEFAULT), "original text untouched");
        assert!(
            out.ends_with(&format!("{BEGIN}\r\n127.0.0.1 shop.test\r\n{END}\r\n")),
            "CRLF kept: {out:?}"
        );

        // Round trip: replacing the block keeps the rest and is idempotent.
        let f2 = HostsFile::parse(&out).unwrap();
        assert_eq!(f2.managed(), vec![entry("shop.test")]);
        assert_eq!(f2.with_managed(&f2.managed()).unwrap(), out);

        // Removing all managed entries restores the original exactly.
        assert_eq!(f2.with_managed(&[]).unwrap(), WINDOWS_DEFAULT);
    }

    #[test]
    fn replaces_only_the_block_in_the_middle_of_a_file() {
        let text = format!(
            "10.0.0.5 nas.lan # my NAS\n{BEGIN}\n127.0.0.1 old.test\n{END}\n# trailing comment\n"
        );
        let f = HostsFile::parse(&text).unwrap();
        assert_eq!(f.entries.len(), 2);
        assert!(!f.entries[0].managed && f.entries[1].managed);
        assert_eq!(f.entries[0].comment.as_deref(), Some("my NAS"));
        let out = f.with_managed(&[entry("a.test"), entry("b.test")]).unwrap();
        assert_eq!(
            out,
            format!(
                "10.0.0.5 nas.lan # my NAS\n{BEGIN}\n127.0.0.1 a.test\n127.0.0.1 b.test\n{END}\n# trailing comment\n"
            )
        );
    }

    #[test]
    fn handles_files_without_trailing_newline() {
        let f = HostsFile::parse("127.0.0.1 localhost").unwrap();
        let out = f.with_managed(&[entry("x.test")]).unwrap();
        assert_eq!(
            out,
            format!("127.0.0.1 localhost\n{BEGIN}\n127.0.0.1 x.test\n{END}\n")
        );
    }

    #[test]
    fn only_loopback_and_valid_domains() {
        let f = HostsFile::parse("").unwrap();
        let bad_ip = ManagedEntry {
            ip: "93.184.216.34".into(),
            domain: "bank.test".into(),
        };
        assert!(matches!(
            f.with_managed(&[bad_ip]),
            Err(HostsError::NotLoopback(_))
        ));
        let ipv6 = ManagedEntry {
            ip: "::1".into(),
            domain: "v6.test".into(),
        };
        assert!(f.with_managed(&[ipv6]).is_ok());
        assert!(matches!(
            f.with_managed(&[entry("bad domain.test")]),
            Err(HostsError::InvalidDomain(..))
        ));
    }

    #[test]
    fn refuses_to_shadow_an_external_mapping() {
        let f = HostsFile::parse("10.1.2.3 shop.test\n").unwrap();
        assert!(matches!(
            f.with_managed(&[entry("shop.test")]),
            Err(HostsError::ExternalConflict { line: 1, .. })
        ));
        // Same loopback mapping outside the block is fine (no conflict).
        let f = HostsFile::parse("127.0.0.1 shop.test\n").unwrap();
        assert!(f.with_managed(&[entry("shop.test")]).is_ok());
    }

    #[test]
    fn unbalanced_markers_are_an_error_not_a_guess() {
        assert_eq!(
            HostsFile::parse(&format!("{BEGIN}\n127.0.0.1 a.test\n")),
            Err(HostsError::UnbalancedBlock)
        );
        assert_eq!(
            HostsFile::parse(&format!("{END}\n{BEGIN}\n")),
            Err(HostsError::UnbalancedBlock)
        );
    }

    #[test]
    fn diff_shows_exact_changes() {
        let f = HostsFile::parse("127.0.0.1 localhost\n").unwrap();
        let out = f.with_managed(&[entry("shop.test")]).unwrap();
        let d = diff(&f.text, &out, "hosts");
        assert!(d.contains("+127.0.0.1 shop.test"), "{d}");
        assert!(!d.contains("-127.0.0.1 localhost"));
    }
}
