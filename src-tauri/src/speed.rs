//! Per-process network byte counters from the Microsoft-Windows-Kernel-Network ETW provider.
//! Needs administrator rights; if the session cannot start, speeds are reported as unavailable.

use std::collections::HashMap;
use std::os::windows::process::CommandExt;
use std::sync::{Arc, Mutex, OnceLock};

use ferrisetw::parser::Parser;
use ferrisetw::provider::Provider;
use ferrisetw::schema_locator::SchemaLocator;
use ferrisetw::trace::UserTrace;
use ferrisetw::EventRecord;

const SESSION: &str = "MicaBar-KernelNetwork";
const KERNEL_NETWORK: &str = "7dd42a49-5329-4832-8dfd-43d979153a88";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// pid -> (bytes sent, bytes received), monotonically increasing.
pub type Counters = Arc<Mutex<HashMap<u32, (u64, u64)>>>;

static TRACE: OnceLock<Mutex<Option<UserTrace>>> = OnceLock::new();

const DNS_CLIENT: &str = "1c95126e-7eea-49a9-a3fe-a378b03ddb4d";

/// Name resolutions seen live (Microsoft-Windows-DNS-Client event 3008): ip -> name. Unlike the
/// DNS cache this also catches answers with tiny TTLs (TUN / fake-ip DNS).
pub type DnsLog = Arc<Mutex<Vec<(std::net::IpAddr, String)>>>;

pub fn dns_log() -> DnsLog {
    static LOG: OnceLock<DnsLog> = OnceLock::new();
    LOG.get_or_init(|| Arc::new(Mutex::new(Vec::new()))).clone()
}

fn dns_provider() -> Provider {
    let log = dns_log();
    Provider::by_guid(DNS_CLIENT)
        .add_callback(move |record: &EventRecord, locator: &SchemaLocator| {
            if record.event_id() != 3008 {
                return;
            }
            let Ok(schema) = locator.event_schema(record) else { return };
            let parser = Parser::create(record, &schema);
            let name: String = parser.try_parse("QueryName").unwrap_or_default();
            let results: String = parser.try_parse("QueryResults").unwrap_or_default();
            if name.is_empty() || results.is_empty() {
                return;
            }
            let name = name.trim_end_matches('.').to_lowercase();
            let mut l = log.lock().unwrap();
            for tok in results.split(';') {
                let t = tok.trim();
                let ip = t.parse::<std::net::IpAddr>().ok().map(|ip| match ip {
                    std::net::IpAddr::V6(v6) => v6.to_ipv4_mapped().map(std::net::IpAddr::V4).unwrap_or(ip),
                    v4 => v4,
                });
                if let Some(ip) = ip {
                    l.push((ip, name.clone()));
                }
            }
            if l.len() > 5000 {
                l.drain(..2500);
            }
        })
        .build()
}

pub fn start() -> Result<Counters, String> {
    // A previous instance that crashed leaves its session behind; stop it first.
    let _ = std::process::Command::new("logman")
        .args(["stop", SESSION, "-ets"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    let counters: Counters = Arc::new(Mutex::new(HashMap::new()));
    let c = counters.clone();
    let provider = Provider::by_guid(KERNEL_NETWORK)
        .any(0x10 | 0x20) // KERNEL_NETWORK_KEYWORD_IPV4 | IPV6
        .add_callback(move |record: &EventRecord, locator: &SchemaLocator| {
            let id = record.event_id();
            let send = matches!(id, 10 | 26 | 42 | 58);
            let recv = matches!(id, 11 | 27 | 43 | 59);
            if !send && !recv {
                return;
            }
            let Ok(schema) = locator.event_schema(record) else { return };
            let parser = Parser::create(record, &schema);
            let pid: u32 = parser.try_parse("PID").unwrap_or(0);
            let size: u32 = parser.try_parse("size").unwrap_or(0);
            if size == 0 {
                return;
            }
            let mut m = c.lock().unwrap();
            let e = m.entry(pid).or_insert((0, 0));
            if send {
                e.0 += size as u64;
            } else {
                e.1 += size as u64;
            }
        })
        .build();

    let trace = UserTrace::new()
        .named(SESSION.to_string())
        .enable(provider)
        .enable(dns_provider())
        .start_and_process()
        .map_err(|e| format!("{e:?}"))?;
    TRACE.get_or_init(|| Mutex::new(None)).lock().unwrap().replace(trace);
    Ok(counters)
}

pub fn stop() {
    if let Some(t) = TRACE.get() {
        if let Some(trace) = t.lock().unwrap().take() {
            let _ = trace.stop();
        }
    }
}
