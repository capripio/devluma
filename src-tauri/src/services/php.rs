//! PHP-specific inspection and php.ini handling.
//!
//! PHP's built-in default `extension_dir` on Windows is `C:\php\ext` (verified), which would
//! load another PHP's DLLs — or nothing. Every php.ini Devluma writes therefore sets an
//! absolute `extension_dir` pointing into its own installation, and is validated by PHP
//! itself (`php -c <ini> -m` must start without startup warnings) before it's put in place.
//! An existing php.ini is never overwritten here.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::install::ExecutableProbe;

pub const INI_FILE: &str = "php.ini";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IniTemplate {
    Development,
    Production,
}

impl IniTemplate {
    fn file_name(self) -> &'static str {
        match self {
            Self::Development => "php.ini-development",
            Self::Production => "php.ini-production",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhpDetails {
    pub install_path: String,
    pub ini_path: String,
    pub ini_exists: bool,
    pub available_templates: Vec<IniTemplate>,
    pub extension_dir: String,
    /// Modules PHP reports as loaded with the current php.ini (`php -m`).
    pub loaded_modules: Vec<String>,
    /// Extension DLLs shipped in `ext/` (`php_<name>.dll` → `<name>`).
    pub available_extensions: Vec<String>,
    /// Startup warnings PHP printed (e.g. an extension that failed to load).
    pub startup_warnings: Vec<String>,
}

fn php_exe(install: &Path) -> PathBuf {
    install.join("php.exe")
}

/// `-m` lists modules; PHP finds `php.ini` next to `php.exe` (the probe clears PHPRC).
pub fn inspect(probe: &dyn ExecutableProbe, install: &Path) -> Result<PhpDetails, String> {
    let ini = install.join(INI_FILE);
    let output = probe.run(&php_exe(install), &["-m"])?;
    let (loaded_modules, startup_warnings) = parse_modules(&output);

    let ext_dir = install.join("ext");
    let mut available_extensions: Vec<String> = fs::read_dir(&ext_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_lowercase();
                    name.strip_prefix("php_")
                        .and_then(|n| n.strip_suffix(".dll"))
                        .map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default();
    available_extensions.sort();

    Ok(PhpDetails {
        install_path: install.display().to_string(),
        ini_path: ini.display().to_string(),
        ini_exists: ini.is_file(),
        available_templates: [IniTemplate::Development, IniTemplate::Production]
            .into_iter()
            .filter(|t| install.join(t.file_name()).is_file())
            .collect(),
        extension_dir: ext_dir.display().to_string(),
        loaded_modules,
        available_extensions,
        startup_warnings,
    })
}

fn parse_modules(output: &str) -> (Vec<String>, Vec<String>) {
    let mut modules = Vec::new();
    let mut warnings = Vec::new();
    for line in output.lines().map(str::trim) {
        if line.contains("PHP Startup:")
            || line.starts_with("PHP Warning")
            || line.starts_with("Warning:")
        {
            // PHP prints each startup warning twice ("PHP Warning:  X" on stderr and
            // "Warning: X" on stdout); keep the message after the label, once.
            let message = line
                .split_once("Warning:")
                .map_or(line, |(_, rest)| rest)
                .trim()
                .to_owned();
            if !warnings.contains(&message) {
                warnings.push(message);
            }
        } else if !line.is_empty() && !line.starts_with('[') {
            modules.push(line.to_owned());
        }
    }
    modules.sort_by_key(|m| m.to_lowercase());
    modules.dedup();
    (modules, warnings)
}

/// Template text with Devluma's required overrides applied.
pub fn render_ini(template_text: &str, install: &Path) -> String {
    let ext = install.join("ext");
    format!(
        "; Created by Devluma from the official PHP template.\n\
         ; extension_dir is set to this installation's own ext folder (PHP's default is C:\\php\\ext).\n\
         extension_dir = \"{}\"\n\n{}",
        ext.display(),
        template_text
    )
}

#[derive(Debug, thiserror::Error)]
pub enum IniError {
    #[error("php.ini already exists; Devluma won't overwrite it.")]
    AlreadyExists,
    #[error("This PHP installation doesn't include the {0} template.")]
    TemplateMissing(&'static str),
    #[error("PHP rejected the new configuration: {0}")]
    Invalid(String),
    #[error("Could not write php.ini: {0}")]
    Io(#[from] std::io::Error),
}

/// Creates `php.ini` from a template: write to a temp file, validate it with PHP itself,
/// then move it into place. Never replaces an existing php.ini.
pub fn create_ini(
    probe: &dyn ExecutableProbe,
    install: &Path,
    template: IniTemplate,
) -> Result<PathBuf, IniError> {
    let target = install.join(INI_FILE);
    if target.exists() {
        return Err(IniError::AlreadyExists);
    }
    let template_path = install.join(template.file_name());
    let text = fs::read_to_string(&template_path)
        .map_err(|_| IniError::TemplateMissing(template.file_name()))?;

    let tmp = install.join(format!("{INI_FILE}.devluma-new"));
    let _ = fs::remove_file(&tmp);
    fs::write(&tmp, render_ini(&text, install))?;

    let tmp_str = tmp.to_string_lossy().into_owned();
    let validation = probe
        .run(&php_exe(install), &["-c", &tmp_str, "-m"])
        .map_err(IniError::Invalid);
    let result = validation.and_then(|out| {
        let (_, warnings) = parse_modules(&out);
        if warnings.is_empty() {
            Ok(())
        } else {
            Err(IniError::Invalid(warnings.join(" ")))
        }
    });
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    // No-replace rename: fails if php.ini appeared meanwhile.
    if target.exists() {
        let _ = fs::remove_file(&tmp);
        return Err(IniError::AlreadyExists);
    }
    fs::rename(&tmp, &target)?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Returns canned `php -m` output; records the args it was called with.
    struct FakePhp {
        output: String,
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl ExecutableProbe for FakePhp {
        fn run(&self, _exe: &Path, args: &[&str]) -> Result<String, String> {
            self.calls
                .lock()
                .unwrap()
                .push(args.iter().map(|s| s.to_string()).collect());
            Ok(self.output.clone())
        }
    }

    fn fake(output: &str) -> FakePhp {
        FakePhp {
            output: output.into(),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn install() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("ext")).unwrap();
        fs::write(dir.path().join("ext").join("php_curl.dll"), b"").unwrap();
        fs::write(dir.path().join("ext").join("php_mbstring.dll"), b"").unwrap();
        fs::write(
            dir.path().join("php.ini-development"),
            "display_errors = On\n;extension=curl\n",
        )
        .unwrap();
        dir
    }

    const MODULES: &str = "[PHP Modules]\nCore\ndate\nhash\n\n[Zend Modules]\n\n";

    #[test]
    fn inspects_modules_extensions_and_ini_state() {
        let dir = install();
        let d = inspect(&fake(MODULES), dir.path()).unwrap();
        assert_eq!(d.loaded_modules, vec!["Core", "date", "hash"]);
        assert_eq!(d.available_extensions, vec!["curl", "mbstring"]);
        assert!(!d.ini_exists);
        assert_eq!(d.available_templates, vec![IniTemplate::Development]);
        assert!(d.startup_warnings.is_empty());
    }

    #[test]
    fn reports_startup_warnings_once() {
        let out = "PHP Warning:  PHP Startup: Unable to load dynamic library 'curl'\nWarning: PHP Startup: Unable to load dynamic library 'curl'\n[PHP Modules]\nCore\n";
        let (modules, warnings) = parse_modules(out);
        assert_eq!(modules, vec!["Core"]);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    #[test]
    fn creates_ini_with_absolute_extension_dir_after_validation() {
        let dir = install();
        let php = fake(MODULES);
        let path = create_ini(&php, dir.path(), IniTemplate::Development).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains(&format!(
            "extension_dir = \"{}\"",
            dir.path().join("ext").display()
        )));
        assert!(text.contains("display_errors = On"));
        let calls = php.calls.lock().unwrap();
        assert_eq!(
            calls[0][0], "-c",
            "validated with the new file before moving it into place"
        );
        assert!(!dir.path().join("php.ini.devluma-new").exists());
    }

    #[test]
    fn never_overwrites_and_discards_invalid_configs() {
        let dir = install();
        fs::write(dir.path().join(INI_FILE), "mine").unwrap();
        assert!(matches!(
            create_ini(&fake(MODULES), dir.path(), IniTemplate::Development),
            Err(IniError::AlreadyExists)
        ));
        assert_eq!(
            fs::read_to_string(dir.path().join(INI_FILE)).unwrap(),
            "mine"
        );

        let dir = install();
        let bad = fake(
            "PHP Warning:  PHP Startup: Unable to load dynamic library 'x'\n[PHP Modules]\nCore\n",
        );
        assert!(matches!(
            create_ini(&bad, dir.path(), IniTemplate::Development),
            Err(IniError::Invalid(_))
        ));
        assert!(!dir.path().join(INI_FILE).exists());
        assert!(!dir.path().join("php.ini.devluma-new").exists());

        assert!(matches!(
            create_ini(&fake(MODULES), dir.path(), IniTemplate::Production),
            Err(IniError::TemplateMissing(_))
        ));
    }
}
