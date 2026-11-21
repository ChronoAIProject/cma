//! Lossless extended metadata. Unknown or changing attributes refuse capture.
use super::*;
use std::{ffi::CString, os::fd::AsRawFd};
const LIMIT: usize = 1024 * 1024;
pub(super) fn read(directory: &File, name: Option<&[u8]>) -> Result<Vec<(String, String)>> {
    let path = anchored_path(directory, name)?;
    let size = unsafe { list(path.as_ptr(), std::ptr::null_mut(), 0) };
    if size < 0 || size as usize > LIMIT {
        return Err(INVALID);
    }
    let mut names = vec![0u8; size as usize];
    let actual = unsafe { list(path.as_ptr(), names.as_mut_ptr().cast(), names.len()) };
    if actual != size {
        return Err(INVALID);
    }
    let mut result = Vec::new();
    let mut total = 0;
    for name in names.split(|v| *v == 0).filter(|n| !n.is_empty()) {
        if result.len() >= 128 {
            return Err(INVALID);
        }
        let key = CString::new(name).map_err(|_| INVALID)?;
        let size = unsafe { get(path.as_ptr(), key.as_ptr(), std::ptr::null_mut(), 0) };
        if size < 0 || size as usize > LIMIT {
            return Err(INVALID);
        }
        total += size as usize;
        if total > LIMIT {
            return Err(INVALID);
        }
        let mut value = vec![0u8; size as usize];
        let actual = unsafe {
            get(
                path.as_ptr(),
                key.as_ptr(),
                value.as_mut_ptr().cast(),
                value.len(),
            )
        };
        if actual != size {
            return Err(INVALID);
        }
        result.push((STANDARD.encode(name), STANDARD.encode(value)));
    }
    result.sort();
    Ok(result)
}
#[cfg(target_os = "linux")]
fn anchored_path(directory: &File, name: Option<&[u8]>) -> Result<CString> {
    let mut path = format!("/proc/self/fd/{}/", directory.as_raw_fd()).into_bytes();
    path.extend(name.unwrap_or(b"."));
    CString::new(path).map_err(|_| INVALID)
}
#[cfg(target_os = "macos")]
fn anchored_path(directory: &File, name: Option<&[u8]>) -> Result<CString> {
    let mut path = vec![0u8; libc::PATH_MAX as usize];
    if unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_GETPATH, path.as_mut_ptr()) } < 0 {
        return Err(INVALID);
    }
    let length = path.iter().position(|v| *v == 0).ok_or(INVALID)?;
    path.truncate(length);
    path.push(b'/');
    path.extend(name.unwrap_or(b"."));
    CString::new(path).map_err(|_| INVALID)
}
#[cfg(target_os = "linux")]
unsafe fn list(path: *const libc::c_char, buffer: *mut libc::c_char, size: usize) -> isize {
    unsafe { libc::llistxattr(path, buffer, size) }
}
#[cfg(target_os = "macos")]
unsafe fn list(path: *const libc::c_char, buffer: *mut libc::c_char, size: usize) -> isize {
    unsafe { libc::listxattr(path, buffer, size, libc::XATTR_NOFOLLOW) }
}
#[cfg(target_os = "linux")]
unsafe fn get(
    path: *const libc::c_char,
    name: *const libc::c_char,
    buffer: *mut libc::c_void,
    size: usize,
) -> isize {
    unsafe { libc::lgetxattr(path, name, buffer, size) }
}
#[cfg(target_os = "macos")]
unsafe fn get(
    path: *const libc::c_char,
    name: *const libc::c_char,
    buffer: *mut libc::c_void,
    size: usize,
) -> isize {
    unsafe { libc::getxattr(path, name, buffer, size, 0, libc::XATTR_NOFOLLOW) }
}
