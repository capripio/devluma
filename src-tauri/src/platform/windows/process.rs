//! Running a known executable once and capturing its output (used for version probes).
//! Explicit path + argument array, no shell, minimal environment, no console window,
//! bounded runtime and output (trd.md §4).

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const MAX_OUTPUT: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("could not start the executable: {0}")]
    Spawn(std::io::Error),
    #[error("the executable did not finish within {0:?}")]
    TimedOut(Duration),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub fn run_captured(exe: &Path, args: &[&str], timeout: Duration) -> Result<Output, RunError> {
    run_captured_with_input(exe, args, None, timeout)
}

/// Like `run_captured`, with a file as standard input (e.g. a SQL dump for a client that
/// only reports statement errors in its exit code when reading stdin).
pub fn run_captured_with_input(
    exe: &Path,
    args: &[&str],
    input: Option<&Path>,
    timeout: Duration,
) -> Result<Output, RunError> {
    let stdin = match input {
        Some(p) => Stdio::from(std::fs::File::open(p)?),
        None => Stdio::null(),
    };
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .env_clear()
        .current_dir(exe.parent().unwrap_or(Path::new(".")))
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    // Many Windows programs fail to start without these.
    for key in ["SystemRoot", "SystemDrive", "WINDIR", "TEMP", "TMP"] {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    // A minimal system-only PATH: some tools crash without one (verified: mariadb-install-db
    // exits with an access violation when PATH is absent).
    if let Some(root) = std::env::var_os("SystemRoot") {
        let root = std::path::PathBuf::from(root);
        cmd.env(
            "PATH",
            format!("{};{}", root.join("System32").display(), root.display()),
        );
    }
    let mut child = cmd.spawn().map_err(RunError::Spawn)?;

    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let out_reader = std::thread::spawn(move || read_capped(stdout));
    let err_reader = std::thread::spawn(move || read_capped(stderr));

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            // Our own direct child, killed via its handle — never by name.
            let _ = child.kill();
            let _ = child.wait();
            return Err(RunError::TimedOut(timeout));
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    Ok(Output {
        exit_code: status.code(),
        stdout: out_reader.join().unwrap_or_default(),
        stderr: err_reader.join().unwrap_or_default(),
    })
}

const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

/// Opens an interactive Windows PowerShell window in `cwd` with `path_prefix` placed first on
/// PATH for that window only (ADR 0009: no global PATH changes). Fixed executable and
/// arguments; nothing user-supplied is put on the command line. The window belongs to the
/// user and is not tracked or stopped by Devluma.
pub fn open_terminal(cwd: &Path, path_prefix: &[String]) -> std::io::Result<()> {
    let system_root = std::env::var_os("SystemRoot")
        .ok_or_else(|| std::io::Error::other("SystemRoot is not set"))?;
    let powershell = Path::new(&system_root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    let mut path = std::ffi::OsString::new();
    for dir in path_prefix {
        path.push(dir);
        path.push(";");
    }
    if let Some(existing) = std::env::var_os("PATH") {
        path.push(existing);
    }
    Command::new(powershell)
        .args(["-NoLogo", "-NoExit"])
        .current_dir(cwd)
        .env("PATH", path)
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map(|_| ())
}

/// Opens an `http://` URL in the user's default browser. Callers construct the URL from
/// validated data (site domain + port); anything else is refused.
pub fn open_url(url: &str) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let parsed = url::Url::parse(url).map_err(|_| std::io::Error::other("invalid URL"))?;
    if parsed.scheme() != "http" || parsed.host_str().is_none() || !parsed.username().is_empty() {
        return Err(std::io::Error::other(
            "only plain http site URLs can be opened",
        ));
    }
    let w = |s: &str| -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    let (verb, target) = (w("open"), w(parsed.as_str()));
    // SAFETY: NUL-terminated strings outlive the call.
    let result = unsafe {
        ShellExecuteW(
            None,
            windows::core::PCWSTR(verb.as_ptr()),
            windows::core::PCWSTR(target.as_ptr()),
            windows::core::PCWSTR::null(),
            windows::core::PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute returns a value > 32 on success.
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "ShellExecute failed ({})",
            result.0 as isize
        )))
    }
}

/// Sets a named Windows event created by another process (e.g. `MySQLShutdown<pid>`).
/// Returns false if no such event exists.
pub fn signal_named_event(name: &str) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};
    let wide: Vec<u16> = std::ffi::OsStr::new(name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: NUL-terminated name outlives the call; the handle is closed before returning.
    unsafe {
        let Ok(h) = OpenEventW(
            EVENT_MODIFY_STATE,
            false,
            windows::core::PCWSTR(wide.as_ptr()),
        ) else {
            return false;
        };
        let ok = SetEvent(h).is_ok();
        let _ = CloseHandle(h);
        ok
    }
}

fn read_capped(mut r: impl Read) -> String {
    let mut buf = Vec::new();
    let _ = r.by_ref().take(MAX_OUTPUT as u64).read_to_end(&mut buf);
    // Drain the rest so the child never blocks on a full pipe.
    let _ = std::io::copy(&mut r, &mut std::io::sink());
    String::from_utf8_lossy(&buf).into_owned()
}

/// Opens a Devluma-managed folder in File Explorer. Callers pass fixed app paths only.
pub fn open_folder(path: &Path) -> std::io::Result<()> {
    if !path.is_dir() {
        return Err(std::io::Error::other("not a folder"));
    }
    let system_root = std::env::var_os("SystemRoot")
        .ok_or_else(|| std::io::Error::other("SystemRoot is not set"))?;
    Command::new(Path::new(&system_root).join("explorer.exe"))
        .arg(path)
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_output_and_exit_code() {
        // The test binary itself is a known executable with deterministic `--list` output.
        let me = std::env::current_exe().unwrap();
        let out = run_captured(
            &me,
            &["--list", "--format", "terse"],
            Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(out.exit_code, Some(0));
        assert!(out.stdout.contains("captures_output_and_exit_code"));
    }

    #[test]
    fn missing_executable_is_a_spawn_error() {
        let res = run_captured(
            Path::new("C:\\definitely\\missing.exe"),
            &[],
            Duration::from_secs(1),
        );
        assert!(matches!(res, Err(RunError::Spawn(_))));
    }

    #[test]
    fn times_out_and_kills_the_child() {
        let me = std::env::current_exe().unwrap();
        // Runs a test that sleeps; the probe must give up long before it finishes.
        let res = run_captured(
            &me,
            &[
                "--exact",
                "platform::windows::process::tests::sleeper",
                "--ignored",
                "--nocapture",
            ],
            Duration::from_millis(300),
        );
        assert!(matches!(res, Err(RunError::TimedOut(_))), "{res:?}");
    }

    #[test]
    #[ignore = "helper for times_out_and_kills_the_child"]
    fn sleeper() {
        std::thread::sleep(Duration::from_secs(30));
    }
}
