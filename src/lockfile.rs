//! Vim-style lock files, matching nano's `locking` option (`-G`/`--locking`)
//! byte-for-byte (see `~/dev/nano`'s `src/files.c`: `write_lockfile`,
//! `do_lockfile`, `delete_lockfile`), so that nano and vim can detect and
//! warn about tico's locks and vice versa.
//!
//! Unix-only: the format encodes a PID, username and hostname the way
//! `getpid()`/`getpwuid()`/`gethostname()` do, which don't have a portable
//! non-Unix equivalent wired up here. On other targets these functions are
//! no-ops, so `locking` simply has no effect there yet.

use std::path::{Path, PathBuf};

const LOCK_SIZE: usize = 1024;

/// The lock-file path for `target`: `.basename.swp` next to it (nano's
/// `locking_prefix`/`locking_suffix`).
pub fn lock_path(target: &Path) -> PathBuf {
    let dir = target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let name = target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    dir.join(format!(".{name}.swp"))
}

#[derive(Debug, Clone)]
pub struct LockInfo {
    pub program: String,
    pub pid: u32,
    pub user: String,
}

pub enum LockCheck {
    /// No lock file present.
    None,
    /// A lock file is present but too short or missing nano/vim's magic
    /// bytes; nano ignores (and will overwrite) such a file.
    Bad,
    /// A valid, presumably-active lock.
    Held(LockInfo),
}

/// Inspect an existing lock file, if any, without creating or modifying it.
pub fn check_lock(path: &Path) -> LockCheck {
    let Ok(data) = std::fs::read(path) else { return LockCheck::None };
    if data.len() < 68 || data[0] != 0x62 || data[1] != 0x30 {
        return LockCheck::Bad;
    }
    let program = bytes_to_string(&data[2..12]);
    let pid = u32::from_le_bytes([data[24], data[25], data[26], data[27]]);
    let user = bytes_to_string(&data[28..44]);
    LockCheck::Held(LockInfo { program, pid, user })
}

fn bytes_to_string(field: &[u8]) -> String {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

/// Write (creating or overwriting) a lock file at `lock_path`, recording
/// that `target_filename` is open in this process, with nano's exact
/// binary layout:
///
/// ```text
///   bytes 0-1     magic 0x62 0x30
///   bytes 2-11    program name ("tico VERSION")
///   bytes 24-27   PID, little-endian
///   bytes 28-43   username
///   bytes 68-99   hostname
///   bytes 108-875 target filename
///   byte  1007    0x55 if modified, else 0x00
/// ```
/// All other bytes are zero.
#[cfg(unix)]
pub fn write_lock(lock_path: &Path, target_filename: &str, modified: bool) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut data = vec![0u8; LOCK_SIZE];
    data[0] = 0x62;
    data[1] = 0x30;

    write_field(&mut data, 2, 10, &format!("tico {}", env!("CARGO_PKG_VERSION")));

    let pid = std::process::id();
    data[24..28].copy_from_slice(&pid.to_le_bytes());

    write_field(&mut data, 28, 16, &current_username());
    write_field(&mut data, 68, 32, &current_hostname());
    write_field(&mut data, 108, 768, target_filename);

    data[1007] = if modified { 0x55 } else { 0x00 };

    // nano always removes an existing lock first, then creates with
    // O_EXCL, rather than truncating in place.
    let _ = std::fs::remove_file(lock_path);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o666).open(lock_path)?;
    f.write_all(&data)
}

#[cfg(not(unix))]
pub fn write_lock(_lock_path: &Path, _target_filename: &str, _modified: bool) -> std::io::Result<()> {
    Ok(())
}

fn write_field(buf: &mut [u8], offset: usize, max_len: usize, s: &str) {
    let bytes = s.as_bytes();
    let n = bytes.len().min(max_len);
    buf[offset..offset + n].copy_from_slice(&bytes[..n]);
}

#[cfg(unix)]
fn current_username() -> String {
    std::env::var("USER").or_else(|_| std::env::var("LOGNAME")).unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(unix)]
fn current_hostname() -> String {
    rustix::system::uname().nodename().to_string_lossy().into_owned()
}

/// Delete a lock file, ignoring errors (including "it's already gone").
pub fn delete_lock(lock_path: &Path) {
    let _ = std::fs::remove_file(lock_path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_path_matches_nano_convention() {
        assert_eq!(lock_path(Path::new("/tmp/foo.txt")), PathBuf::from("/tmp/.foo.txt.swp"));
        assert_eq!(lock_path(Path::new("bar.rs")), PathBuf::from("./.bar.rs.swp"));
    }

    #[test]
    #[cfg(unix)]
    fn write_then_check_roundtrip() {
        let dir = std::env::temp_dir().join(format!("tico-lock-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("somefile.txt");
        let lp = lock_path(&target);

        write_lock(&lp, target.to_str().unwrap(), false).unwrap();
        match check_lock(&lp) {
            LockCheck::Held(info) => {
                assert!(info.program.starts_with("tico "));
                assert_eq!(info.pid, std::process::id());
                assert!(!info.user.is_empty());
            }
            _ => panic!("expected a held lock"),
        }

        delete_lock(&lp);
        assert!(matches!(check_lock(&lp), LockCheck::None));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
