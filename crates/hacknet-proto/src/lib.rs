//! Shared types between the HacknetOS shell, daemon and CLI.
//!
//! The layout algorithm here must stay byte-for-byte equivalent to
//! `tools/hacknet-layout.py`. `layout::tests` pins both resolutions used by CI.

pub mod layout;
pub mod protocol;
pub mod theme;
pub mod window;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const SOCKET_NAME: &str = "hacknetd.sock";

pub fn state_dir() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("HACKNET_STATE_DIR") {
        return p.into();
    }
    let base = std::env::var("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| home().join(".local/state"));
    base.join("hacknet")
}

pub fn runtime_dir() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("HACKNET_RUNTIME_DIR") {
        return p.into();
    }
    std::env::var("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
        .join("hacknet")
}

pub fn home() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/"))
}

pub fn theme_dir() -> std::path::PathBuf {
    state_dir().join("theme")
}

pub fn socket_path() -> std::path::PathBuf {
    runtime_dir().join(SOCKET_NAME)
}
