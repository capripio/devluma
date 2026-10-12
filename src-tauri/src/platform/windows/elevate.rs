//! Launching the narrow privilege helper (ADR 0007). One UAC prompt per action; the helper
//! does one thing and exits. The main app itself never runs elevated.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
use windows::Win32::UI::Shell::{
    SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::PCWSTR;

pub const HELPER_EXE: &str = "devluma-elevate.exe";

#[derive(Debug, thiserror::Error)]
pub enum ElevateError {
    #[error("administrator approval was declined")]
    Declined,
    #[error("the elevation helper ({0}) is missing from this installation")]
    HelperMissing(PathBuf),
    #[error("the elevated helper did not finish in time")]
    TimedOut,
    #[error("could not start the elevated helper: {0}")]
    Failed(String),
}

/// The helper ships next to the main executable.
pub fn helper_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(|d| d.join(HELPER_EXE))
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Quotes one argument for the Windows command line (paths and hex hashes only).
fn quote(arg: &str) -> String {
    format!("\"{}\"", arg.replace('"', ""))
}

/// Runs `helper` elevated with `args` and returns its exit code.
pub fn run_elevated(
    helper: &Path,
    args: &[String],
    timeout: Duration,
) -> Result<u32, ElevateError> {
    if !helper.is_file() {
        return Err(ElevateError::HelperMissing(helper.to_path_buf()));
    }
    let file = wide(helper.as_os_str());
    let verb = wide(OsStr::new("runas"));
    let params = wide(OsStr::new(
        &args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" "),
    ));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: all strings are NUL-terminated and outlive the call; `info` is fully initialized.
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        return Err(if e.code() == ERROR_CANCELLED.to_hresult() {
            ElevateError::Declined
        } else {
            ElevateError::Failed(e.message())
        });
    }
    let process = info.hProcess;
    if process.is_invalid() {
        return Err(ElevateError::Failed("no process handle".into()));
    }
    // SAFETY: valid process handle owned by us until CloseHandle below.
    unsafe {
        let waited = WaitForSingleObject(
            process,
            timeout.as_millis().min(u32::MAX as u128 - 1) as u32,
        );
        let mut code = 0u32;
        let got = GetExitCodeProcess(process, &mut code);
        let _ = CloseHandle(process);
        if waited != WAIT_OBJECT_0 {
            return Err(ElevateError::TimedOut);
        }
        got.map_err(|e| ElevateError::Failed(e.message()))?;
        Ok(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_helper_is_reported_without_prompting() {
        let err = run_elevated(
            Path::new(r"C:\nowhere\devluma-elevate.exe"),
            &[],
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(matches!(err, ElevateError::HelperMissing(_)));
    }

    #[test]
    fn arguments_cannot_break_out_of_quotes() {
        assert_eq!(quote(r#"C:\a b\x" --evil"#), r#""C:\a b\x --evil""#);
    }
}
