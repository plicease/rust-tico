//! Native filesystem-change notification, used by the on-disk-change
//! checker (`fileio::check_external_change`) instead of its own periodic
//! `stat()` polling when the current platform supports it: inotify
//! (via `poll(2)`, so a background thread can still shut down cleanly) on
//! Linux, kqueue on macOS/BSD, `ReadDirectoryChangesW` (via overlapped I/O
//! and `WaitForMultipleObjects`, for the same clean-shutdown reason) on
//! Windows. Anywhere else, `FileWatcher::new` returns `None` and the
//! caller falls back to the existing periodic-poll behavior unchanged.
//!
//! Every implementation watches the file's *parent directory* rather than
//! the file's own inode/handle: that is what makes it notice not only
//! in-place edits but also the common "safe save" pattern (write to a
//! temp file, then rename over the original) that many editors and tools
//! use, which replaces the inode entirely.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct FileWatcher {
    changed: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    /// Write end of a self-pipe the background thread also polls
    /// alongside its real watch fd(s), so `Drop` can wake it immediately
    /// instead of waiting out its poll/kevent timeout — that wait (up to
    /// 1s) used to make switching buffers, and exiting, visibly pause.
    #[cfg(unix)]
    wake_write_fd: std::os::raw::c_int,
    /// Windows equivalent of `wake_write_fd`: a manual-reset event the
    /// background thread also waits on (alongside the overlapped I/O
    /// completion event) via `WaitForMultipleObjects`, so `Drop` can wake
    /// it immediately by signaling it instead of waiting for a change.
    #[cfg(windows)]
    wake_event: platform::WakeHandle,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl FileWatcher {
    /// Start watching `path` in the background. `None` means this
    /// platform (or this particular path) isn't supported — the caller
    /// should fall back to its own periodic check instead.
    pub fn new(path: &Path) -> Option<FileWatcher> {
        platform::start(path)
    }

    /// Reports, and clears, whether a change was flagged since the last
    /// call. Never blocks.
    pub fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::SeqCst)
    }
}

impl Drop for FileWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        #[cfg(unix)]
        unsafe {
            let byte = [1u8];
            libc::write(self.wake_write_fd, byte.as_ptr() as *const libc::c_void, 1);
            libc::close(self.wake_write_fd);
        }
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::Threading::SetEvent(self.wake_event.0);
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        // The background thread only ever *waits* on wake_event; it never
        // closes it (unlike its own dir/IO handles), so it's still valid
        // here and Drop -- the sole owner -- is the right place to do it,
        // now that `join` above guarantees the thread is done with it.
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.wake_event.0);
        }
    }
}

/// A connected pair of fds where writing to `.1` makes `.0` readable —
/// used purely to wake a blocked `poll`/`kevent` call promptly. `None`
/// only on a `pipe(2)` failure (an exhausted fd table, essentially).
#[cfg(unix)]
fn wake_pipe() -> Option<(std::os::raw::c_int, std::os::raw::c_int)> {
    let mut fds = [0 as std::os::raw::c_int; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        None
    } else {
        Some((fds[0], fds[1]))
    }
}

/// Resolve the directory a watch should be placed on for `path`: its
/// parent, or "." when `path` is a bare filename in the working directory.
fn watch_dir(path: &Path) -> std::path::PathBuf {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{FileWatcher, wake_pipe, watch_dir};
    use std::ffi::{CString, OsStr};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    pub fn start(path: &Path) -> Option<FileWatcher> {
        let dir = watch_dir(path);
        let filename = path.file_name()?.to_owned();
        let dir_c = CString::new(dir.as_os_str().as_bytes()).ok()?;

        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return None;
        }
        let mask = libc::IN_MODIFY
            | libc::IN_ATTRIB
            | libc::IN_CLOSE_WRITE
            | libc::IN_CREATE
            | libc::IN_DELETE
            | libc::IN_MOVED_FROM
            | libc::IN_MOVED_TO
            | libc::IN_MOVE_SELF
            | libc::IN_DELETE_SELF;
        let wd = unsafe { libc::inotify_add_watch(fd, dir_c.as_ptr(), mask) };
        if wd < 0 {
            unsafe { libc::close(fd) };
            return None;
        }
        let Some((wake_read, wake_write)) = wake_pipe() else {
            unsafe { libc::close(fd) };
            return None;
        };

        let changed = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let changed2 = changed.clone();
        let stop2 = stop.clone();

        let handle = std::thread::spawn(move || {
            // A generous fixed buffer: inotify events are small, and even
            // a burst of them (e.g. a tool rewriting many files in this
            // directory at once) only needs to be *noticed*, not fully
            // drained in one read -- the next poll() picks up the rest.
            let mut buf = [0u8; 4096];
            loop {
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                let mut pfds = [
                    libc::pollfd {
                        fd,
                        events: libc::POLLIN,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: wake_read,
                        events: libc::POLLIN,
                        revents: 0,
                    },
                ];
                // A 1s timeout as a fallback safety net; the wake pipe is
                // what actually makes this thread notice `stop` promptly.
                let rc = unsafe { libc::poll(pfds.as_mut_ptr(), 2, 1000) };
                if rc <= 0 {
                    continue;
                }
                if pfds[1].revents & libc::POLLIN != 0 {
                    break; // woken explicitly for shutdown
                }
                if pfds[0].revents & libc::POLLIN == 0 {
                    continue;
                }
                let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
                if n <= 0 {
                    continue;
                }
                let n = n as usize;
                let mut offset = 0usize;
                while offset + std::mem::size_of::<libc::inotify_event>() <= n {
                    let event =
                        unsafe { &*(buf.as_ptr().add(offset) as *const libc::inotify_event) };
                    let name_len = event.len as usize;
                    let name_start = offset + std::mem::size_of::<libc::inotify_event>();
                    if name_len > 0 && name_start + name_len <= n {
                        let raw = &buf[name_start..name_start + name_len];
                        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
                        if OsStr::from_bytes(&raw[..end]) == filename.as_os_str() {
                            changed2.store(true, Ordering::SeqCst);
                        }
                    }
                    offset = name_start + name_len;
                }
            }
            unsafe {
                libc::close(fd);
                libc::close(wake_read);
            }
        });

        Some(FileWatcher {
            changed,
            stop,
            wake_write_fd: wake_write,
            handle: Some(handle),
        })
    }
}

#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]
mod platform {
    use super::{FileWatcher, wake_pipe, watch_dir};
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::RawFd;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn open_readonly(path: &Path) -> Option<RawFd> {
        let c = CString::new(path.as_os_str().as_bytes()).ok()?;
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
        if fd < 0 { None } else { Some(fd) }
    }

    pub fn start(path: &Path) -> Option<FileWatcher> {
        let dir_fd = open_readonly(&watch_dir(path))?;
        // The file may not exist at watch-start time (e.g. it was deleted
        // out from under us); the directory watch alone still catches it
        // reappearing, so this is optional.
        let file_fd = open_readonly(path);

        let kq = unsafe { libc::kqueue() };
        if kq < 0 {
            unsafe {
                libc::close(dir_fd);
                if let Some(f) = file_fd {
                    libc::close(f);
                }
            }
            return None;
        }
        let Some((wake_read, wake_write)) = wake_pipe() else {
            unsafe {
                libc::close(kq);
                libc::close(dir_fd);
                if let Some(f) = file_fd {
                    libc::close(f);
                }
            }
            return None;
        };

        let vnode_flags = libc::NOTE_WRITE
            | libc::NOTE_DELETE
            | libc::NOTE_RENAME
            | libc::NOTE_EXTEND
            | libc::NOTE_ATTRIB;
        let mut changelist = vec![
            libc::kevent {
                ident: dir_fd as libc::uintptr_t,
                filter: libc::EVFILT_VNODE,
                flags: libc::EV_ADD | libc::EV_CLEAR,
                fflags: vnode_flags,
                data: 0,
                udata: std::ptr::null_mut(),
            },
            libc::kevent {
                ident: wake_read as libc::uintptr_t,
                filter: libc::EVFILT_READ,
                flags: libc::EV_ADD,
                fflags: 0,
                data: 0,
                udata: std::ptr::null_mut(),
            },
        ];
        if let Some(f) = file_fd {
            changelist.push(libc::kevent {
                ident: f as libc::uintptr_t,
                filter: libc::EVFILT_VNODE,
                flags: libc::EV_ADD | libc::EV_CLEAR,
                fflags: vnode_flags,
                data: 0,
                udata: std::ptr::null_mut(),
            });
        }
        let registered = unsafe {
            libc::kevent(
                kq,
                changelist.as_ptr(),
                changelist.len() as i32,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        if registered < 0 {
            unsafe {
                libc::close(kq);
                libc::close(dir_fd);
                libc::close(wake_read);
                libc::close(wake_write);
                if let Some(f) = file_fd {
                    libc::close(f);
                }
            }
            return None;
        }

        let changed = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let changed2 = changed.clone();
        let stop2 = stop.clone();

        let handle = std::thread::spawn(move || {
            loop {
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                let mut events: [libc::kevent; 4] = unsafe { std::mem::zeroed() };
                // A 1s timeout as a fallback safety net; the wake pipe is
                // what actually makes this thread notice `stop` promptly.
                let timeout = libc::timespec {
                    tv_sec: 1,
                    tv_nsec: 0,
                };
                let n = unsafe {
                    libc::kevent(
                        kq,
                        std::ptr::null(),
                        0,
                        events.as_mut_ptr(),
                        events.len() as i32,
                        &timeout,
                    )
                };
                if n <= 0 {
                    continue;
                }
                let woken = events[..n as usize].iter().any(|e| {
                    e.filter == libc::EVFILT_READ && e.ident == wake_read as libc::uintptr_t
                });
                if woken {
                    break; // woken explicitly for shutdown
                }
                changed2.store(true, Ordering::SeqCst);
            }
            unsafe {
                libc::close(kq);
                libc::close(dir_fd);
                libc::close(wake_read);
                if let Some(f) = file_fd {
                    libc::close(f);
                }
            }
        });

        Some(FileWatcher {
            changed,
            stop,
            wake_write_fd: wake_write,
            handle: Some(handle),
        })
    }
}

#[cfg(windows)]
mod platform {
    use super::{FileWatcher, watch_dir};
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::Path;
    use std::ptr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
        FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_CREATION, FILE_NOTIFY_CHANGE_DIR_NAME,
        FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
        FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING, ReadDirectoryChangesW,
    };
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, WaitForMultipleObjects};

    /// A `HANDLE` (an opaque OS handle, not memory this process manages)
    /// that needs to cross the thread boundary -- unlike a general raw
    /// pointer, moving or sharing one carries no aliasing risk.
    pub struct WakeHandle(pub(super) HANDLE);
    unsafe impl Send for WakeHandle {}
    struct SendHandle(HANDLE);
    unsafe impl Send for SendHandle {}
    impl SendHandle {
        // A method taking `self` by value, rather than reading the `.0`
        // field directly, forces a spawned closure to capture the whole
        // (`Send`) wrapper instead of disjointly capturing just its
        // not-`Send` `HANDLE` field.
        fn into_inner(self) -> HANDLE {
            self.0
        }
    }

    const WATCH_MASK: u32 = FILE_NOTIFY_CHANGE_FILE_NAME
        | FILE_NOTIFY_CHANGE_DIR_NAME
        | FILE_NOTIFY_CHANGE_ATTRIBUTES
        | FILE_NOTIFY_CHANGE_SIZE
        | FILE_NOTIFY_CHANGE_LAST_WRITE
        | FILE_NOTIFY_CHANGE_CREATION;

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    }

    /// NTFS/ReFS names are case-insensitive-but-preserving, so this can't
    /// just be a byte-for-byte match the way the inotify/kqueue sides
    /// compare names.
    fn names_match(reported: &std::ffi::OsStr, filename: &std::ffi::OsStr) -> bool {
        reported
            .to_string_lossy()
            .eq_ignore_ascii_case(&filename.to_string_lossy())
    }

    pub fn start(path: &Path) -> Option<FileWatcher> {
        let dir = watch_dir(path);
        let filename = path.file_name()?.to_owned();
        let dir_wide = wide(&dir);

        let dir_handle = unsafe {
            CreateFileW(
                dir_wide.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                ptr::null_mut(),
            )
        };
        if dir_handle == INVALID_HANDLE_VALUE {
            return None;
        }
        let io_event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if io_event.is_null() {
            unsafe { CloseHandle(dir_handle) };
            return None;
        }
        let wake_event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if wake_event.is_null() {
            unsafe {
                CloseHandle(io_event);
                CloseHandle(dir_handle);
            }
            return None;
        }

        let changed = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let changed2 = changed.clone();
        let stop2 = stop.clone();
        let dir_handle_s = SendHandle(dir_handle);
        let io_event_s = SendHandle(io_event);
        let wake_event_s = SendHandle(wake_event);

        let handle = std::thread::spawn(move || {
            let dir_handle = dir_handle_s.into_inner();
            let io_event = io_event_s.into_inner();
            let wake_event = wake_event_s.into_inner();
            // A generous fixed buffer, matching the inotify side: even a
            // burst of changes only needs to be *noticed*, not fully
            // drained in one read -- ReadDirectoryChangesW is re-issued
            // right after.
            let mut buf = [0u8; 4096];
            loop {
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
                overlapped.hEvent = io_event;
                let mut bytes_returned = 0u32;
                let issued = unsafe {
                    ReadDirectoryChangesW(
                        dir_handle,
                        buf.as_mut_ptr() as *mut _,
                        buf.len() as u32,
                        0, // bWatchSubtree = FALSE: this dir only, like the other platforms
                        WATCH_MASK,
                        &mut bytes_returned,
                        &mut overlapped,
                        None,
                    )
                };
                if issued == 0 {
                    break; // e.g. the watched directory itself was removed
                }

                let handles = [io_event, wake_event];
                let wait = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
                if wait != WAIT_OBJECT_0 {
                    // Either woken for shutdown (index 1) or a wait
                    // failure -- either way, cancel the pending read and
                    // wait for that cancellation to land before touching
                    // the handles again.
                    unsafe {
                        CancelIoEx(dir_handle, &overlapped);
                        let mut discard = 0u32;
                        GetOverlappedResult(dir_handle, &overlapped, &mut discard, 1);
                    }
                    break;
                }

                let mut transferred = 0u32;
                let ok =
                    unsafe { GetOverlappedResult(dir_handle, &overlapped, &mut transferred, 0) };
                if ok == 0 || transferred == 0 {
                    // 0 bytes transferred (including a change-buffer
                    // overflow, when too many changes land between reads)
                    // means we can't tell what changed -- report a change
                    // conservatively rather than risk missing one.
                    changed2.store(true, Ordering::SeqCst);
                    continue;
                }

                let mut offset = 0usize;
                loop {
                    let info = unsafe {
                        &*(buf.as_ptr().add(offset) as *const FILE_NOTIFY_INFORMATION)
                    };
                    let name_offset =
                        offset + std::mem::offset_of!(FILE_NOTIFY_INFORMATION, FileName);
                    let name_len = info.FileNameLength as usize / 2;
                    let name = unsafe {
                        std::slice::from_raw_parts(buf.as_ptr().add(name_offset) as *const u16, name_len)
                    };
                    if names_match(&OsString::from_wide(name), &filename) {
                        changed2.store(true, Ordering::SeqCst);
                    }
                    if info.NextEntryOffset == 0 {
                        break;
                    }
                    offset += info.NextEntryOffset as usize;
                }
            }
            unsafe {
                CloseHandle(io_event);
                CloseHandle(dir_handle);
            }
        });

        Some(FileWatcher {
            changed,
            stop,
            wake_event: WakeHandle(wake_event),
            handle: Some(handle),
        })
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
    windows
)))]
mod platform {
    use super::FileWatcher;
    use std::path::Path;

    pub fn start(_path: &Path) -> Option<FileWatcher> {
        None
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos", windows)))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn test_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tico_watch_test_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Poll `take_changed()` for up to a few seconds -- the background
    /// thread needs a moment to notice and report an OS event.
    fn wait_for_change(watcher: &FileWatcher) -> bool {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if watcher.take_changed() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn detects_an_in_place_modification() {
        let dir = test_dir("in_place");
        let path = dir.join("watched.txt");
        std::fs::write(&path, "one").unwrap();

        let watcher = FileWatcher::new(&path).expect("watcher should start on this platform");
        std::thread::sleep(Duration::from_millis(50)); // let the watch register first
        std::fs::write(&path, "two").unwrap();

        assert!(
            wait_for_change(&watcher),
            "expected the watcher to flag the in-place write"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn detects_an_atomic_rename_replace_save() {
        let dir = test_dir("rename_replace");
        let path = dir.join("watched.txt");
        std::fs::write(&path, "one").unwrap();

        let watcher = FileWatcher::new(&path).expect("watcher should start on this platform");
        std::thread::sleep(Duration::from_millis(50));

        let tmp = dir.join("watched.txt.tmp");
        std::fs::write(&tmp, "two").unwrap();
        std::fs::rename(&tmp, &path).unwrap();

        assert!(
            wait_for_change(&watcher),
            "expected the watcher to flag the rename-over-original save"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn take_changed_stays_false_with_no_activity() {
        let dir = test_dir("quiet");
        let path = dir.join("watched.txt");
        std::fs::write(&path, "one").unwrap();

        let watcher = FileWatcher::new(&path).expect("watcher should start on this platform");
        std::thread::sleep(Duration::from_millis(200));
        assert!(!watcher.take_changed());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn drop_does_not_wait_out_the_poll_timeout() {
        // Regression test: Drop used to just set the stop flag and join
        // the background thread, which only checks that flag after its
        // poll()/kevent() call returns -- up to the full 1s timeout later
        // if nothing else happened. That made switching buffers, and
        // exiting, visibly pause. The wake pipe should make Drop return
        // in well under that.
        let dir = test_dir("drop_timing");
        let path = dir.join("watched.txt");
        std::fs::write(&path, "one").unwrap();

        let watcher = FileWatcher::new(&path).expect("watcher should start on this platform");
        std::thread::sleep(Duration::from_millis(50)); // let the watch register first

        let started = Instant::now();
        drop(watcher);
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_millis(300),
            "dropping the watcher took {elapsed:?}, expected well under the 1s poll timeout"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
