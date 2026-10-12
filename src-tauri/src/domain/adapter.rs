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
            #[cfg(test)]
            Self::Fixture => VersionProbe {
                args: &["--version"],
                pattern: r"^fixture-tool (\S+)",
            },
        }
    }

    pub fn service_role(self) -> Option<ServiceRole> {
        match self {
            Self::Nginx | Self::Apache => Some(ServiceRole::WebServer),
            Self::Mariadb => Some(ServiceRole::Database),
            Self::Php => Some(ServiceRole::FastCgi),
            Self::Mailpit => Some(ServiceRole::Mail),
            Self::Postgresql | Self::Mongodb => Some(ServiceRole::Database),
            Self::Node | Self::Python => None,
            #[cfg(test)]
            Self::Fixture => Some(ServiceRole::WebServer),
        }
    }

    /// System DLLs (in `%SystemRoot%\System32`) the package's executables need, with the
    /// product that provides them. Checked before installing.
    pub fn required_system_dlls(self) -> &'static [(&'static str, &'static str)] {
        const VC_REDIST: &str = "Microsoft Visual C++ 2015–2022 Redistributable (x64)";
        match self {
            Self::Php | Self::Apache => &[
                ("vcruntime140.dll", VC_REDIST),
                ("vcruntime140_1.dll", VC_REDIST),
            ],
            // Python's package ships its own vcruntime DLLs; Node.js needs none.
            Self::Nginx
            | Self::Mariadb
            | Self::Node
            | Self::Python
            | Self::Mailpit
            | Self::Mongodb => &[],
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
            _ => &[""],
        }
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
}
