/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `sys/mount.h`, file system statistics

use crate::dyld::{export_c_func, FunctionExports};
use crate::libc::dirent::MAXPATHLEN;
use crate::libc::errno::{set_errno, EBADF, ENOENT};
use crate::libc::posix_io::stat::uid_t;
use crate::libc::posix_io::{FileDescriptor, STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};
use crate::mem::{ConstPtr, MutPtr, SafeRead};
use crate::Environment;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;

const MFSTYPENAMELEN: usize = 16;

#[allow(non_camel_case_types)]
#[derive(Default, Debug, Copy, Clone)]
#[repr(C, packed)]
pub struct fsid_t {
    pub val: [i32; 2],
}

#[allow(non_camel_case_types)]
#[derive(Debug)]
#[repr(C, packed)]
pub struct statfs {
    pub f_bsize: u32,
    pub f_iosize: i32,
    pub f_blocks: u64,
    pub f_bfree: u64,
    pub f_bavail: u64,
    pub f_files: u64,
    pub f_ffree: u64,
    pub f_fsid: fsid_t,
    pub f_owner: uid_t,
    pub f_type: u32,
    pub f_flags: u32,
    pub f_fssubtype: u32,
    pub f_fstypename: [u8; MFSTYPENAMELEN],
    pub f_mntonname: [u8; MAXPATHLEN],
    pub f_mntfromname: [u8; MAXPATHLEN],
    pub f_reserved: [u32; 8],
}
unsafe impl SafeRead for statfs {}

/// Host free/total bytes. Unix reads statvfs; Windows (where the emulated
/// volume is just a folder on the app data drive) falls back to the
/// iOS baseline constants rather than shelling out to Win32.
#[cfg(unix)]
fn real_free_bytes() -> Option<(u64, u64)> {
    let mut vfs = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let base = crate::paths::user_data_base_path();
    let ok = unsafe { libc::statvfs(base.as_os_str().as_bytes().as_ptr() as *const _, vfs.as_mut_ptr()) == 0 };
    if !ok {
        return None;
    }
    let vfs = unsafe { vfs.assume_init() };
    Some((vfs.f_bavail as u64 * vfs.f_bsize as u64, vfs.f_frsize as u64 * vfs.f_blocks as u64))
}

#[cfg(not(unix))]
fn real_free_bytes() -> Option<(u64, u64)> {
    None
}

/// A plausible `statfs` for the (single, fake) filesystem we present to apps.
/// Values are taken from a test run of iOS 4.3 Simulator.
fn fake_statfs() -> statfs {
    // Overlay the host filesystem's real capacity on the iOS baseline so
    // apps that check free space before writing (e.g. large downloads)
    // make decisions based on actual available storage.
    let (real_free_bytes, real_total_bytes) = real_free_bytes()
        .map(|(free, total)| (Some(free), Some(total)))
        .unwrap_or((None, None));
    let to_blocks = |bytes: Option<u64>| {
        bytes.map(|b| u64::from(b / 4096)).unwrap_or(0)
    };
    let mut statfs = statfs {
        f_bsize: 4096,
        f_iosize: 1048576,
        f_blocks: to_blocks(real_total_bytes).max(16567314),
        f_bfree: to_blocks(real_free_bytes).max(12461147),
        f_bavail: to_blocks(real_free_bytes).max(12397147),
        f_files: 16567312,
        f_ffree: 12397147,
        f_fsid: fsid_t {
            val: [234881026, 17],
        },
        f_owner: 0,
        f_type: 17,
        f_flags: 75550720,
        f_fssubtype: 1,
        f_fstypename: [b'\0'; MFSTYPENAMELEN],
        f_mntonname: [b'\0'; MAXPATHLEN],
        f_mntfromname: [b'\0'; MAXPATHLEN],
        f_reserved: [0u32; 8],
    };
    statfs.f_fstypename[..3].copy_from_slice(b"hfs");
    statfs.f_mntonname[..1].copy_from_slice(b"/");
    statfs.f_mntfromname[..12].copy_from_slice(b"/dev/disk0s2");
    statfs
}

pub fn statfs_inner(env: &mut Environment, path: ConstPtr<u8>) -> (i32, statfs) {
    let Ok(path_str) = env.mem.cstr_at_utf8(path) else {
        set_errno(env, ENOENT);
        return (-1, fake_statfs());
    };

    if path_str.is_empty() {
        set_errno(env, ENOENT);
        return (-1, fake_statfs());
    }

    // Apple documents `statfs(2)` as returning information about the mounted
    // filesystem containing the supplied path. touchHLE exposes a single
    // filesystem, so any valid guest path should report the same mount.
    // Returning the fake filesystem for every valid path is therefore the
    // closest match and avoids crashing on apps that probe arbitrary paths.
    (0, fake_statfs())
}

fn statfs(env: &mut Environment, path: ConstPtr<u8>, buf: MutPtr<statfs>) -> i32 {
    let (ret, statfs) = statfs_inner(env, path);
    if ret == 0 {
        env.mem.write(buf, statfs);
    }
    ret
}

fn fstatfs(env: &mut Environment, fd: FileDescriptor, buf: MutPtr<statfs>) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    let valid = matches!(fd, STDIN_FILENO | STDOUT_FILENO | STDERR_FILENO)
        || env.libc_state.posix_io.is_fd_open(fd);
    let result = if !valid {
        set_errno(env, EBADF);
        -1
    } else {
        env.mem.write(buf, fake_statfs());
        0
    };

    log_dbg!("fstatfs({fd}, {buf:?}) -> {result}");
    result
}

pub const FUNCTIONS: FunctionExports =
    &[export_c_func!(statfs(_, _)), export_c_func!(fstatfs(_, _))];
