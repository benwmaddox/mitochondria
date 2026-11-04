//! Thin Windows layer: reserve address space, commit/decommit pages, read OS counters.
//! Reserve = claim addresses only (costs no memory). Commit = back them with memory.
use std::ffi::c_void;

const MEM_COMMIT: u32 = 0x1000;
const MEM_RESERVE: u32 = 0x2000;
const MEM_DECOMMIT: u32 = 0x4000;
const MEM_RELEASE: u32 = 0x8000;
const PAGE_READWRITE: u32 = 0x04;
const PAGE_NOACCESS: u32 = 0x01;

extern "system" {
    fn VirtualAlloc(addr: *mut c_void, size: usize, ty: u32, prot: u32) -> *mut c_void;
    fn VirtualFree(addr: *mut c_void, size: usize, ty: u32) -> i32;
    fn GetCurrentProcess() -> *mut c_void;
    fn K32GetProcessMemoryInfo(p: *mut c_void, c: *mut Counters, cb: u32) -> i32;
}

#[repr(C)]
struct Counters {
    cb: u32,
    page_fault_count: u32,
    peak_working_set: usize,
    working_set: usize,
    _quota: [usize; 4],
    pagefile_usage: usize,
    peak_pagefile: usize,
    private_usage: usize,
}

pub const PAGE: usize = 4096;

/// What the OS says about this process, in bytes.
#[derive(Clone, Copy, Debug)]
pub struct OsMem {
    /// Commit charge: memory the OS has promised to back (committed pages).
    pub private: usize,
    /// Working set: committed pages actually resident in RAM right now.
    pub resident: usize,
}

pub fn os_mem() -> OsMem {
    unsafe {
        let mut c: Counters = std::mem::zeroed();
        c.cb = std::mem::size_of::<Counters>() as u32;
        K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb);
        OsMem { private: c.private_usage, resident: c.working_set }
    }
}

pub fn reserve(size: usize) -> *mut u8 {
    let p = unsafe { VirtualAlloc(std::ptr::null_mut(), size, MEM_RESERVE, PAGE_NOACCESS) };
    assert!(!p.is_null(), "reserve failed");
    p as *mut u8
}

pub fn commit(p: *mut u8, size: usize) -> bool {
    !unsafe { VirtualAlloc(p as _, size, MEM_COMMIT, PAGE_READWRITE) }.is_null()
}

pub fn decommit(p: *mut u8, size: usize) {
    unsafe { VirtualFree(p as _, size, MEM_DECOMMIT) };
}

pub fn release(p: *mut u8) {
    unsafe { VirtualFree(p as _, 0, MEM_RELEASE) };
}

#[link(name = "bcrypt")]
extern "system" {
    fn BCryptGenRandom(alg: *mut c_void, buf: *mut u8, len: u32, flags: u32) -> i32;
}

/// Random bytes from the operating system (never our own generator): used for session secrets.
pub fn random_u64() -> u64 {
    let mut b = [0u8; 8];
    let st = unsafe { BCryptGenRandom(std::ptr::null_mut(), b.as_mut_ptr(), 8, 2) }; // 2 = use the system-preferred generator
    assert!(st >= 0, "OS random failed");
    u64::from_le_bytes(b)
}
