//! Offline country / ASN lookup using MaxMind-format databases that proxy clients already ship
//! (Mihomo Party / Clash Verge keep country.mmdb and ASN.mmdb), or files placed next to the exe.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::SystemTime;

use maxminddb::{geoip2, Reader};

#[derive(Clone, Copy, PartialEq, Eq)]
enum CountrySchema {
    /// Mihomo `Meta-geoip0` format: heterogeneous records that are either a bare `"cn"` string or
    /// a `["us", "google"]` tag list within the same database!
    Meta,
    GeoIp2,
    Any,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum MetaRecord<'a> {
    Single(&'a str),
    List(Vec<&'a str>),
}

impl MetaRecord<'_> {
    fn code(&self) -> Option<String> {
        match self {
            MetaRecord::Single(s) => two_letter(s),
            MetaRecord::List(v) => v.iter().find_map(|s| two_letter(s)),
        }
    }
}

type FileStamp = Option<(PathBuf, Option<SystemTime>)>;

struct GeoInner {
    country: Option<(Reader<Vec<u8>>, CountrySchema)>,
    country_stamp: FileStamp,
    asn: Option<Reader<Vec<u8>>>,
    asn_stamp: FileStamp,
    source: Option<String>,
}

pub struct Geo {
    inner: RwLock<GeoInner>,
    cache: Mutex<HashMap<IpAddr, GeoInfo>>,
    last_check_ms: AtomicU64,
    pub source: Option<String>,
}

const COUNTRY_NAMES: &[&str] = &["country.mmdb", "Country.mmdb", "GeoLite2-Country.mmdb"];
const ASN_NAMES: &[&str] = &["ASN.mmdb", "GeoLite2-ASN.mmdb"];

fn candidates() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            dirs.push(d.join("geo"));
        }
    }
    if let Some(cexe) = crate::netdiag::client_exe() {
        if let Some(d) = Path::new(&cexe).parent() {
            dirs.push(d.join("data").join("work"));
            dirs.push(d.join("data"));
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        let a = PathBuf::from(appdata);
        dirs.push(a.join("clash-party").join("work"));
        dirs.push(a.join("mihomo-party").join("work"));
        dirs.push(a.join("io.github.clash-verge-rev.clash-verge-rev"));
        dirs.push(a.join("clash-verge"));
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        dirs.push(PathBuf::from(home).join(".config").join("mihomo"));
    }
    dirs
}

fn find_file(names: &[&str]) -> Option<(PathBuf, Option<SystemTime>)> {
    for d in candidates() {
        for n in names {
            let p = d.join(n);
            if let Ok(meta) = std::fs::metadata(&p) {
                if meta.is_file() {
                    return Some((p, meta.modified().ok()));
                }
            }
        }
    }
    None
}

fn open(names: &[&str]) -> Option<(Reader<Vec<u8>>, PathBuf, Option<SystemTime>)> {
    for d in candidates() {
        for n in names {
            let p = d.join(n);
            if let Ok(meta) = std::fs::metadata(&p) {
                if meta.is_file() {
                    if let Ok(r) = Reader::open_readfile(&p) {
                        return Some((r, p, meta.modified().ok()));
                    }
                }
            }
        }
    }
    None
}

fn geoip2_code(c: geoip2::Country<'_>) -> Option<String> {
    c.country
        .and_then(|c| c.iso_code)
        .or_else(|| c.registered_country.and_then(|c| c.iso_code))
        .and_then(two_letter)
}

fn detect_schema(r: &Reader<Vec<u8>>) -> CountrySchema {
    if r.metadata.database_type.to_ascii_lowercase().contains("meta-geoip") {
        return CountrySchema::Meta;
    }
    for sample in [
        IpAddr::from([8, 8, 8, 8]),
        IpAddr::from([1, 1, 1, 1]),
        IpAddr::from([114, 114, 114, 114]),
    ] {
        if r.lookup::<MetaRecord>(sample).ok().and_then(|m| m.code()).is_some() {
            return CountrySchema::Meta;
        }
        // Verify an actual 2-letter ISO code is extracted: serde otherwise deserializes any map
        // with unknown keys into `Country { country: None, registered_country: None, .. }`!
        if r.lookup::<geoip2::Country>(sample).ok().and_then(geoip2_code).is_some() {
            return CountrySchema::GeoIp2;
        }
    }
    CountrySchema::Any
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct GeoInfo {
    pub cc: Option<String>,
    pub asn: Option<u32>,
    pub org: Option<String>,
}

fn canon(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    }
}

pub fn is_private(ip: &IpAddr) -> bool {
    match canon(*ip) {
        IpAddr::V4(v) => {
            let o = v.octets();
            v.is_private()
                || v.is_loopback()
                || v.is_link_local()
                || v.is_unspecified()
                || v.is_broadcast()
                || v.is_multicast()
                || o[0] == 0 // 0.0.0.0/8 "this network"
                || (o[0] == 100 && (64..128).contains(&o[1])) // 100.64.0.0/10 CGNAT
                || (o[0] == 192 && o[1] == 0 && (o[2] == 0 || o[2] == 2)) // 192.0.0.0/24, 192.0.2.0/24 TEST-NET-1
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19)) // 198.18.0.0/15 benchmark / Mihomo fake-ip
                || (o[0] == 198 && o[1] == 51 && o[2] == 100) // 198.51.100.0/24 TEST-NET-2
                || (o[0] == 203 && o[1] == 0 && o[2] == 113) // 203.0.113.0/24 TEST-NET-3
                || o[0] >= 240 // 240.0.0.0/4 reserved
        }
        IpAddr::V6(v) => {
            let s = v.segments();
            v.is_loopback()
                || v.is_unspecified()
                || v.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // fc00::/7 ULA
                || (s[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
                || (s[0] == 0x2001 && s[1] == 0x0db8) // 2001:db8::/32 documentation
        }
    }
}

fn two_letter(s: &str) -> Option<String> {
    (s.len() == 2 && s.chars().all(|c| c.is_ascii_alphabetic())).then(|| s.to_ascii_uppercase())
}

/// mihomo's "Meta-geoip0" country.mmdb mixes bare codes ("cn") and tag lists (["us", "google"])
/// in the same database; MaxMind / Loyalsoldier files store the GeoIP2 Country structure.
fn country_code(r: &Reader<Vec<u8>>, schema: CountrySchema, ip: IpAddr) -> Option<String> {
    match schema {
        CountrySchema::Meta => r.lookup::<MetaRecord>(ip).ok().and_then(|m| m.code()),
        CountrySchema::GeoIp2 => r.lookup::<geoip2::Country>(ip).ok().and_then(geoip2_code),
        CountrySchema::Any => {
            if let Some(cc) = r.lookup::<MetaRecord>(ip).ok().and_then(|m| m.code()) {
                return Some(cc);
            }
            if let Some(cc) = r.lookup::<geoip2::Country>(ip).ok().and_then(geoip2_code) {
                return Some(cc);
            }
            if let Ok(m) = r.lookup::<HashMap<String, serde_json::Value>>(ip) {
                for k in ["iso_code", "country_code", "country", "cc"] {
                    if let Some(cc) = m.get(k).and_then(|v| v.as_str()).and_then(two_letter) {
                        return Some(cc);
                    }
                }
            }
            None
        }
    }
}

impl Geo {
    fn load_inner() -> GeoInner {
        let country = open(COUNTRY_NAMES);
        let asn = open(ASN_NAMES);
        let source = country
            .as_ref()
            .map(|(_, p, _)| p.display().to_string())
            .or_else(|| asn.as_ref().map(|(_, p, _)| p.display().to_string()));
        let country_stamp = country.as_ref().map(|(_, p, m)| (p.clone(), *m));
        let asn_stamp = asn.as_ref().map(|(_, p, m)| (p.clone(), *m));
        let country = country.map(|(r, _, _)| {
            let schema = detect_schema(&r);
            (r, schema)
        });
        let asn = asn.map(|(r, _, _)| r);
        GeoInner { country, country_stamp, asn, asn_stamp, source }
    }

    pub fn load() -> Geo {
        let inner = Self::load_inner();
        let source = inner.source.clone();
        Geo {
            inner: RwLock::new(inner),
            cache: Mutex::new(HashMap::new()),
            last_check_ms: AtomicU64::new(crate::now_ms()),
            source,
        }
    }

    pub fn source(&self) -> Option<String> {
        self.inner
            .read()
            .ok()
            .and_then(|g| g.source.clone())
            .or_else(|| self.source.clone())
    }

    /// Checks (at most every 10s) whether MMDB files appeared or were updated on disk.
    pub fn reload_if_needed(&self) {
        let now = crate::now_ms();
        let prev = self.last_check_ms.load(Ordering::Relaxed);
        if now.saturating_sub(prev) < 10_000 {
            return;
        }
        if self
            .last_check_ms
            .compare_exchange(prev, now, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let next_c = find_file(COUNTRY_NAMES);
        let next_a = find_file(ASN_NAMES);
        let changed = {
            let Ok(g) = self.inner.read() else { return };
            g.country_stamp != next_c || g.asn_stamp != next_a
        };
        if !changed {
            return;
        }
        let fresh = Self::load_inner();
        if fresh.source.is_none() {
            return;
        }
        if let Ok(mut g) = self.inner.write() {
            *g = fresh;
        }
        if let Ok(mut c) = self.cache.lock() {
            c.clear();
        }
    }

    pub fn lookup(&self, ip: IpAddr) -> GeoInfo {
        let ip = canon(ip);
        if is_private(&ip) {
            return GeoInfo::default();
        }
        self.reload_if_needed();
        if let Ok(c) = self.cache.lock() {
            if let Some(hit) = c.get(&ip) {
                return hit.clone();
            }
        }
        let mut g = GeoInfo::default();
        if let Ok(inner) = self.inner.read() {
            if let Some((r, schema)) = &inner.country {
                g.cc = country_code(r, *schema, ip);
            }
            if let Some(r) = &inner.asn {
                if let Ok(a) = r.lookup::<geoip2::Asn>(ip) {
                    g.asn = a.autonomous_system_number;
                    g.org = a.autonomous_system_organization.map(|s| s.to_string());
                }
            }
        }
        if let Ok(mut c) = self.cache.lock() {
            if c.len() >= 4096 {
                c.clear();
            }
            c.insert(ip, g.clone());
        }
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_mmdb_country_if_present() {
        let g = Geo::load();
        if g.source.is_some() {
            let cn = g.lookup("117.61.117.206".parse().unwrap());
            let us = g.lookup("8.8.8.8".parse().unwrap());
            assert_eq!(cn.cc.as_deref(), Some("CN"), "expected CN for 117.61.117.206 from {:?}", g.source);
            assert_eq!(us.cc.as_deref(), Some("US"), "expected US for 8.8.8.8 from {:?}", g.source);
        }
    }
}
