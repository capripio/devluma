//! Local site domain names: normalization and validation (schema.md §6: domains are
//! normalized to lowercase IDNA so case-only duplicates are impossible).

use serde::Serialize;

/// Top-level domains reserved for local/testing use (RFC 2606, RFC 6761, and `.internal`).
const LOCAL_TLDS: [&str; 5] = ["test", "localhost", "internal", "example", "invalid"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainCheck {
    /// Lowercase ASCII (punycode) form; what gets stored and written to configs.
    pub normalized: String,
    /// Non-blocking advice, e.g. a public TLD that will shadow a real website.
    pub warnings: Vec<String>,
}

/// Normalizes `input` to lowercase IDNA and validates it as a hostname.
pub fn check(input: &str) -> Result<DomainCheck, String> {
    let trimmed = input.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        return Err("Enter a domain, for example my-app.test.".into());
    }
    if trimmed.contains(['/', ':', '@', ' ', '*', '\\', '"', '$', ';']) {
        return Err("A domain can only contain letters, digits, hyphens and dots.".into());
    }
    let normalized = match url::Host::parse(trimmed) {
        Ok(url::Host::Domain(d)) => d,
        Ok(_) => return Err("Use a domain name, not an IP address.".into()),
        Err(_) => return Err("That isn't a valid domain name.".into()),
    };
    if normalized.len() > 253 {
        return Err("The domain is too long (253 characters at most).".into());
    }
    let labels: Vec<&str> = normalized.split('.').collect();
    if labels.len() < 2 {
        return Err("Use at least two parts, for example my-app.test.".into());
    }
    for label in &labels {
        let ok = !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !ok {
            return Err(format!("\"{label}\" isn't a valid part of a domain name."));
        }
    }

    let tld = labels.last().copied().unwrap_or_default();
    let mut warnings = Vec::new();
    if tld == "local" {
        warnings.push(".local is used by mDNS/Bonjour and can resolve slowly or inconsistently. .test is recommended.".into());
    } else if !LOCAL_TLDS.contains(&tld) {
        warnings.push(format!(
            ".{tld} isn't reserved for local use: this computer will stop reaching any real {normalized} website. .test is recommended."
        ));
    }
    Ok(DomainCheck {
        normalized,
        warnings,
    })
}

/// A domain suggestion from a project name, e.g. "My Shop" → "my-shop.test".
pub fn suggest(project_name: &str) -> String {
    let mut slug = String::new();
    for c in project_name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    format!("{}.test", if slug.is_empty() { "site" } else { slug })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_case_and_idna() {
        assert_eq!(check("My-App.TEST").unwrap().normalized, "my-app.test");
        assert_eq!(
            check("bücher.test").unwrap().normalized,
            "xn--bcher-kva.test"
        );
        assert_eq!(check("shop.test.").unwrap().normalized, "shop.test");
        assert!(check("shop.test").unwrap().warnings.is_empty());
    }

    #[test]
    fn rejects_bad_input() {
        for bad in [
            "",
            "localhost",
            "127.0.0.1",
            "a b.test",
            "x.test/evil",
            "-bad.test",
            "bad-.test",
            "a..test",
            "x.test;rm",
            "$host.test",
            "under_score.test",
        ] {
            assert!(check(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(check(&format!("{}.test", "a".repeat(64))).is_err());
    }

    #[test]
    fn warns_about_public_and_mdns_tlds() {
        assert!(
            check("google.com").unwrap().warnings[0].contains("stop reaching any real google.com")
        );
        assert!(check("printer.local").unwrap().warnings[0].contains("mDNS"));
    }

    #[test]
    fn suggests_from_project_names() {
        assert_eq!(suggest("My Shop!"), "my-shop.test");
        assert_eq!(suggest("  "), "site.test");
        assert_eq!(suggest("API v2"), "api-v2.test");
    }
}
