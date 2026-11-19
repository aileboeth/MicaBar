//! Offline country / ASN lookup using MaxMind-format databases that proxy clients already ship
//! (Mihomo Party / Clash Verge keep country.mmdb and ASN.mmdb), or files placed next to the exe.

use std::net::IpAddr;
use std::path::PathBuf;

use maxminddb::{geoip2, Reader};

pub struct Geo {
    country: Option<Reader<Vec<u8>>>,
    asn: Option<Reader<Vec<u8>>>,
    pub source: Option<String>,
}

fn candidates() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            dirs.push(d.join("geo"));
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        let a = PathBuf::from(appdata);
        dirs.push(a.join("mihomo-party").join("work"));
        dirs.push(a.join("clash-party").join("work"));
        dirs.push(a.join("io.github.clash-verge-rev.clash-verge-rev"));
        dirs.push(a.join("clash-verge"));
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        dirs.push(PathBuf::from(home).join(".config").join("mihomo"));
    }
    dirs
}

fn open(names: &[&str]) -> Option<(Reader<Vec<u8>>, PathBuf)> {
    for d in candidates() {
        for n in names {
            let p = d.join(n);
            if p.is_file() {
                if let Ok(r) = Reader::open_readfile(&p) {
                    return Some((r, p));
                }
            }
        }
    }
    None
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct GeoInfo {
    pub cc: Option<String>,
    pub asn: Option<u32>,
    pub org: Option<String>,
}

pub fn is_private(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            v.is_private()
                || v.is_loopback()
                || v.is_link_local()
                || v.is_unspecified()
                || v.is_broadcast()
                || v.is_multicast()
                || (o[0] == 100 && (64..128).contains(&o[1])) // CGNAT
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19)) // benchmark / fake-ip range
        }
        IpAddr::V6(v) => {
            let s = v.segments();
            v.is_loopback()
                || v.is_unspecified()
                || v.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // ULA
                || (s[0] & 0xffc0) == 0xfe80 // link-local
        }
    }
}

fn two_letter(s: &str) -> Option<String> {
    (s.len() == 2 && s.chars().all(|c| c.is_ascii_alphabetic())).then(|| s.to_ascii_uppercase())
}

/// mihomo's "Meta-geoip0" country.mmdb stores a bare code ("cn") or a list (["us", "google"]);
/// MaxMind / Loyalsoldier files store the GeoIP2 Country structure.
fn country_code(r: &Reader<Vec<u8>>, ip: IpAddr) -> Option<String> {
    if let Ok(s) = r.lookup::<String>(ip) {
        return two_letter(&s);
    }
    if let Ok(v) = r.lookup::<Vec<String>>(ip) {
        return v.iter().find_map(|s| two_letter(s));
    }
    if let Ok(c) = r.lookup::<geoip2::Country>(ip) {
        return c
            .country
            .and_then(|c| c.iso_code)
            .or_else(|| c.registered_country.and_then(|c| c.iso_code))
            .and_then(two_letter);
    }
    None
}

impl Geo {
    pub fn load() -> Geo {
        let country = open(&["country.mmdb", "Country.mmdb", "GeoLite2-Country.mmdb"]);
        let asn = open(&["ASN.mmdb", "GeoLite2-ASN.mmdb"]);
        let source = country
            .as_ref()
            .map(|(_, p)| p.display().to_string())
            .or_else(|| asn.as_ref().map(|(_, p)| p.display().to_string()));
        Geo { country: country.map(|c| c.0), asn: asn.map(|a| a.0), source }
    }

    pub fn lookup(&self, ip: IpAddr) -> GeoInfo {
        let mut g = GeoInfo::default();
        if is_private(&ip) {
            return g;
        }
        if let Some(r) = &self.country {
            g.cc = country_code(r, ip);
        }
        if let Some(r) = &self.asn {
            if let Ok(a) = r.lookup::<geoip2::Asn>(ip) {
                g.asn = a.autonomous_system_number;
                g.org = a.autonomous_system_organization.map(|s| s.to_string());
            }
        }
        g
    }
}
