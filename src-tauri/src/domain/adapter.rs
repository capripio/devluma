//! Compiled allowlist of package adapters (ADR 0004). A catalog manifest names an adapter
//! by id; anything not listed here makes the package unavailable (fail closed).
//!
//! Adapters own every *behaviour* — e.g. which arguments verify an executable's version —
//! so catalog data never decides what gets executed.

use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdapterId {
    Php,
    Nginx,
    Mariadb,
    Node,
    Python,
    Mailpit,
    Postgresql,
    Mongodb,
    Apache,
    /// Caddy web server (ADR 0019).
    Caddy,
    /// FrankenPHP: Caddy with PHP built in. Never downloaded: the user's own
    /// frankenphp.exe is registered (ADR 0019, `services::frankenphp`).
    Frankenphp,
    /// Cmder console (a terminal, not a runtime or service; ADR 0015).
    Cmder,
    /// Composer (`composer.phar`, run with the project's PHP; ADR 0020).
    Composer,
    /// WordPress release, copied into new projects by the app installer (ADR 0020).
    Wordpress,
    /// Deterministic test package; never accepted outside `cfg(test)`.
    #[cfg(test)]
    Fixture,
}

/// How to ask an executable for its version, and how to read the answer.
pub struct VersionProbe {
    pub args: &'static [&'static str],
    /// First capture group is the version string.
    pub pattern: &'static str,
}

/// What kind of long-running service a package provides, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceRole {
    WebServer,
    Database,
    /// PHP via `php-cgi -b`, started on demand for sites (ADR 0009).
    FastCgi,
    /// Local mail catcher (SMTP + web UI).
    Mail,
}

impl AdapterId {
    pub fn parse(id: &str) -> Option<Self> {
        match id {
            "php" => Some(Self::Php),
            "nginx" => Some(Self::Nginx),
            "mariadb" => Some(Self::Mariadb),
            "node" => Some(Self::Node),
            "python" => Some(Self::Python),
            "mailpit" => Some(Self::Mailpit),
            "postgresql" => Some(Self::Postgresql),
            "mongodb" => Some(Self::Mongodb),
            "apache" => Some(Self::Apache),
            "caddy" => Some(Self::Caddy),
            "frankenphp" => Some(Self::Frankenphp),
            "cmder" => Some(Self::Cmder),
            "composer" => Some(Self::Composer),
            "wordpress" => Some(Self::Wordpress),
            #[cfg(test)]
            "fixture" => Some(Self::Fixture),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Php => "php",
            Self::Nginx => "nginx",
            Self::Mariadb => "mariadb",
            Self::Node => "node",
            Self::Python => "python",
            Self::Mailpit => "mailpit",
            Self::Postgresql => "postgresql",
            Self::Mongodb => "mongodb",
            Self::Apache => "apache",
            Self::Caddy => "caddy",
            Self::Frankenphp => "frankenphp",
            Self::Cmder => "cmder",
            Self::Composer => "composer",
            Self::Wordpress => "wordpress",
            #[cfg(test)]
            Self::Fixture => "fixture",
        }
    }

    pub fn version_probe(self) -> VersionProbe {
        match self {
            // `-n`: ignore any php.ini so a broken config can't affect verification.
            Self::Php => VersionProbe {
                args: &["-n", "-v"],
                pattern: r"^PHP (\d+\.\d+\.\d+)",
            },
            // Printed on stderr: "nginx version: nginx/1.30.5".
            Self::Nginx => VersionProbe {
                args: &["-v"],
                pattern: r"^nginx version: nginx/(\d+\.\d+\.\d+)",
            },
            // Server: "... Ver 11.8.9-MariaDB for Win64"; client: "... from 11.8.9-MariaDB, client".
            Self::Mariadb => VersionProbe {
                args: &["--version"],
                pattern: r"(?:Ver| from) (\d+\.\d+\.\d+)-MariaDB",
            },
            // "v24.21.0".
            Self::Node => VersionProbe {
                args: &["-v"],
                pattern: r"^v(\d+\.\d+\.\d+)\r?$",
            },
            // `-I`: isolated mode, so PYTHON* variables and user site-packages can't interfere.
            Self::Python => VersionProbe {
                args: &["-I", "-V"],
                pattern: r"^Python (\d+\.\d+\.\d+)\r?$",
            },
            // "<path>\mailpit.exe v1.31.5 compiled with go1.27.2 on windows/amd64".
            Self::Mailpit => VersionProbe {
                args: &["version"],
                pattern: r" v(\d+\.\d+\.\d+) compiled with",
            },
            // "postgres (PostgreSQL) 18.6".
            Self::Postgresql => VersionProbe {
                args: &["--version"],
                pattern: r"^postgres \(PostgreSQL\) (\d+\.\d+)\r?$",
            },
            // "db version v9.0.2" (first line of several).
            Self::Mongodb => VersionProbe {
                args: &["--version"],
                pattern: r"^db version v(\d+\.\d+\.\d+)\r?$",
            },
            // "Server version: Apache/2.4.69 (Win64)".
            Self::Apache => VersionProbe {
                args: &["-v"],
                pattern: r"^Server version: Apache/(\d+\.\d+\.\d+) ",
            },
            // "v2.11.9 h1:bhZt…=": the caddy module's version and sum from the build info
            // (read at review with `go version -m caddy.exe`).
            Self::Caddy => VersionProbe {
                args: &["version"],
                pattern: r"^v(\d+\.\d+\.\d+) h1:",
            },
            // "FrankenPHP 1.13.5 PHP 8.5.12 Caddy v2.11.9 h1:…" (verified with v1.13.5).
            Self::Frankenphp => VersionProbe {
                args: &["version"],
                pattern: r"^FrankenPHP v?(\d+\.\d+\.\d+) PHP ",
            },
            // Cmder.exe is a GUI launcher: never run it to probe. Its version resource is
            // read instead (see `reads_version_resource`), e.g. "FileVersion 1.3.25.328".
            Self::Cmder => VersionProbe {
                args: &[],
                pattern: r"^FileVersion (\d+\.\d+\.\d+)\.\d+$",
            },
            // Read from the file, never run: `const VERSION = '2.11.2';` in the phar's
            // Composer.php (stored uncompressed; checked with 2.11.2).
            Self::Composer => VersionProbe {
                args: &[],
                pattern: r"const VERSION = '(\d+\.\d+\.\d+)';",
            },
            // `$wp_version = '7.1.4';` in wp-includes/version.php ("7.2" for an x.y release).
            Self::Wordpress => VersionProbe {
                args: &[],
                pattern: r"^\$wp_version = '(\d+\.\d+(?:\.\d+)?)';\r?$",
            },
            #[cfg(test)]
            Self::Fixture => VersionProbe {
                args: &["--version"],
                pattern: r"^fixture-tool (\S+)",
            },
        }
    }

    pub fn service_role(self) -> Option<ServiceRole> {
        match self {
            Self::Nginx | Self::Apache | Self::Caddy | Self::Frankenphp => {
                Some(ServiceRole::WebServer)
            }
            Self::Mariadb => Some(ServiceRole::Database),
            Self::Php => Some(ServiceRole::FastCgi),
            Self::Mailpit => Some(ServiceRole::Mail),
            Self::Postgresql | Self::Mongodb => Some(ServiceRole::Database),
            Self::Node | Self::Python | Self::Cmder | Self::Composer | Self::Wordpress => None,
            #[cfg(test)]
            Self::Fixture => Some(ServiceRole::WebServer),
        }
    }

    /// System DLLs (in `%SystemRoot%\System32`) the package's executables need, with the
    /// product that provides them. Checked before installing.
    pub fn required_system_dlls(self) -> &'static [(&'static str, &'static str)] {
        const VC_REDIST: &str = "Microsoft Visual C++ 2015–2022 Redistributable (x64)";
        match self {
            // FrankenPHP's Windows build bundles the official (VS17) PHP.
            Self::Php | Self::Apache | Self::Frankenphp => &[
                ("vcruntime140.dll", VC_REDIST),
                ("vcruntime140_1.dll", VC_REDIST),
            ],
            // Python's package ships its own vcruntime DLLs; Node.js and Caddy (Go, built
            // with CGO_ENABLED=0) need none.
            Self::Nginx
            | Self::Caddy
            | Self::Mariadb
            | Self::Node
            | Self::Python
            | Self::Mailpit
            | Self::Mongodb
            | Self::Cmder
            | Self::Composer
            | Self::Wordpress => &[],
            // EDB's PostgreSQL builds link against the VC++ runtime but don't ship it.
            Self::Postgresql => &[("vcruntime140.dll", VC_REDIST), ("msvcp140.dll", VC_REDIST)],
            #[cfg(test)]
            Self::Fixture => &[],
        }
    }

    /// Folders (relative to the install root) a project terminal puts first on PATH.
    pub fn path_dirs(self) -> &'static [&'static str] {
        match self {
            // The NuGet package keeps the interpreter in `tools/`; pip puts commands in
            // `tools/Scripts` once something is installed.
            Self::Python => &["tools", r"tools\Scripts"],
            // A terminal program, not a tool to run from one; WordPress is copied, not run.
            Self::Cmder | Self::Wordpress => &[],
            _ => &[""],
        }
    }

    /// Tools Devluma never downloads: the user's own copy is registered instead, and the
    /// catalog may not offer them (ADR 0019).
    pub fn user_provided(self) -> bool {
        matches!(self, Self::Frankenphp)
    }

    /// True when the version comes from the executable's version resource rather than its
    /// output (the probe then reports `FileVersion <a.b.c.d>`).
    pub fn reads_version_resource(self) -> bool {
        matches!(self, Self::Cmder)
    }

    /// True when the version is read from the listed file's text (PHP code Devluma never
    /// runs to verify it). The probe output is then the file's text.
    pub fn reads_file_text(self) -> bool {
        matches!(self, Self::Composer | Self::Wordpress)
    }

    /// Extracts the version from probe output, if it matches this adapter's format.
    pub fn parse_version(self, output: &str) -> Option<String> {
        let re = Regex::new(&format!("(?m){}", self.version_probe().pattern)).ok()?;
        re.captures(output).map(|c| c[1].to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_adapters_are_rejected() {
        assert_eq!(AdapterId::parse("php"), Some(AdapterId::Php));
        assert_eq!(AdapterId::parse("PHP"), None);
        assert_eq!(AdapterId::parse("shell"), None);
    }

    #[test]
    fn cmder_reads_its_version_resource() {
        assert!(AdapterId::Cmder.reads_version_resource());
        assert!(!AdapterId::Php.reads_version_resource());
        assert_eq!(
            AdapterId::Cmder
                .parse_version("FileVersion 1.3.25.328")
                .as_deref(),
            Some("1.3.25")
        );
        assert_eq!(AdapterId::Cmder.parse_version("1.3.25.328"), None);
        assert_eq!(AdapterId::Cmder.service_role(), None);
        assert!(AdapterId::Cmder.path_dirs().is_empty());
    }

    #[test]
    fn php_version_parsing() {
        let out = "PHP 8.4.1 (cli) (built: Nov 20 2024 11:13:42) (NTS Visual C++ 2022 x64)\nCopyright (c) The PHP Group";
        assert_eq!(AdapterId::Php.parse_version(out).as_deref(), Some("8.4.1"));
        assert_eq!(AdapterId::Php.parse_version("Hello"), None);
    }

    /// Outputs captured from the real binaries during catalog review.
    #[test]
    fn nginx_and_mariadb_version_parsing() {
        assert_eq!(
            AdapterId::Nginx
                .parse_version("\nnginx version: nginx/1.30.5\n")
                .as_deref(),
            Some("1.30.5")
        );
        let server = r"C:\Devluma\packages\mariadb\11.8.9-x64\bin\mariadbd.exe  Ver 11.8.9-MariaDB for Win64 on AMD64 (MariaDB Server)";
        let client = r"C:\Devluma\packages\mariadb\11.8.9-x64\bin\mariadb.exe from 11.8.9-MariaDB, client 15.2 for Win64 (AMD64), source revision bf91";
        assert_eq!(
            AdapterId::Mariadb.parse_version(server).as_deref(),
            Some("11.8.9")
        );
        assert_eq!(
            AdapterId::Mariadb.parse_version(client).as_deref(),
            Some("11.8.9")
        );
        assert_eq!(
            AdapterId::Mariadb.parse_version(r"C:\packages\mariadb\11.8.9-x64\bin\x.exe"),
            None,
            "the version in the install path must not be mistaken for the reported one"
        );
    }

    /// Windows consoles end lines with CRLF; the real `node -v` output is "v24.21.0\r\n".
    #[test]
    fn node_and_python_version_parsing() {
        assert_eq!(
            AdapterId::Node.parse_version("v24.21.0\r\n").as_deref(),
            Some("24.21.0")
        );
        assert_eq!(AdapterId::Node.parse_version("v24.21.0-nightly\r\n"), None);
        assert_eq!(
            AdapterId::Python
                .parse_version("Python 3.14.8\r\n")
                .as_deref(),
            Some("3.14.8")
        );
        assert_eq!(AdapterId::Python.parse_version("Python 3.15.0rc1\n"), None);
    }

    /// Text from the real files (Composer 2.11.2's phar, WordPress 7.1.4's version.php).
    #[test]
    fn composer_and_wordpress_versions_are_read_from_their_files() {
        let phar = "class Composer\n{\n    public const VERSION = '2.11.2';\n    public const BRANCH_ALIAS_VERSION = '';";
        assert_eq!(
            AdapterId::Composer.parse_version(phar).as_deref(),
            Some("2.11.2")
        );
        assert_eq!(
            AdapterId::Composer.parse_version("const VERSION = '@package_version@';"),
            None
        );
        let wp = "<?php\r\n$wp_version = '7.1.4';\r\n$wp_db_version = 60717;\r\n";
        assert_eq!(
            AdapterId::Wordpress.parse_version(wp).as_deref(),
            Some("7.1.4")
        );
        assert_eq!(
            AdapterId::Wordpress
                .parse_version("$wp_version = '7.2';\n")
                .as_deref(),
            Some("7.2")
        );
        assert_eq!(
            AdapterId::Wordpress.parse_version("$wp_version = '7.2-beta1';"),
            None
        );
        for a in [AdapterId::Composer, AdapterId::Wordpress] {
            assert!(a.reads_file_text() && a.service_role().is_none());
            assert_eq!(AdapterId::parse(a.as_str()), Some(a));
        }
        assert!(AdapterId::Wordpress.path_dirs().is_empty());
        assert_eq!(AdapterId::Composer.path_dirs(), &[""]);
    }

    #[test]
    fn caddy_version_parsing() {
        let out = "v2.11.9 h1:bhZtcRKQOq0fuLvEgCdPYvUxXyGn9kzjG4lh2332DTE=\r\n";
        assert_eq!(
            AdapterId::Caddy.parse_version(out).as_deref(),
            Some("2.11.9")
        );
        assert_eq!(
            AdapterId::Caddy.parse_version("v2.12.0-beta.1 h1:x=\n"),
            None
        );
        assert_eq!(AdapterId::parse("caddy"), Some(AdapterId::Caddy));
        let franken = "FrankenPHP 1.13.5 PHP 8.5.12 Caddy v2.11.9 h1:bhZtcRKQOq0fuLvEgCdPYvUxXyGn9kzjG4lh2332DTE=\r\n";
        assert_eq!(
            AdapterId::Frankenphp.parse_version(franken).as_deref(),
            Some("1.13.5")
        );
        assert!(AdapterId::Frankenphp.user_provided() && !AdapterId::Caddy.user_provided());
        assert_eq!(
            AdapterId::Caddy.service_role(),
            Some(ServiceRole::WebServer)
        );
    }
}
