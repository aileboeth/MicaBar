//! Reverse map IP -> host name built from the local Windows DNS client cache.

use std::collections::HashMap;
use std::ffi::c_void;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[repr(C)]
struct DnsCacheEntry {
    next: *mut DnsCacheEntry,
    name: *mut u16,
    wtype: u16,
    data_len: u16,
    flags: u32,
}

#[repr(C)]
struct DnsRecordHead {
    next: *mut DnsRecordHead,
    name: *mut u16,
    wtype: u16,
    data_len: u16,
    flags: u32,
    ttl: u32,
    reserved: u32,
    // followed by the Data union (8-byte aligned, offset 32 on x64)
}

#[link(name = "dnsapi")]
extern "system" {
    fn DnsGetCacheDataTable(entry: *mut *mut DnsCacheEntry) -> i32;
    fn DnsQuery_W(
        name: *const u16,
        wtype: u16,
        options: u32,
        extra: *mut c_void,
        results: *mut *mut DnsRecordHead,
        reserved: *mut c_void,
    ) -> i32;
    fn DnsRecordListFree(list: *mut DnsRecordHead, free_type: i32);
    fn DnsFree(data: *mut c_void, free_type: i32);
}

const DNS_TYPE_A: u16 = 1;
const DNS_TYPE_AAAA: u16 = 28;
const DNS_QUERY_NO_WIRE_QUERY: u32 = 0x10;
const DNS_QUERY_BYPASS_CACHE: u32 = 0x8;

/// A fresh A lookup on the wire (the cache bypassed); the DnsQuery status code (0 = ok).
pub fn query_fresh(host: &str) -> i32 {
    let w: Vec<u16> = host.encode_utf16().chain(std::iter::once(0)).collect();
    let mut res: *mut DnsRecordHead = std::ptr::null_mut();
    unsafe {
        let rc = DnsQuery_W(w.as_ptr(), DNS_TYPE_A, DNS_QUERY_BYPASS_CACHE, std::ptr::null_mut(), &mut res, std::ptr::null_mut());
        if !res.is_null() {
            DnsRecordListFree(res, DNS_FREE_RECORD_LIST);
        }
        rc
    }
}
const DNS_FREE_FLAT: i32 = 0;
const DNS_FREE_RECORD_LIST: i32 = 1;

unsafe fn wstr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
}

/// Reads every cached A/AAAA name and resolves it from the cache only (no network query).
pub fn read_cache() -> HashMap<IpAddr, String> {
    let mut out = HashMap::new();
    unsafe {
        let mut head: *mut DnsCacheEntry = std::ptr::null_mut();
        if DnsGetCacheDataTable(&mut head) == 0 {
            return out;
        }
        let mut names: Vec<(String, u16)> = Vec::new();
        let mut cur = head;
        while !cur.is_null() {
            let e = &*cur;
            if e.wtype == DNS_TYPE_A || e.wtype == DNS_TYPE_AAAA {
                names.push((wstr(e.name), e.wtype));
            }
            let next = e.next;
            if !e.name.is_null() {
                DnsFree(e.name as *mut c_void, DNS_FREE_FLAT);
            }
            DnsFree(cur as *mut c_void, DNS_FREE_FLAT);
            cur = next;
        }
        for (name, t) in names {
            let w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let mut res: *mut DnsRecordHead = std::ptr::null_mut();
            if DnsQuery_W(w.as_ptr(), t, DNS_QUERY_NO_WIRE_QUERY, std::ptr::null_mut(), &mut res, std::ptr::null_mut()) != 0 {
                continue;
            }
            let mut r = res;
            while !r.is_null() {
                let rec = &*r;
                let data = (r as *const u8).add(32);
                if rec.wtype == DNS_TYPE_A {
                    let a = *(data as *const u32);
                    out.insert(IpAddr::V4(Ipv4Addr::from(a.to_ne_bytes())), name.clone());
                } else if rec.wtype == DNS_TYPE_AAAA {
                    let mut b = [0u8; 16];
                    std::ptr::copy_nonoverlapping(data, b.as_mut_ptr(), 16);
                    out.insert(IpAddr::V6(Ipv6Addr::from(b)), name.clone());
                }
                r = rec.next;
            }
            DnsRecordListFree(res, DNS_FREE_RECORD_LIST);
        }
    }
    out
}
