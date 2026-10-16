//! Windows hosts file model (app_flow.md §10, ADR 0007, ADR 0016).
//!
//! Devluma only ever edits its own lines, recognised by a trailing tag:
//!
//! ```text
//! 127.0.0.1    shop.test    # managed by devluma      (tab-separated in the file)
//! ```
//!
//! A tagged line counts as Devluma's only when it points at a loopback address; a tag on any
//! other mapping is reported and never touched. Every other byte of the file — comments,
//! other entries (including an old `# BEGIN Devluma` block), blank lines, line endings — is
//! preserved exactly.

use std::collections::BTreeMap;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// The comment that marks Devluma's lines (compared case-insensitively, spaces trimmed).
pub const MARKER: &str = "managed by devluma";
/// How Devluma writes its tag.
pub const TAG: &str = "# managed by devluma";

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
    /// Lines carrying Devluma's tag on a non-loopback address: reported, never edited.
    pub stray_tags: Vec<HostsLine>,
    newline: &'static str,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HostsError {
    #[error("{0} is not a loopback address; Devluma only maps domains to this computer")]
    NotLoopback(String),
    #[error("invalid domain {0}: {1}")]
    InvalidDomain(String, String),
    #[error(
        "{domain} is already mapped to {ip} by another line (line {line}); change or remove that line first"
    )]
    ExternalConflict {
        domain: String,
        ip: String,
        line: usize,
    },
}

fn is_tag(comment: Option<&str>) -> bool {
    comment.is_some_and(|c| c.trim().eq_ignore_ascii_case(MARKER))
}

impl HostsFile {
    pub fn parse(text: &str) -> Self {
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let mut entries = Vec::new();
        let mut stray_tags = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let (content, comment) = match raw.split_once('#') {
                Some((c, rest)) => (c, Some(rest.trim().to_owned()).filter(|s| !s.is_empty())),
                None => (raw, None),
            };
            let mut parts = content.split_whitespace();
            let Some(ip) = parts.next() else { continue };
            let hostnames: Vec<String> = parts.map(|h| h.to_lowercase()).collect();
            let Ok(addr) = ip.parse::<IpAddr>() else {
                continue;
            };
            if hostnames.is_empty() {
                continue;
            }
            let tagged = is_tag(comment.as_deref());
            let line = HostsLine {
                line: i + 1,
                ip: ip.to_owned(),
                hostnames,
                comment,
                managed: tagged && addr.is_loopback(),
            };
            if tagged && !addr.is_loopback() {
                stray_tags.push(line.clone());
            }
            entries.push(line);
        }
        Self {
            text: text.to_owned(),
            entries,
            stray_tags,
            newline,
        }
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

    /// Lines in the file that aren't Devluma's (for "Devluma leaves N entries alone").
    pub fn other_count(&self) -> usize {
        self.entries.iter().filter(|e| !e.managed).count()
    }

    /// The file with Devluma's tagged lines set to exactly `entries`: kept lines stay where
    /// they are (normalised), dropped ones are removed, new ones are appended. Every untagged
    /// byte is copied verbatim.
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
        let mut remaining: BTreeMap<&str, &str> = entries
            .iter()
            .map(|e| (e.domain.as_str(), e.ip.as_str()))
            .collect();
        let managed_lines: BTreeMap<usize, &HostsLine> = self
            .entries
            .iter()
            .filter(|e| e.managed)
            .map(|e| (e.line, e))
            .collect();

        let mut out = String::with_capacity(self.text.len() + entries.len() * 48);
        for (i, raw) in self.text.split_inclusive('\n').enumerate() {
            let Some(line) = managed_lines.get(&(i + 1)) else {
                out.push_str(raw);
                continue;
            };
            for host in &line.hostnames {
                if let Some(ip) = remaining.remove(host.as_str()) {
                    out.push_str(&tagged_line(ip, host, nl));
                }
            }
        }
        if !remaining.is_empty() {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push_str(nl);
            }
            for (domain, ip) in remaining {
                out.push_str(&tagged_line(ip, domain, nl));
            }
        }
        Ok(out)
    }
}

fn tagged_line(ip: &str, domain: &str, nl: &str) -> String {
    format!("{ip}\t{domain}\t{TAG}{nl}")
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
    fn appends_tagged_lines_and_preserves_everything_else_byte_for_byte() {
        let f = HostsFile::parse(WINDOWS_DEFAULT);
        let out = f.with_managed(&[entry("Shop.test")]).unwrap();
        assert_eq!(
            out,
            format!("{WINDOWS_DEFAULT}127.0.0.1\tshop.test\t# managed by devluma\r\n"),
            "CRLF kept"
        );

        // Round trip: rewriting the same entries is idempotent.
        let f2 = HostsFile::parse(&out);
        assert_eq!(f2.managed(), vec![entry("shop.test")]);
        assert_eq!(f2.with_managed(&f2.managed()).unwrap(), out);

        // Removing all managed entries restores the original exactly.
        assert_eq!(f2.with_managed(&[]).unwrap(), WINDOWS_DEFAULT);
    }

    #[test]
    fn edits_tagged_lines_in_place_and_leaves_other_lines_alone() {
        let text = "10.0.0.5 nas.lan # my NAS\n127.0.0.1   old.test   #Managed By Devluma\n127.0.0.1 child.local #laragon magic!\n# trailing comment\n127.0.0.1 keep.test # managed by devluma\n";
        let f = HostsFile::parse(text);
        assert_eq!(f.other_count(), 2);
        assert_eq!(f.managed(), vec![entry("old.test"), entry("keep.test")]);
        let out = f
            .with_managed(&[entry("keep.test"), entry("new.test")])
            .unwrap();
        assert_eq!(
            out,
            "10.0.0.5 nas.lan # my NAS\n127.0.0.1 child.local #laragon magic!\n# trailing comment\n127.0.0.1\tkeep.test\t# managed by devluma\n127.0.0.1\tnew.test\t# managed by devluma\n"
        );
    }

    #[test]
    fn similar_comments_are_not_the_tag() {
        let f = HostsFile::parse(
            "127.0.0.1 a.test # managed by devluma-old\n127.0.0.1 b.test # devluma\n127.0.0.1 c.test #laragon magic!\n",
        );
        assert!(f.managed().is_empty());
        assert_eq!(f.other_count(), 3);
    }

    #[test]
    fn a_tag_on_a_real_address_is_reported_and_never_edited() {
        let text = "10.0.0.7 nas.test # managed by devluma\n";
        let f = HostsFile::parse(text);
        assert!(f.managed().is_empty());
        assert_eq!(f.stray_tags.len(), 1);
        assert_eq!(f.stray_tags[0].line, 1);
        assert_eq!(f.with_managed(&[]).unwrap(), text);
    }

    #[test]
    fn the_old_marked_block_is_left_as_it_is() {
        let text = "# BEGIN Devluma (managed entries; edit them in Devluma)\n127.0.0.1 old.test\n# END Devluma\n";
        let f = HostsFile::parse(text);
        assert!(f.managed().is_empty());
        let out = f.with_managed(&[entry("new.test")]).unwrap();
        assert_eq!(
            out,
            format!("{text}127.0.0.1\tnew.test\t# managed by devluma\n")
        );
    }

    #[test]
    fn handles_files_without_trailing_newline() {
        let f = HostsFile::parse("127.0.0.1 localhost");
        let out = f.with_managed(&[entry("x.test")]).unwrap();
        assert_eq!(
            out,
            "127.0.0.1 localhost\n127.0.0.1\tx.test\t# managed by devluma\n"
        );
    }

    #[test]
    fn a_tagged_line_with_several_names_keeps_only_the_wanted_ones() {
        let f = HostsFile::parse("127.0.0.1 a.test b.test # managed by devluma\n");
        let out = f.with_managed(&[entry("b.test")]).unwrap();
        assert_eq!(out, "127.0.0.1\tb.test\t# managed by devluma\n");
    }

    #[test]
    fn only_loopback_and_valid_domains() {
        let f = HostsFile::parse("");
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
        let f = HostsFile::parse("10.1.2.3 shop.test\n");
        assert!(matches!(
            f.with_managed(&[entry("shop.test")]),
            Err(HostsError::ExternalConflict { line: 1, .. })
        ));
        // Same loopback mapping on someone else's line is fine (no conflict).
        let f = HostsFile::parse("127.0.0.1 shop.test\n");
        assert!(f.with_managed(&[entry("shop.test")]).is_ok());
    }

    #[test]
    fn diff_shows_exact_changes() {
        let f = HostsFile::parse("127.0.0.1 localhost\n");
        let out = f.with_managed(&[entry("shop.test")]).unwrap();
        let d = diff(&f.text, &out, "hosts");
        assert!(
            d.contains("+127.0.0.1\tshop.test\t# managed by devluma"),
            "{d}"
        );
        assert!(!d.contains("-127.0.0.1 localhost"));
    }
}
