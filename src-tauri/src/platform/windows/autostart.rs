//! "Start with Windows": one value under the current user's `Run` key. The registry is the
//! source of truth (the user can also remove it in Task Manager), so nothing is cached.
//! Per-user only; no elevation.

use std::path::Path;

use serde::Serialize;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "Devluma";
/// Passed by the `Run` entry so the app knows it was started at sign-in.
pub const AUTOSTART_ARG: &str = "--autostart";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchAtLogin {
    Off,
    On,
    /// The entry starts a different devluma.exe (e.g. a portable copy that was moved).
    OtherCopy,
}

pub struct RunKey {
    path: String,
}

impl RunKey {
    pub fn current_user() -> Self {
        Self {
            path: RUN_KEY.to_owned(),
        }
    }

    #[cfg(test)]
    fn at(path: &str) -> Self {
        Self {
            path: path.to_owned(),
        }
    }

    pub fn status(&self, exe: &Path) -> LaunchAtLogin {
        let Ok(value) = windows_registry::CURRENT_USER
            .open(&self.path)
            .and_then(|k| k.get_string(VALUE_NAME))
        else {
            return LaunchAtLogin::Off;
        };
        match registered_exe(&value) {
            Some(p) if p.eq_ignore_ascii_case(&exe.to_string_lossy()) => LaunchAtLogin::On,
            _ => LaunchAtLogin::OtherCopy,
        }
    }

    pub fn enable(&self, exe: &Path) -> std::io::Result<()> {
        let exe = exe.to_string_lossy();
        if exe.contains('"') {
            return Err(std::io::Error::other(
                "unexpected quote in the executable path",
            ));
        }
        windows_registry::CURRENT_USER
            .create(&self.path)
            .and_then(|k| k.set_string(VALUE_NAME, format!("\"{exe}\" {AUTOSTART_ARG}")))
            .map_err(|e| std::io::Error::other(e.message()))
    }

    /// Removes the entry; already absent is fine.
    pub fn disable(&self) -> std::io::Result<()> {
        let Ok(key) = windows_registry::CURRENT_USER.create(&self.path) else {
            return Ok(());
        };
        match key.remove_value(VALUE_NAME) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND.to_hresult() => {
                Ok(())
            }
            Err(e) => Err(std::io::Error::other(e.message())),
        }
    }
}

/// The executable of a `Run` command line: the quoted part, or everything before a space.
fn registered_exe(command: &str) -> Option<&str> {
    let c = command.trim();
    if let Some(rest) = c.strip_prefix('"') {
        rest.split('"').next()
    } else {
        c.split(' ').next()
    }
    .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempKey(String);
    impl Drop for TempKey {
        fn drop(&mut self) {
            let _ = windows_registry::CURRENT_USER.remove_tree(&self.0);
        }
    }

    #[test]
    fn enable_status_disable_round_trip_in_a_private_key() {
        let key = TempKey(format!(r"Software\Devluma-test-{}", ulid::Ulid::generate()));
        let run = RunKey::at(&key.0);
        let exe = Path::new(r"C:\Tools\Devluma\devluma.exe");
        assert_eq!(run.status(exe), LaunchAtLogin::Off);
        run.disable().unwrap();

        run.enable(exe).unwrap();
        assert_eq!(run.status(exe), LaunchAtLogin::On);
        assert_eq!(
            run.status(Path::new(r"c:\tools\devluma\DEVLUMA.EXE")),
            LaunchAtLogin::On
        );
        assert_eq!(
            run.status(Path::new(r"D:\Other\devluma.exe")),
            LaunchAtLogin::OtherCopy
        );
        let stored = windows_registry::CURRENT_USER
            .open(&key.0)
            .unwrap()
            .get_string(VALUE_NAME)
            .unwrap();
        assert_eq!(stored, r#""C:\Tools\Devluma\devluma.exe" --autostart"#);

        run.disable().unwrap();
        assert_eq!(run.status(exe), LaunchAtLogin::Off);
    }

    #[test]
    fn parses_registered_command_lines() {
        assert_eq!(
            registered_exe(r#""C:\a b\x.exe" --y"#),
            Some(r"C:\a b\x.exe")
        );
        assert_eq!(registered_exe(r"C:\x.exe --y"), Some(r"C:\x.exe"));
        assert_eq!(registered_exe("  "), None);
    }
}
