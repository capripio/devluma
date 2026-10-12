//! Safe ZIP extraction (trd.md §5 step 5). Archive contents are untrusted:
//!
//! - every entry name goes through [`paths::join_relative`] (no `..`, absolute, drive,
//!   ADS or device names);
//! - symlink entries are rejected outright;
//! - duplicate names (case-insensitive, as on NTFS) are rejected;
//! - entry count, total size and compression ratio are capped (zip bombs);
//! - files are created with `create_new`, so nothing pre-existing is overwritten;
//! - the destination must be a new, empty directory that we create.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::paths::{self, PathError};

#[derive(Debug, Clone, Copy)]
pub struct ExtractLimits {
    pub max_entries: usize,
    pub max_total_bytes: u64,
    /// Uncompressed:compressed ratio above which an entry is treated as a bomb.
    pub max_ratio: u64,
    pub strip_components: usize,
}

impl ExtractLimits {
    /// Defaults scaled to the expected archive size.
    pub fn for_archive(size_bytes: u64, strip_components: u8) -> Self {
        Self {
            max_entries: 100_000,
            max_total_bytes: (size_bytes.saturating_mul(20))
                .clamp(64 * 1024 * 1024, 8 * 1024 * 1024 * 1024),
            max_ratio: 250,
            strip_components: strip_components as usize,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("archive is not a valid zip file")]
    NotAZip,
    #[error("archive entry \"{0}\" has an unsafe path: {1}")]
    UnsafePath(String, PathError),
    #[error("archive entry \"{0}\" is a symbolic link")]
    Symlink(String),
    #[error("archive contains duplicate entry \"{0}\"")]
    Duplicate(String),
    #[error("archive exceeds extraction limits ({0})")]
    LimitExceeded(&'static str),
    #[error("archive entry \"{0}\" is encrypted")]
    Encrypted(String),
    #[error("extraction destination must not exist yet")]
    DestinationExists,
    #[error("extraction was cancelled")]
    Cancelled,
    #[error("io: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ExtractSummary {
    pub files: usize,
    pub bytes: u64,
}

/// Extracts `archive` into `dest`, which must not exist. On error, `dest` may be partially
/// populated; callers own cleanup of their staging directory.
pub fn extract_zip(
    archive: &Path,
    dest: &Path,
    limits: ExtractLimits,
    cancel: &AtomicBool,
) -> Result<ExtractSummary, ExtractError> {
    if dest.exists() {
        return Err(ExtractError::DestinationExists);
    }
    let mut zip = zip::ZipArchive::new(File::open(archive)?).map_err(|_| ExtractError::NotAZip)?;
    if zip.len() > limits.max_entries {
        return Err(ExtractError::LimitExceeded("too many entries"));
    }
    fs::create_dir_all(dest)?;
    let root = fs::canonicalize(dest)?;

    // Pass 1: validate every entry before writing anything.
    let mut seen = HashSet::new();
    let mut declared_total: u64 = 0;
    let mut plan: Vec<(usize, Option<PathBuf>, bool)> = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        let entry = zip.by_index_raw(i).map_err(|_| ExtractError::NotAZip)?;
        let name = entry
            .name()
            .map_err(|_| ExtractError::NotAZip)?
            .into_owned();
        if entry.is_symlink() {
            return Err(ExtractError::Symlink(name));
        }
        if entry.encrypted() {
            return Err(ExtractError::Encrypted(name));
        }
        let normalized = name.replace('\\', "/");
        // Checked before stripping, which would otherwise silently turn "/x" into "x".
        if normalized.starts_with('/') {
            return Err(ExtractError::UnsafePath(name, PathError::Escapes));
        }
        let stripped: Vec<&str> = normalized
            .split('/')
            .filter(|s| !s.is_empty())
            .skip(limits.strip_components)
            .collect();
        if stripped.is_empty() {
            plan.push((i, None, entry.is_dir()));
            continue; // the stripped top-level directory itself
        }
        let rel = stripped.join("/");
        let target = paths::join_relative(&root, &rel)
            .map_err(|e| ExtractError::UnsafePath(name.clone(), e))?;
        if !seen.insert(rel.to_lowercase()) {
            return Err(ExtractError::Duplicate(name));
        }
        if !entry.is_dir() {
            let size = entry.size();
            let compressed = entry.compressed_size().max(1);
            if size / compressed > limits.max_ratio && size > 1024 * 1024 {
                return Err(ExtractError::LimitExceeded("compression ratio"));
            }
            declared_total = declared_total.saturating_add(size);
            if declared_total > limits.max_total_bytes {
                return Err(ExtractError::LimitExceeded("total size"));
            }
        }
        plan.push((i, Some(target), entry.is_dir()));
    }

    // Pass 2: write. Sizes are enforced again on the actual stream, not just the header.
    let mut summary = ExtractSummary::default();
    for (i, target, is_dir) in plan {
        if cancel.load(Ordering::Relaxed) {
            return Err(ExtractError::Cancelled);
        }
        let Some(target) = target else { continue };
        if is_dir {
            create_dir_inside(&root, &target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            create_dir_inside(&root, parent)?;
        }
        let mut entry = zip.by_index(i).map_err(|_| ExtractError::NotAZip)?;
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?;
        let remaining = limits.max_total_bytes - summary.bytes;
        let written = copy_limited(&mut entry, &mut out, remaining, cancel)?;
        if written > entry.size() {
            return Err(ExtractError::LimitExceeded("entry larger than declared"));
        }
        summary.bytes += written;
        summary.files += 1;
    }
    Ok(summary)
}

fn create_dir_inside(root: &Path, dir: &Path) -> Result<(), ExtractError> {
    fs::create_dir_all(dir)?;
    // A directory we just created can't be a junction, but verify the resolved location
    // anyway so a pre-existing link in the tree can never redirect writes.
    paths::ensure_within(root, dir)
        .map_err(|e| ExtractError::UnsafePath(dir.display().to_string(), e))?;
    Ok(())
}

fn copy_limited(
    r: &mut impl Read,
    w: &mut impl Write,
    limit: u64,
    cancel: &AtomicBool,
) -> Result<u64, ExtractError> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ExtractError::Cancelled);
        }
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > limit {
            return Err(ExtractError::LimitExceeded("total size"));
        }
        w.write_all(&buf[..n])?;
    }
    w.flush()?;
    Ok(total)
}

#[cfg(test)]
pub(crate) mod test_zip {
    use std::io::Write;

    use zip::write::SimpleFileOptions;

    /// Builds a zip in memory. Entries ending in `/` are directories.
    pub fn build(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            for (name, data) in entries {
                if name.ends_with('/') {
                    w.add_directory(*name, opts).unwrap();
                } else {
                    w.start_file(*name, opts).unwrap();
                    w.write_all(data).unwrap();
                }
            }
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    pub fn build_with_symlink(name: &str, target: &str) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            w.add_symlink(name, target, SimpleFileOptions::default())
                .unwrap();
            w.finish().unwrap();
        }
        buf.into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::test_zip::*;
    use super::*;

    fn run(
        bytes: &[u8],
        strip: usize,
    ) -> (
        tempfile::TempDir,
        PathBuf,
        Result<ExtractSummary, ExtractError>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.zip");
        fs::write(&archive, bytes).unwrap();
        let dest = dir.path().join("out");
        let limits = ExtractLimits {
            strip_components: strip,
            ..ExtractLimits::for_archive(bytes.len() as u64, 0)
        };
        let res = extract_zip(&archive, &dest, limits, &AtomicBool::new(false));
        (dir, dest, res)
    }

    #[test]
    fn extracts_with_strip_components() {
        let zip = build(&[
            ("tool-1.0/", b""),
            ("tool-1.0/bin/tool.exe", b"MZ"),
            ("tool-1.0/README", b"hi"),
        ]);
        let (_d, dest, res) = run(&zip, 1);
        assert_eq!(res.unwrap(), ExtractSummary { files: 2, bytes: 4 });
        assert_eq!(fs::read(dest.join("bin").join("tool.exe")).unwrap(), b"MZ");
    }

    #[test]
    fn rejects_path_traversal_before_writing_anything() {
        let zip = build(&[("ok.txt", b"x"), ("../../evil.txt", b"x")]);
        let (dir, dest, res) = run(&zip, 0);
        assert!(matches!(res, Err(ExtractError::UnsafePath(..))), "{res:?}");
        assert!(
            !dest.join("ok.txt").exists(),
            "validation happens before any write"
        );
        assert!(!dir.path().parent().unwrap().join("evil.txt").exists());
    }

    #[test]
    fn rejects_absolute_drive_and_stream_names() {
        for name in ["/abs.txt", "C:/Windows/evil.dll", "file.txt:hidden", "CON"] {
            let zip = build(&[(name, b"x")]);
            let (_d, _dest, res) = run(&zip, 0);
            assert!(res.is_err(), "{name} should be rejected");
        }
    }

    #[test]
    fn rejects_symlinks() {
        let zip = build_with_symlink("link", "C:/Windows/System32");
        let (_d, _dest, res) = run(&zip, 0);
        assert!(matches!(res, Err(ExtractError::Symlink(_))));
    }

    #[test]
    fn rejects_case_insensitive_duplicates() {
        let zip = build(&[("bin/Tool.exe", b"a"), ("BIN/tool.EXE", b"b")]);
        let (_d, _dest, res) = run(&zip, 0);
        assert!(matches!(res, Err(ExtractError::Duplicate(_))));
    }

    #[test]
    fn rejects_compression_bombs() {
        let big = vec![0u8; 8 * 1024 * 1024];
        let zip = build(&[("zeros.bin", &big)]);
        let (_d, _dest, res) = run(&zip, 0);
        assert!(
            matches!(res, Err(ExtractError::LimitExceeded(_))),
            "{res:?}"
        );
    }

    #[test]
    fn refuses_existing_destination_and_non_zip_input() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.zip");
        fs::write(&archive, build(&[("a", b"x")])).unwrap();
        let res = extract_zip(
            &archive,
            dir.path(),
            ExtractLimits::for_archive(10, 0),
            &AtomicBool::new(false),
        );
        assert!(matches!(res, Err(ExtractError::DestinationExists)));

        let (_d, _dest, res) = run(b"definitely not a zip", 0);
        assert!(matches!(res, Err(ExtractError::NotAZip)));
    }

    #[test]
    fn honours_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("a.zip");
        fs::write(&archive, build(&[("a", b"x")])).unwrap();
        let res = extract_zip(
            &archive,
            &dir.path().join("out"),
            ExtractLimits::for_archive(10, 0),
            &AtomicBool::new(true),
        );
        assert!(matches!(res, Err(ExtractError::Cancelled)));
    }
}
