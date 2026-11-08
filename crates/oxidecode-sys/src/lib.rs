//! Safe, zero-cost Rust wrappers around the C runtime in `C/ocpty.c`.
//!
//! The C layer exists for the operations where Rust's standard library has no
//! stable API on Android/Termux: PTY allocation, controlling-terminal setup,
//! `TIOCSCTTY`, madvise-free literal search over `mmap`, and directory fsync.

use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io;
use std::os::raw::{c_char, c_int, c_short, c_ulonglong};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::{FromRawFd, RawFd};
use std::path::Path;

#[derive(Clone, Copy)]
#[repr(C)]
struct RawMatch {
    offset: c_ulonglong,
    line: u32,
    column: u32,
}

extern "C" {
    fn oc_pty_open(
        master_fd: *mut c_int,
        slave_name: *mut c_char,
        slave_name_len: usize,
        rows: c_short,
        cols: c_short,
    ) -> c_int;
    fn oc_pty_spawn(
        cwd: *const c_char,
        argv: *const *const c_char,
        envp: *const *const c_char,
        rows: c_short,
        cols: c_short,
        master_fd: *mut c_int,
        pid_out: *mut c_int,
    ) -> c_int;
    fn oc_pty_resize(fd: c_int, rows: c_short, cols: c_short) -> c_int;
    fn oc_pty_nonblock(fd: c_int) -> c_int;
    fn oc_pty_wait(pid: c_int, status: *mut c_int, nohang: c_int) -> c_int;
    fn oc_pty_terminate(pid: c_int) -> c_int;
    fn oc_term_size(fd: c_int, rows: *mut c_short, cols: *mut c_short) -> c_int;
    fn oc_fsync_dir(path: *const c_char) -> c_int;
    fn oc_statvfs(
        path: *const c_char,
        available: *mut c_ulonglong,
        total: *mut c_ulonglong,
    ) -> c_int;
    fn oc_search_file(
        path: *const c_char,
        needle: *const c_char,
        needle_len: usize,
        out: *mut RawMatch,
        out_cap: usize,
    ) -> std::os::raw::c_long;
}

fn errno() -> io::Error {
    io::Error::last_os_error()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub offset: u64,
    pub line: u32,
    pub column: u32,
}

/// A child process attached to a pseudo-terminal.
#[derive(Debug)]
pub struct Pty {
    master: File,
    pid: c_int,
    reaped: bool,
}

unsafe impl Send for Pty {}

impl Pty {
    /// Forks `argv` inside a new PTY. `envp` entries use `KEY=VALUE` form.
    pub fn spawn(
        cwd: Option<&Path>,
        argv: &[&OsStr],
        envp: &[&OsStr],
        rows: u16,
        cols: u16,
    ) -> io::Result<Self> {
        let argv_c: Vec<CString> = argv
            .iter()
            .map(|value| CString::new(value.as_bytes()))
            .collect::<Result<_, _>>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "argument contains NUL"))?;
        let env_c: Vec<CString> = envp
            .iter()
            .map(|value| CString::new(value.as_bytes()))
            .collect::<Result<_, _>>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "environment contains NUL"))?;
        let mut argv_ptrs: Vec<*const c_char> = argv_c.iter().map(|v| v.as_ptr()).collect();
        argv_ptrs.push(std::ptr::null());
        let mut env_ptrs: Vec<*const c_char> = env_c.iter().map(|v| v.as_ptr()).collect();
        env_ptrs.push(std::ptr::null());
        let cwd_c = cwd
            .map(|p| CString::new(p.as_os_str().as_bytes()))
            .transpose()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "cwd contains NUL"))?;

        let mut master_fd: c_int = -1;
        let mut pid: c_int = 0;
        let rc = unsafe {
            oc_pty_spawn(
                cwd_c.as_ref().map(|c| c.as_ptr()).unwrap_or(std::ptr::null()),
                argv_ptrs.as_ptr(),
                env_ptrs.as_ptr(),
                rows as c_short,
                cols as c_short,
                &mut master_fd,
                &mut pid,
            )
        };
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(-rc));
        }
        let master = unsafe { File::from_raw_fd(master_fd) };
        Ok(Self {
            master,
            pid,
            reaped: false,
        })
    }

    pub fn pid(&self) -> i32 {
        self.pid
    }

    pub fn raw_fd(&self) -> RawFd {
        use std::os::unix::io::AsRawFd;
        self.master.as_raw_fd()
    }

    pub fn master(&self) -> &File {
        &self.master
    }

    pub fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
        let rc = unsafe { oc_pty_resize(self.raw_fd(), rows as c_short, cols as c_short) };
        if rc != 0 {
            Err(io::Error::from_raw_os_error(-rc))
        } else {
            Ok(())
        }
    }

    /// Returns `Ok(None)` while the child is alive, or its exit status.
    pub fn try_wait(&mut self) -> io::Result<Option<i32>> {
        if self.reaped {
            return Ok(Some(0));
        }
        let mut status: c_int = 0;
        let rc = unsafe { oc_pty_wait(self.pid, &mut status, 1) };
        if rc < 0 {
            return Err(io::Error::from_raw_os_error(-rc));
        }
        if rc == 0 {
            return Ok(None);
        }
        self.reaped = true;
        Ok(Some(status))
    }

    pub fn terminate(&mut self) -> io::Result<()> {
        if self.reaped {
            return Ok(());
        }
        let rc = unsafe { oc_pty_terminate(self.pid) };
        self.reaped = true;
        if rc != 0 {
            Err(io::Error::from_raw_os_error(-rc))
        } else {
            Ok(())
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = unsafe { oc_pty_terminate(self.pid) };
        }
    }
}

pub fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let rc = unsafe { oc_pty_nonblock(fd) };
    if rc != 0 {
        Err(io::Error::from_raw_os_error(-rc))
    } else {
        Ok(())
    }
}

pub fn term_size(fd: RawFd) -> io::Result<(u16, u16)> {
    let mut rows: c_short = 0;
    let mut cols: c_short = 0;
    let rc = unsafe { oc_term_size(fd, &mut rows, &mut cols) };
    if rc != 0 {
        return Err(io::Error::from_raw_os_error(-rc));
    }
    Ok((rows as u16, cols as u16))
}

pub fn fsync_dir(path: &Path) -> io::Result<()> {
    let c = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let rc = unsafe { oc_fsync_dir(c.as_ptr()) };
    if rc != 0 {
        Err(io::Error::from_raw_os_error(-rc))
    } else {
        Ok(())
    }
}

pub fn statvfs(path: &Path) -> io::Result<(u64, u64)> {
    let c = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let mut available: c_ulonglong = 0;
    let mut total: c_ulonglong = 0;
    let rc = unsafe { oc_statvfs(c.as_ptr(), &mut available, &mut total) };
    if rc != 0 {
        return Err(io::Error::from_raw_os_error(-rc));
    }
    Ok((available as u64, total as u64))
}

/// Scans a regular file for a literal needle using `mmap`.
pub fn search_file(path: &Path, needle: &[u8], limit: usize) -> io::Result<Vec<Match>> {
    if needle.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let path_c = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let mut raw = vec![
        RawMatch {
            offset: 0,
            line: 0,
            column: 0,
        };
        limit
    ];
    let rc = unsafe {
        oc_search_file(
            path_c.as_ptr(),
            needle.as_ptr() as *const c_char,
            needle.len(),
            raw.as_mut_ptr(),
            raw.len(),
        )
    };
    if rc < 0 {
        return Err(io::Error::from_raw_os_error((-rc) as i32));
    }
    raw.truncate(rc as usize);
    Ok(raw
        .into_iter()
        .map(|m| Match {
            offset: m.offset,
            line: m.line,
            column: m.column,
        })
        .collect())
}

pub fn version() -> &'static str {
    option_env!("CARGO_PKG_VERSION").unwrap_or("0.1.0")
}
