//! Reading an executable's fixed file version from its version resource, without running it.
//! Used for GUI programs (Cmder) whose version can't be asked on the command line.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
};
use windows::core::{PCWSTR, w};

/// `major.minor.build.revision` from the file's `VS_FIXEDFILEINFO`.
pub fn file_version(path: &Path) -> std::io::Result<String> {
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is NUL-terminated and outlives every call; `data` is sized by the API.
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(wide.as_ptr()), None);
        if size == 0 {
            return Err(std::io::Error::other("the file has no version resource"));
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(wide.as_ptr()), None, size, data.as_mut_ptr().cast())
            .map_err(|e| std::io::Error::other(e.message()))?;
        let mut info: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let ok = VerQueryValueW(data.as_ptr().cast(), w!("\\"), &mut info, &mut len);
        if !ok.as_bool() || info.is_null() || (len as usize) < size_of::<VS_FIXEDFILEINFO>() {
            return Err(std::io::Error::other(
                "the version resource has no file version",
            ));
        }
        let fixed = &*(info as *const VS_FIXEDFILEINFO);
        Ok(format!(
            "{}.{}.{}.{}",
            fixed.dwFileVersionMS >> 16,
            fixed.dwFileVersionMS & 0xffff,
            fixed.dwFileVersionLS >> 16,
            fixed.dwFileVersionLS & 0xffff
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_system_executable_and_rejects_files_without_one() {
        let root = std::env::var_os("SystemRoot").unwrap();
        let notepad = Path::new(&root).join("System32").join("notepad.exe");
        let v = file_version(&notepad).unwrap();
        assert_eq!(v.split('.').count(), 4, "{v}");
        assert!(v.starts_with("10."), "{v}");

        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("plain.exe");
        std::fs::write(&plain, b"not a PE file").unwrap();
        assert!(file_version(&plain).is_err());
    }
}
