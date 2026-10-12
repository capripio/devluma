//! OS-specific code. Everything that calls Windows APIs lives under `windows/`.

#[cfg(windows)]
pub mod windows;
