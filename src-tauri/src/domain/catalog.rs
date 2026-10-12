//! Package catalog manifest: strict schema + semantic validation (trd.md §5, ADR 0004).
//!
//! Parsing is all-or-nothing for the document shape (`deny_unknown_fields`, typed enums).
//! Semantic problems are reported per package/version: a bad entry is rejected and listed
//! as an issue, while valid entries remain usable.

use std::collections::{HashMap, HashSet};

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

use super::adapter::AdapterId;

pub const SCHEMA_VERSION: u32 = 1;
/// Largest archive Devluma will download for one package version.
pub const MAX_ARCHIVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

// ---- Wire format ---------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogDocument {
    pub schema_version: u32,
    pub catalog_version: String,
    pub packages: Vec<PackageManifest>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageManifest {
    pub id: String,
    pub display_name: String,
    pub category: Category,
    #[serde(default)]
    pub description: Option<String>,
    pub publisher: String,
    #[serde(default)]
    pub homepage_url: Option<String>,
    pub license: License,
    #[serde(default)]
    pub support_policy_url: Option<String>,
    pub adapter: String,
    /// Download hosts this package may use, including redirect targets.
    pub allowed_hosts: Vec<String>,
    pub versions: Vec<VersionManifest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Runtime,
    WebServer,
    Database,
    Cache,
    Mail,
    Tunnel,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct License {
    pub id: String,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Architecture {
    X64,
    Arm64,
    Any,
}

impl Architecture {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::X64 => "x64",
            Self::Arm64 => "arm64",
            Self::Any => "any",
        }
    }

    /// The architecture of this build of Devluma.
    pub fn host() -> Option<Self> {
        match std::env::consts::ARCH {
            "x86_64" => Some(Self::X64),
            "aarch64" => Some(Self::Arm64),
            _ => None,
        }
    }

    pub fn runs_on_host(self) -> bool {
        self == Self::Any || Some(self) == Self::host()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VersionManifest {
    pub version: String,
    pub architecture: Architecture,
    #[serde(default)]
    pub min_windows_build: Option<u32>,
    pub source: Source,
    pub integrity: Integrity,
    pub executables: Vec<ExecutableManifest>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub conflicts: Vec<String>,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub end_of_life_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Source {
    pub url: String,
    pub archive: ArchiveType,
    pub size_bytes: u64,
    /// Leading path components to drop when extracting (e.g. a top-level `nginx-1.27.0/`).
    #[serde(default)]
    pub strip_components: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveType {
    Zip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntegrityPolicy {
    #[serde(rename = "sha256-required")]
    Sha256Required,
    #[serde(rename = "sha256+signature")]
    Sha256AndSignature,
}

impl IntegrityPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sha256Required => "sha256-required",
            Self::Sha256AndSignature => "sha256+signature",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Integrity {
    pub policy: IntegrityPolicy,
    pub sha256: String,
    #[serde(default)]
    pub signature: Option<SignatureSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignatureSpec {
    pub kind: SignatureKind,
    /// Expected signer subject (Authenticode) or key fingerprint (GPG).
    pub signer: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureKind {
    Authenticode,
    Gpg,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutableManifest {
    /// Relative to the install root, e.g. `php.exe` or `bin/node.exe`.
    pub path: String,
    /// The version the adapter's probe must report for the install to be accepted.
    pub expected_version: String,
}

// ---- Validated model -------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Catalog {
    pub catalog_version: String,
    pub packages: Vec<CatalogPackage>,
    pub issues: Vec<CatalogIssue>,
}

#[derive(Debug, Clone)]
pub struct CatalogPackage {
    pub manifest: PackageManifest,
    pub adapter: AdapterId,
    /// Only versions that passed validation.
    pub versions: Vec<VersionManifest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogIssue {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub message: String,
}

impl Catalog {
    pub fn empty() -> Self {
        Self {
            catalog_version: "none".into(),
            packages: Vec::new(),
            issues: Vec::new(),
        }
    }

    pub fn package(&self, id: &str) -> Option<&CatalogPackage> {
        self.packages.iter().find(|p| p.manifest.id == id)
    }

    pub fn find_version(
        &self,
        package_id: &str,
        version: &str,
        arch: Architecture,
    ) -> Option<(&CatalogPackage, &VersionManifest)> {
        let pkg = self.package(package_id)?;
        let v = pkg
            .versions
            .iter()
            .find(|v| v.version == version && v.architecture == arch)?;
        Some((pkg, v))
    }
}

/// Stable DB id for a catalog version row (`package_versions.id`).
pub fn version_row_id(package_id: &str, version: &str, arch: Architecture) -> String {
    format!("{package_id}@{version}@{}", arch.as_str())
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ValidationOptions {
    /// Tests only: allow `http://127.0.0.1` sources served by an in-process server.
    pub allow_loopback_http: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("catalog is not valid JSON for schema {SCHEMA_VERSION}: {0}")]
    Malformed(#[from] serde_json::Error),
    #[error("unsupported catalog schema version {0}")]
    UnsupportedSchema(u32),
}

static PACKAGE_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9-]{0,39}$").unwrap());
static VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+(\.\d+){1,3}(-[0-9A-Za-z.]+)?$").unwrap());
static SHA256: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{64}$").unwrap());
static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{4}-\d{2}-\d{2}$").unwrap());
static HOSTNAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$").unwrap()
});

pub fn parse_and_validate(json: &str, opts: ValidationOptions) -> Result<Catalog, CatalogError> {
    let doc: CatalogDocument = serde_json::from_str(json)?;
    if doc.schema_version != SCHEMA_VERSION {
        return Err(CatalogError::UnsupportedSchema(doc.schema_version));
    }
    Ok(validate(doc, opts))
}

fn validate(doc: CatalogDocument, opts: ValidationOptions) -> Catalog {
    let mut issues = Vec::new();
    let mut seen_ids = HashSet::new();
    let mut accepted: Vec<CatalogPackage> = Vec::new();

    for pkg in doc.packages {
        let pid = pkg.id.clone();
        let issue = |msg: String| CatalogIssue {
            package_id: Some(pid.clone()),
            version: None,
            message: msg,
        };
        if !PACKAGE_ID.is_match(&pkg.id) {
            issues.push(issue("Invalid package id.".into()));
            continue;
        }
        if !seen_ids.insert(pkg.id.clone()) {
            issues.push(issue("Duplicate package id.".into()));
            continue;
        }
        let Some(adapter) = AdapterId::parse(&pkg.adapter) else {
            issues.push(issue(format!(
                "Unknown adapter \"{}\"; package unavailable.",
                pkg.adapter
            )));
            continue;
        };
        if pkg.display_name.trim().is_empty()
            || pkg.publisher.trim().is_empty()
            || pkg.license.id.trim().is_empty()
        {
            issues.push(issue(
                "Display name, publisher and license are required.".into(),
            ));
            continue;
        }
        if pkg.allowed_hosts.is_empty()
            || !pkg
                .allowed_hosts
                .iter()
                .all(|h| HOSTNAME.is_match(h) || (opts.allow_loopback_http && h == "127.0.0.1"))
        {
            issues.push(issue("allowedHosts must list lowercase host names.".into()));
            continue;
        }

        let mut versions = Vec::new();
        let mut seen_versions = HashSet::new();
        for v in &pkg.versions {
            match validate_version(&pkg, v, opts) {
                Ok(()) if !seen_versions.insert((v.version.clone(), v.architecture)) => issues
                    .push(CatalogIssue {
                        package_id: Some(pkg.id.clone()),
                        version: Some(v.version.clone()),
                        message: "Duplicate version/architecture.".into(),
                    }),
                Ok(()) => versions.push(v.clone()),
                Err(message) => issues.push(CatalogIssue {
                    package_id: Some(pkg.id.clone()),
                    version: Some(v.version.clone()),
                    message,
                }),
            }
        }
        accepted.push(CatalogPackage {
            manifest: pkg,
            adapter,
            versions,
        });
    }

    // References must resolve to accepted packages; dependency graph must be acyclic.
    let known: HashSet<String> = accepted.iter().map(|p| p.manifest.id.clone()).collect();
    for pkg in &mut accepted {
        let id = pkg.manifest.id.clone();
        pkg.versions.retain(|v| {
            let missing: Vec<_> = v
                .dependencies
                .iter()
                .chain(&v.conflicts)
                .filter(|d| !known.contains(*d) || **d == id)
                .collect();
            if missing.is_empty() {
                return true;
            }
            issues.push(CatalogIssue {
                package_id: Some(id.clone()),
                version: Some(v.version.clone()),
                message: format!("Unknown or self-referencing dependency/conflict: {missing:?}."),
            });
            false
        });
    }
    for cyclic in dependency_cycles(&accepted) {
        issues.push(CatalogIssue {
            package_id: Some(cyclic.clone()),
            version: None,
            message: "Package is part of a dependency cycle; unavailable.".into(),
        });
        accepted.retain(|p| p.manifest.id != cyclic);
    }

    Catalog {
        catalog_version: doc.catalog_version,
        packages: accepted,
        issues,
    }
}

fn validate_version(
    pkg: &PackageManifest,
    v: &VersionManifest,
    opts: ValidationOptions,
) -> Result<(), String> {
    if !VERSION.is_match(&v.version) {
        return Err("Invalid version string.".into());
    }
    let url =
        url::Url::parse(&v.source.url).map_err(|_| "Source URL is not a valid URL.".to_string())?;
    let loopback_http =
        opts.allow_loopback_http && url.scheme() == "http" && url.host_str() == Some("127.0.0.1");
    if url.scheme() != "https" && !loopback_http {
        return Err("Source URL must use HTTPS.".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Source URL must not contain credentials.".into());
    }
    let host = url.host_str().unwrap_or_default();
    if !pkg.allowed_hosts.iter().any(|h| h == host) {
        return Err(format!("Source host \"{host}\" is not in allowedHosts."));
    }
    if v.source.size_bytes == 0 || v.source.size_bytes > MAX_ARCHIVE_BYTES {
        return Err("sizeBytes must be between 1 byte and 2 GiB.".into());
    }
    if v.source.strip_components > 2 {
        return Err("stripComponents must be 0, 1 or 2.".into());
    }
    if !SHA256.is_match(&v.integrity.sha256) {
        return Err("sha256 must be 64 lowercase hex characters.".into());
    }
    match (v.integrity.policy, &v.integrity.signature) {
        (IntegrityPolicy::Sha256AndSignature, None) => {
            return Err("Policy sha256+signature requires a signature spec.".into());
        }
        (IntegrityPolicy::Sha256Required, Some(_)) => {
            return Err("A signature spec requires policy sha256+signature.".into());
        }
        _ => {}
    }
    if v.executables.is_empty() {
        return Err("At least one executable must be listed for verification.".into());
    }
    for exe in &v.executables {
        validate_relative_exe(&exe.path)?;
        if exe.expected_version.trim().is_empty() {
            return Err("Executable expectedVersion is required.".into());
        }
    }
    for date in [&v.release_date, &v.end_of_life_date].into_iter().flatten() {
        if !DATE.is_match(date) {
            return Err("Dates must be YYYY-MM-DD.".into());
        }
    }
    Ok(())
}

/// Executable paths are data from the catalog: keep them strictly inside the install root.
fn validate_relative_exe(path: &str) -> Result<(), String> {
    let normalized = path.replace('\\', "/");
    let bad = normalized.is_empty()
        || normalized.starts_with('/')
        || normalized.contains(':')
        || normalized
            .split('/')
            .any(|seg| seg.is_empty() || seg == "." || seg == "..")
        || !normalized.to_ascii_lowercase().ends_with(".exe");
    if bad {
        return Err(format!(
            "Executable path \"{path}\" must be a relative .exe path inside the package."
        ));
    }
    Ok(())
}

/// Returns ids of packages that participate in a dependency cycle.
fn dependency_cycles(packages: &[CatalogPackage]) -> Vec<String> {
    let edges: HashMap<&str, HashSet<&str>> = packages
        .iter()
        .map(|p| {
            let deps = p
                .versions
                .iter()
                .flat_map(|v| v.dependencies.iter().map(String::as_str))
                .collect();
            (p.manifest.id.as_str(), deps)
        })
        .collect();

    fn reaches<'a>(
        from: &'a str,
        target: &str,
        edges: &HashMap<&'a str, HashSet<&'a str>>,
        seen: &mut HashSet<&'a str>,
    ) -> bool {
        for &next in edges.get(from).into_iter().flatten() {
            if next == target {
                return true;
            }
            if seen.insert(next) && reaches(next, target, edges, seen) {
                return true;
            }
        }
        false
    }

    let mut cyclic: Vec<String> = edges
        .keys()
        .filter(|id| reaches(id, id, &edges, &mut HashSet::new()))
        .map(|id| id.to_string())
        .collect();
    cyclic.sort();
    cyclic
}

#[cfg(test)]
pub(crate) mod fixtures {
    use serde_json::{Value, json};

    pub const FIXTURE_SHA: &str =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    pub fn version(v: &str) -> Value {
        json!({
            "version": v,
            "architecture": "x64",
            "source": { "url": format!("https://downloads.example.org/tool-{v}.zip"), "archive": "zip", "sizeBytes": 1024 },
            "integrity": { "policy": "sha256-required", "sha256": FIXTURE_SHA },
            "executables": [{ "path": "bin/tool.exe", "expectedVersion": v }]
        })
    }

    pub fn package(id: &str, versions: Vec<Value>) -> Value {
        json!({
            "id": id,
            "displayName": id.to_uppercase(),
            "category": "tool",
            "publisher": "Fixture Inc.",
            "license": { "id": "MIT" },
            "adapter": "fixture",
            "allowedHosts": ["downloads.example.org"],
            "versions": versions
        })
    }

    pub fn document(packages: Vec<Value>) -> String {
        json!({ "schemaVersion": 1, "catalogVersion": "test-1", "packages": packages }).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use serde_json::json;

    fn parse(doc: String) -> Catalog {
        parse_and_validate(&doc, ValidationOptions::default()).unwrap()
    }

    fn only_issue(c: &Catalog) -> &str {
        assert_eq!(c.issues.len(), 1, "issues: {:?}", c.issues);
        &c.issues[0].message
    }

    #[test]
    fn accepts_a_valid_catalog() {
        let c = parse(document(vec![package(
            "tool",
            vec![version("1.2.3"), version("1.3.0")],
        )]));
        assert!(c.issues.is_empty(), "{:?}", c.issues);
        assert_eq!(c.packages[0].versions.len(), 2);
        assert!(c.find_version("tool", "1.3.0", Architecture::X64).is_some());
    }

    #[test]
    fn unknown_fields_and_bad_shapes_reject_the_whole_document() {
        let mut doc: serde_json::Value =
            serde_json::from_str(&document(vec![package("tool", vec![version("1.0.0")])])).unwrap();
        doc["packages"][0]["installScript"] = json!("rm -rf /");
        assert!(matches!(
            parse_and_validate(&doc.to_string(), ValidationOptions::default()),
            Err(CatalogError::Malformed(_))
        ));
        assert!(parse_and_validate("{", ValidationOptions::default()).is_err());
        assert!(matches!(
            parse_and_validate(
                r#"{"schemaVersion":2,"catalogVersion":"x","packages":[]}"#,
                ValidationOptions::default()
            ),
            Err(CatalogError::UnsupportedSchema(2))
        ));
    }

    #[test]
    fn unknown_adapter_fails_closed() {
        let mut p = package("tool", vec![version("1.0.0")]);
        p["adapter"] = json!("run-anything");
        let c = parse(document(vec![p]));
        assert!(c.packages.is_empty());
        assert!(only_issue(&c).contains("Unknown adapter"));
    }

    #[test]
    fn bad_versions_are_rejected_individually() {
        let mut http = version("1.0.1");
        http["source"]["url"] = json!("http://downloads.example.org/x.zip");
        let mut other_host = version("1.0.2");
        other_host["source"]["url"] = json!("https://evil.example.net/x.zip");
        let mut creds = version("1.0.3");
        creds["source"]["url"] = json!("https://u:p@downloads.example.org/x.zip");
        let mut short_hash = version("1.0.4");
        short_hash["integrity"]["sha256"] = json!("abc");
        let mut traversal = version("1.0.5");
        traversal["executables"][0]["path"] = json!("../../Windows/System32/cmd.exe");
        let mut absolute = version("1.0.6");
        absolute["executables"][0]["path"] = json!("C:\\Windows\\notepad.exe");
        let mut huge = version("1.0.7");
        huge["source"]["sizeBytes"] = json!(MAX_ARCHIVE_BYTES + 1);
        let mut sig_missing = version("1.0.8");
        sig_missing["integrity"]["policy"] = json!("sha256+signature");
        let mut no_exe = version("1.0.9");
        no_exe["executables"] = json!([]);

        let c = parse(document(vec![package(
            "tool",
            vec![
                version("1.0.0"),
                http,
                other_host,
                creds,
                short_hash,
                traversal,
                absolute,
                huge,
                sig_missing,
                no_exe,
            ],
        )]));
        assert_eq!(
            c.packages[0].versions.len(),
            1,
            "only the valid version survives"
        );
        assert_eq!(c.issues.len(), 9, "{:?}", c.issues);
    }

    #[test]
    fn integrity_policy_none_is_not_a_thing() {
        let mut v = version("1.0.0");
        v["integrity"]["policy"] = json!("none");
        assert!(
            parse_and_validate(
                &document(vec![package("tool", vec![v])]),
                ValidationOptions::default()
            )
            .is_err()
        );
    }

    #[test]
    fn dependencies_must_exist_and_be_acyclic() {
        let mut a = version("1.0.0");
        a["dependencies"] = json!(["b"]);
        let mut b = version("1.0.0");
        b["dependencies"] = json!(["a"]);
        let mut c_dep = version("1.0.0");
        c_dep["dependencies"] = json!(["missing"]);
        let cat = parse(document(vec![
            package("a", vec![a]),
            package("b", vec![b]),
            package("c", vec![c_dep]),
            package("d", vec![version("2.0.0")]),
        ]));
        let ids: Vec<_> = cat
            .packages
            .iter()
            .map(|p| p.manifest.id.as_str())
            .collect();
        assert_eq!(ids, vec!["c", "d"]);
        assert!(
            cat.packages[0].versions.is_empty(),
            "c's only version has a missing dependency"
        );
        assert_eq!(cat.issues.len(), 3);
    }

    #[test]
    fn duplicates_are_rejected() {
        let c = parse(document(vec![
            package("tool", vec![version("1.0.0"), version("1.0.0")]),
            package("tool", vec![version("2.0.0")]),
        ]));
        assert_eq!(c.packages.len(), 1);
        assert_eq!(c.packages[0].versions.len(), 1);
        assert_eq!(c.issues.len(), 2);
    }

    #[test]
    fn loopback_http_only_with_test_option() {
        let mut p = package("tool", vec![]);
        p["allowedHosts"] = json!(["127.0.0.1"]);
        let mut v = version("1.0.0");
        v["source"]["url"] = json!("http://127.0.0.1:8080/tool.zip");
        p["versions"] = json!([v]);
        let doc = document(vec![p]);
        assert!(parse(doc.clone()).packages[0].versions.is_empty());
        let c = parse_and_validate(
            &doc,
            ValidationOptions {
                allow_loopback_http: true,
            },
        )
        .unwrap();
        assert_eq!(c.packages[0].versions.len(), 1);
    }
}
