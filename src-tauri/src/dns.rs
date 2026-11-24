//! Reverse map IP -> host name built from the local Windows DNS client cache.

use std::collections::HashMap;
use std::ffi::c_void;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

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

fn seen_queries() -> &'static Mutex<HashMap<(String, u16), Instant>> {
    static SEEN: OnceLock<Mutex<HashMap<(String, u16), Instant>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Clears the incremental query stamp cache (e.g. when network changes or DNS is flushed).
pub fn invalidate_cache() {
    if let Ok(mut s) = seen_queries().lock() {
        s.clear();
    }
}

/// Reads newly cached A/AAAA names and resolves them from the local cache only (no network query),
/// skipping names already queried within the last 120s so `Dnscache` isn't hit with 1000+ RPCs/10s.
pub fn read_cache() -> HashMap<IpAddr, String> {
    let mut out = HashMap::new();
    unsafe {
        let mut head: *mut DnsCacheEntry = std::ptr::null_mut();
        if DnsGetCacheDataTable(&mut head) == 0 {
            return out;
        }
        let now = Instant::now();
        let mut seen_guard = seen_queries().lock().unwrap();
        if seen_guard.len() > 8192 {
            seen_guard.retain(|_, t| now.duration_since(*t) < Duration::from_secs(120));
        }
        let mut names: Vec<(String, String, u16)> = Vec::new();
        let mut batch: std::collections::HashSet<(String, u16)> = std::collections::HashSet::new();
        let mut cur = head;
        while !cur.is_null() {
            let e = &*cur;
            if e.wtype == DNS_TYPE_A || e.wtype == DNS_TYPE_AAAA {
                let raw = wstr(e.name);
                let norm = raw.trim().trim_end_matches('.').to_lowercase();
                if !norm.is_empty() && batch.insert((norm.clone(), e.wtype)) {
                    let fresh_enough = seen_guard
                        .get(&(norm.clone(), e.wtype))
                        .map(|t| now.duration_since(*t) < Duration::from_secs(120))
                        .unwrap_or(false);
                    if !fresh_enough {
                        seen_guard.insert((norm.clone(), e.wtype), now);
                        names.push((raw, norm, e.wtype));
                    }
                }
            }
            let next = e.next;
            if !e.name.is_null() {
                DnsFree(e.name as *mut c_void, DNS_FREE_FLAT);
            }
            DnsFree(cur as *mut c_void, DNS_FREE_FLAT);
            cur = next;
        }
        drop(seen_guard);
        let head_size = std::mem::size_of::<DnsRecordHead>();
        for (query_name, norm_name, t) in names {
            let w: Vec<u16> = query_name.encode_utf16().chain(std::iter::once(0)).collect();
            let mut res: *mut DnsRecordHead = std::ptr::null_mut();
            let rc = DnsQuery_W(w.as_ptr(), t, DNS_QUERY_NO_WIRE_QUERY, std::ptr::null_mut(), &mut res, std::ptr::null_mut());
            if rc != 0 || res.is_null() {
                if !res.is_null() {
                    DnsRecordListFree(res, DNS_FREE_RECORD_LIST);
                }
                continue;
            }
            let mut r = res;
            while !r.is_null() {
                let rec = &*r;
                let data = (r as *const u8).add(head_size);
                if rec.wtype == DNS_TYPE_A && rec.data_len >= 4 {
                    let b = std::ptr::read_unaligned(data as *const [u8; 4]);
                    out.insert(IpAddr::V4(Ipv4Addr::from(b)), norm_name.clone());
                } else if rec.wtype == DNS_TYPE_AAAA && rec.data_len >= 16 {
                    let mut b = [0u8; 16];
                    std::ptr::copy_nonoverlapping(data, b.as_mut_ptr(), 16);
                    let v6 = Ipv6Addr::from(b);
                    let ip = match v6.to_ipv4_mapped() {
                        Some(v4) => IpAddr::V4(v4),
                        None => IpAddr::V6(v6),
                    };
                    out.insert(ip, norm_name.clone());
                }
                r = rec.next;
            }
            DnsRecordListFree(res, DNS_FREE_RECORD_LIST);
        }
    }
    out
}
