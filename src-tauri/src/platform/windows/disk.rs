use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::core::PCWSTR;

/// Bytes available to the current user on the volume containing `path` (which must exist).
pub fn available_bytes(path: &Path) -> std::io::Result<u64> {
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut available = 0u64;
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call; the out
    // pointer refers to a live u64; the other outputs are optional and passed as None.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut available), None, None) }
        .map_err(|e| std::io::Error::other(e.message()))?;
    Ok(available)
}

#[cfg(test)]
mod tests {
    #[test]
    fn reports_space_for_an_existing_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(super::available_bytes(dir.path()).unwrap() > 0);
    }

    #[test]
    fn errors_for_a_missing_volume() {
        assert!(super::available_bytes(std::path::Path::new("Q:\\definitely\\missing\\")).is_err());
    }
}
