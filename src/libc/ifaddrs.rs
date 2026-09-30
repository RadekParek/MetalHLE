/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `ifaddrs.h` and `net/if.h` (interface addresses and interface naming)

use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::GuestUSize;
use crate::libc::errno::{set_errno, EINVAL, ENXIO};
use crate::libc::sys::socket::sockaddr;
use crate::mem::{ConstPtr, MutPtr, SafeRead};
use crate::Environment;

// Mirrors the POSIX `struct ifaddrs` layout as seen by 32-bit ARM guests.
// All pointer fields are 4-byte guest pointers.
#[allow(non_camel_case_types)]
#[repr(C, packed)]
pub struct ifaddrs {
    /// Next node in the linked list (NULL = end).
    pub ifa_next: MutPtr<ifaddrs>,
    /// NUL-terminated interface name, e.g. "en0".
    pub ifa_name: ConstPtr<u8>,
    /// Interface flags (IFF_UP, IFF_LOOPBACK, …).
    pub ifa_flags: u32,
    /// Primary address (may be NULL).
    pub ifa_addr: u32, // guest ptr to sockaddr – typed as u32 to avoid pulling in socket types
    /// Netmask (may be NULL).
    pub ifa_netmask: u32,
    /// Broadcast or point-to-point destination address (may be NULL).
    pub ifa_broadaddr: u32,
    /// Protocol-specific data (may be NULL).
    pub ifa_data: u32,
}
// SAFETY: the struct is plain data; every field is either a scalar or a guest
// pointer that touchHLE's pointer type already validates.
unsafe impl SafeRead for ifaddrs {}

// ---------------------------------------------------------------------------
// Interface flags (`net/if.h`)
// ---------------------------------------------------------------------------

const IFF_UP: u32 = 0x1;
const IFF_BROADCAST: u32 = 0x2;
const IFF_LOOPBACK: u32 = 0x8;
const IFF_RUNNING: u32 = 0x40;
const IFF_MULTICAST: u32 = 0x1000;

/// One guest-visible IPv4 interface. The address set always comes from the
/// host, so the guest sees the machine's actual connectivity.
struct GuestInterface {
    name: String,
    flags: u32,
    addr: [u8; 4],
    netmask: [u8; 4],
    broadcast: Option<[u8; 4]>,
}

#[cfg(all(unix, not(target_os = "android")))]
/// The "broadcast/destination address" slot is named `ifa_dstaddr` on
/// BSD/macOS but is part of the `ifa_ifu` union-flattened field on Linux's
/// `struct ifaddrs`. This helper papers over the difference.
unsafe fn ifa_broad_addr(ia: &libc::ifaddrs) -> *const libc::sockaddr {
    // Linux and Android both flatten the BSD ifa_dstaddr/ifa_broadaddr union
    // into a plain `ifa_ifu` pointer in the libc crate.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        ia.ifa_ifu
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        ia.ifa_dstaddr
    }
}

/// Enumerate the host's IPv4 interfaces.
#[cfg(all(unix, not(target_os = "android")))]
fn host_ipv4_interfaces() -> Option<Vec<GuestInterface>> {
    let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut ifap) } != 0 || ifap.is_null() {
        return None;
    }
    let mut out: Vec<GuestInterface> = Vec::new();
    let mut cur = ifap;
    while !cur.is_null() {
        let ia = unsafe { &*cur };
        cur = ia.ifa_next;

        let name = if ia.ifa_name.is_null() {
            continue;
        } else {
            unsafe { std::ffi::CStr::from_ptr(ia.ifa_name) }
                .to_string_lossy()
                .into_owned()
        };
        if out.iter().any(|i| i.name == name) {
            continue;
        }
        let Some(ia_addr) = (unsafe { ia.ifa_addr.as_ref() }) else {
            continue;
        };
        if ia_addr.sa_family as i32 != libc::AF_INET {
            continue;
        }
        let sa_in = unsafe { *(ia.ifa_addr as *const libc::sockaddr_in) };
        let addr = u32::from_be(sa_in.sin_addr.s_addr).to_ne_bytes();
        let netmask = unsafe { (ia.ifa_netmask as *const libc::sockaddr_in).as_ref() }
            .map(|m| u32::from_be(m.sin_addr.s_addr).to_ne_bytes())
            .unwrap_or([255, 255, 255, 0]);
        let has_bcast = (ia.ifa_flags as u32) & libc::IFF_BROADCAST as u32 != 0;
        let broadcast = if has_bcast {
            unsafe { (ifa_broad_addr(ia) as *const libc::sockaddr_in).as_ref() }
                .map(|b| u32::from_be(b.sin_addr.s_addr).to_ne_bytes())
        } else {
            None
        };
        // Keep the host flags but make sure the broadcast flag matches what
        // we actually advertise (some hosts set it with a null address).
        let flags = (ia.ifa_flags as u32 & !IFF_BROADCAST)
            | if broadcast.is_some() { IFF_BROADCAST } else { 0 };
        out.push(GuestInterface {
            name,
            flags,
            addr,
            netmask,
            broadcast,
        });
        if out.len() >= 8 {
            break;
        }
    }
    unsafe { libc::freeifaddrs(ifap) };
    if out.is_empty() { None } else { Some(out) }
}

/// Fallback host enumeration for platforms without `getifaddrs` in `libc`:
/// a UDP route probe (no packets are sent; `connect` on UDP only selects the
/// source address the kernel would use for the default route).
#[cfg(any(not(unix), target_os = "android"))]
fn host_ipv4_interfaces() -> Option<Vec<GuestInterface>> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:53").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    match ip {
        std::net::IpAddr::V4(v4) => Some(vec![GuestInterface {
            name: "en0".to_owned(),
            flags: IFF_UP | IFF_BROADCAST | IFF_RUNNING | IFF_MULTICAST,
            addr: v4.octets(),
            netmask: [255, 255, 255, 0],
            broadcast: None,
        }]),
        std::net::IpAddr::V6(_) => None,
    }
}

// ---------------------------------------------------------------------------
// getifaddrs / freeifaddrs
// ---------------------------------------------------------------------------

/// `int getifaddrs(struct ifaddrs **ifap)`
///
/// Returns the real host interface list: the loopback interface (always
/// present, like on a real iPhone) plus the host's actual IPv4 interfaces
/// with their real addresses, netmasks and broadcast addresses. Network-aware
/// apps (GameKit matchmaking, server browsers) therefore see the same
/// connectivity as the host instead of a permanent offline mode.
fn getifaddrs(env: &mut Environment, ifap: MutPtr<MutPtr<ifaddrs>>) -> i32 {
    if ifap.is_null() {
        set_errno(env, EINVAL);
        return -1;
    }

    let mut interfaces = host_ipv4_interfaces().unwrap_or_default();
    // Loopback always exists on iOS; add it if the host list didn't include it.
    if !interfaces
        .iter()
        .any(|i| i.flags & IFF_LOOPBACK != 0 || i.name == "lo0")
    {
        interfaces.insert(
            0,
            GuestInterface {
                name: "lo0".to_owned(),
                flags: IFF_UP | IFF_LOOPBACK | IFF_RUNNING | IFF_MULTICAST,
                addr: [127, 0, 0, 1],
                netmask: [255, 0, 0, 0],
                broadcast: None,
            },
        );
    }

    let mut head: MutPtr<ifaddrs> = MutPtr::null();
    let mut prev: MutPtr<ifaddrs> = MutPtr::null();
    for iface in &interfaces {
        let name_ptr = env.mem.alloc_and_write_cstr(iface.name.as_bytes());
        let addr_ptr = env
            .mem
            .alloc_and_write(sockaddr::from_ipv4_parts(iface.addr, 0));
        let mask_ptr = env
            .mem
            .alloc_and_write(sockaddr::from_ipv4_parts(iface.netmask, 0));
        let bcast_ptr = iface
            .broadcast
            .map(|b| env.mem.alloc_and_write(sockaddr::from_ipv4_parts(b, 0)));
        let node = env.mem.alloc_and_write(ifaddrs {
            ifa_next: MutPtr::null(),
            ifa_name: name_ptr.cast().cast_const(),
            ifa_flags: iface.flags,
            ifa_addr: addr_ptr.to_bits(),
            ifa_netmask: mask_ptr.to_bits(),
            ifa_broadaddr: bcast_ptr.map(|p| p.to_bits()).unwrap_or(0),
            ifa_data: 0,
        });
        if !prev.is_null() {
            let mut prev_node = env.mem.read(prev);
            prev_node.ifa_next = node;
            env.mem.write(prev, prev_node);
        } else {
            head = node;
        }
        prev = node;
    }

    env.mem.write(ifap, head);
    log_dbg!(
        "getifaddrs() => 0 ({} interface(s): {})",
        interfaces.len(),
        interfaces
            .iter()
            .map(|i| i.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    0
}

/// `void freeifaddrs(struct ifaddrs *ifa)`
///
/// Walks the list `getifaddrs` built and frees each node, name string and
/// sockaddr with the same allocator that allocated them.
fn freeifaddrs(env: &mut Environment, ifa: MutPtr<ifaddrs>) {
    let mut cur = ifa;
    while !cur.is_null() {
        let node = env.mem.read(cur);
        if !node.ifa_name.is_null() {
            env.mem.free(node.ifa_name.cast_mut().cast());
        }
        if node.ifa_addr != 0 {
            env.mem
                .free(unsafe { MutPtr::<ifaddrs>::from_bits(node.ifa_addr).cast() });
        }
        if node.ifa_netmask != 0 {
            env.mem
                .free(unsafe { MutPtr::<ifaddrs>::from_bits(node.ifa_netmask).cast() });
        }
        if node.ifa_broadaddr != 0 {
            env.mem
                .free(unsafe { MutPtr::<ifaddrs>::from_bits(node.ifa_broadaddr).cast() });
        }
        let next = node.ifa_next;
        env.mem.free(cur.cast());
        cur = next;
    }
}

// ---------------------------------------------------------------------------
// net/if.h – interface index / name mapping
// (commonly used together with ifaddrs by network-aware apps)
// ---------------------------------------------------------------------------

/// Maximum length of an interface name including the NUL terminator.
const IF_NAMESIZE: usize = 16;

/// The interface list the guest was shown by `getifaddrs` (loopback first,
/// then the host's). `if_nametoindex` / `if_indextoname` / `if_nameindex` are
/// all defined against this same list so the three APIs stay consistent.
fn guest_interfaces() -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if let Some(interfaces) = host_ipv4_interfaces() {
        for iface in interfaces {
            if !names.contains(&iface.name) {
                names.push(iface.name);
            }
        }
    }
    if !names.iter().any(|n| n == "lo0") {
        names.insert(0, "lo0".to_owned());
    }
    names
}

/// `unsigned int if_nametoindex(const char *ifname)`
///
/// Returns the index for the named interface, or 0 on error (per POSIX,
/// which also documents `errno` getting set to `ENXIO`).
fn if_nametoindex(env: &mut Environment, ifname: ConstPtr<u8>) -> u32 {
    let name_owned = env.mem.cstr_at_utf8(ifname).unwrap_or("<invalid>").to_owned();
    let name: &str = &name_owned;
    let index = guest_interfaces()
        .iter()
        .position(|n| n == name)
        .map(|p| (p + 1) as u32)
        .unwrap_or(0);
    if index == 0 {
        set_errno(env, ENXIO);
    }
    log_dbg!("if_nametoindex(\"{name}\") => {index}");
    index
}

/// `char *if_indextoname(unsigned int ifindex, char *ifname)`
///
/// Writes the name of interface `ifindex` into `ifname` (at least
/// `IF_NAMESIZE` bytes) and returns `ifname`, or NULL on error.
fn if_indextoname(env: &mut Environment, ifindex: u32, ifname: MutPtr<u8>) -> MutPtr<u8> {
    let Some(name) = ifindex
        .checked_sub(1)
        .and_then(|i| guest_interfaces().get(i as usize).cloned())
    else {
        set_errno(env, ENXIO);
        return MutPtr::null();
    };
    if name.len() + 1 > IF_NAMESIZE {
        set_errno(env, ENXIO);
        return MutPtr::null();
    }
    if ifname.is_null() {
        set_errno(env, EINVAL);
        return MutPtr::null();
    }
    for (i, b) in name.as_bytes().iter().chain(std::iter::once(&0u8)).enumerate() {
        env.mem.write(ifname + i as GuestUSize, *b);
    }
    log_dbg!("if_indextoname({ifindex}) => \"{name}\"");
    ifname
}

// `struct if_nameindex` used by if_nameindex() / if_freenameindex().
#[allow(non_camel_case_types)]
#[repr(C, packed)]
pub struct if_nameindex {
    pub if_index: u32,
    pub if_name: ConstPtr<u8>,
}
unsafe impl SafeRead for if_nameindex {}

/// `struct if_nameindex *if_nameindex(void)`
///
/// Returns an array of all interface name/index pairs terminated by an entry
/// with `if_index == 0` and `if_name == NULL`.
fn if_nameindex(env: &mut Environment) -> MutPtr<if_nameindex> {
    let names = guest_interfaces();
    let count = names.len();
    // count entries + terminating null entry
    let array = env.mem.alloc(((count + 1) * std::mem::size_of::<if_nameindex>()) as GuestUSize);
    let mut ptr = array.cast::<if_nameindex>();
    for (i, name) in names.iter().enumerate() {
        let name_ptr = env.mem.alloc_and_write_cstr(name.as_bytes());
        env.mem.write(
            ptr + i as GuestUSize,
            if_nameindex {
                if_index: (i + 1) as u32,
                if_name: name_ptr.cast().cast_const(),
            },
        );
    }
    // Terminator
    env.mem.write(
        ptr + count as GuestUSize,
        if_nameindex {
            if_index: 0,
            if_name: ConstPtr::null(),
        },
    );
    log_dbg!("if_nameindex() => {count} interface(s)");
    ptr
}

/// `void if_freenameindex(struct if_nameindex *ptr)`
///
/// Frees the array and the name strings `if_nameindex` allocated.
fn if_freenameindex(env: &mut Environment, ptr: MutPtr<if_nameindex>) {
    if ptr.is_null() {
        return;
    }
    let mut i = 0usize;
    loop {
        let entry = env.mem.read(ptr + i as GuestUSize);
        if entry.if_index == 0 {
            break;
        }
        if !entry.if_name.is_null() {
            env.mem.free(entry.if_name.cast_mut().cast());
        }
        i += 1;
    }
    env.mem.free(ptr.cast());
}

// ---------------------------------------------------------------------------
// Export table
// ---------------------------------------------------------------------------

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(getifaddrs(_)),
    export_c_func!(freeifaddrs(_)),
    export_c_func!(if_nametoindex(_)),
    export_c_func!(if_indextoname(_, _)),
    export_c_func!(if_nameindex()),
    export_c_func!(if_freenameindex(_)),
];
