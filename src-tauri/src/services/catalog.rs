//! Loads the package catalog. The bundled catalog ships inside the app (works offline,
//! ADR 0004). Remote, signed catalog updates are a later feature.
//!
//! Debug builds only: `DEVLUMA_CATALOG=<path>` loads a local catalog file instead, for
//! manual testing. It goes through exactly the same validation (HTTPS, allowlists, hashes).

use crate::domain::catalog::{self, Catalog, CatalogIssue, ValidationOptions};

const BUNDLED: &str = include_str!("../../catalog/catalog.json");

pub fn load() -> Catalog {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("DEVLUMA_CATALOG") {
        return match std::fs::read_to_string(&path) {
            Ok(json) => parse_or_issue(&json),
            Err(e) => failed(format!("Could not read DEVLUMA_CATALOG: {e}")),
        };
    }
    parse_or_issue(BUNDLED)
}

fn parse_or_issue(json: &str) -> Catalog {
    match catalog::parse_and_validate(json, ValidationOptions::default()) {
        Ok(c) => c,
        Err(e) => failed(format!("The package catalog was rejected: {e}")),
    }
}

/// An unusable catalog yields no packages plus an explanation, never a crash.
fn failed(message: String) -> Catalog {
    let mut c = Catalog::empty();
    c.issues.push(CatalogIssue {
        package_id: None,
        version: None,
        message,
    });
    c
}

#[cfg(test)]
mod tests {
    #[test]
    fn bundled_catalog_is_valid() {
        let c = super::parse_or_issue(super::BUNDLED);
        assert!(c.issues.is_empty(), "{:?}", c.issues);
    }

    #[test]
    fn malformed_catalog_becomes_an_issue() {
        let c = super::parse_or_issue("{ nope");
        assert!(c.packages.is_empty());
        assert_eq!(c.issues.len(), 1);
    }
}
