//! Per-process network byte counters from the Microsoft-Windows-Kernel-Network ETW provider.
//! Needs administrator rights; if the session cannot start, speeds are reported as unavailable.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::os::windows::process::CommandExt;
use std::sync::atomic::{AtomicU16, Ordering};
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

static PROXY_PORTS: [AtomicU16; 8] = [
    AtomicU16::new(0),
    AtomicU16::new(0),
    AtomicU16::new(0),
    AtomicU16::new(0),
    AtomicU16::new(0),
    AtomicU16::new(0),
    AtomicU16::new(0),
    AtomicU16::new(0),
];

/// Registers active local proxy ports so loopback packets to/from the proxy are counted while
/// non-proxy localhost IPC traffic is ignored.
pub fn set_proxy_ports(ports: &[u16]) {
    let mut clean = [0u16; 8];
    let mut k = 0;
    for &p in ports {
        if p == 0 || clean[..k].contains(&p) {
            continue;
        }
        clean[k] = p;
        k += 1;
        if k == clean.len() {
            break;
        }
    }
    for (slot, val) in PROXY_PORTS.iter().zip(clean) {
        slot.store(val, Ordering::Relaxed);
    }
}

/// Removes counters of processes that have exited so the map never grows unbounded.
pub fn prune_pids(alive: &HashSet<u32>, counters: &Counters) {
    if let Ok(mut m) = counters.lock() {
        m.retain(|pid, _| alive.contains(pid));
    }
}

fn is_proxy_port(raw_port: u16) -> bool {
    if raw_port == 0 {
        return false;
    }
    let be = u16::from_be(raw_port);
    for slot in &PROXY_PORTS {
        let p = slot.load(Ordering::Relaxed);
        if p == 0 {
            break;
        }
        if p == be || p == raw_port {
            return true;
        }
    }
    false
}

fn is_loopback_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.to_ipv4_mapped().map(|v4| v4.is_loopback()).unwrap_or(false),
    }
}

const DNS_CLIENT: &str = "1c95126e-7eea-49a9-a3fe-a378b03ddb4d";

/// Name resolutions seen live (Microsoft-Windows-DNS-Client event 3008): ip -> name. Unlike the
/// DNS cache this also catches answers with tiny TTLs (TUN / fake-ip DNS).
pub type DnsLog = Arc<Mutex<Vec<(IpAddr, String)>>>;

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
                let ip = t.parse::<IpAddr>().ok().map(|ip| match ip {
                    IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
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

#[repr(C)]
struct Guid {
    d1: u32,
    d2: u16,
    d3: u16,
    d4: [u8; 8],
}

#[repr(C)]
struct WnodeHeader {
    buffer_size: u32,
    provider_id: u32,
    historical_context: u64,
    time_stamp: i64,
    guid: Guid,
    client_context: u32,
    flags: u32,
}

#[repr(C)]
struct EventTraceProperties {
    wnode: WnodeHeader,
    buffer_size: u32,
    minimum_buffers: u32,
    maximum_buffers: u32,
    maximum_file_size: u32,
    log_file_mode: u32,
    flush_timer: u32,
    enable_flags: u32,
    age_limit: i32,
    number_of_buffers: u32,
    free_buffers: u32,
    events_lost: u32,
    buffers_written: u32,
    log_buffers_lost: u32,
    real_time_buffers_lost: u32,
    logger_thread_id: usize,
    log_file_name_offset: u32,
    logger_name_offset: u32,
}

#[repr(C)]
struct EventFilterEventId<const K: usize> {
    filter_in: u8,
    reserved: u8,
    count: u16,
    events: [u16; K],
}

#[repr(C)]
struct EventFilterDescriptor {
    ptr: u64,
    size: u32,
    kind: u32,
}

#[repr(C)]
struct EnableTraceParameters {
    version: u32,
    enable_property: u32,
    control_flags: u32,
    source_id: Guid,
    enable_filter_desc: *mut EventFilterDescriptor,
    filter_desc_count: u32,
}

#[link(name = "advapi32")]
extern "system" {
    fn ControlTraceW(handle: u64, session_name: *const u16, properties: *mut EventTraceProperties, control_code: u32) -> u32;
    fn EnableTraceEx2(
        trace_handle: u64,
        provider_id: *const Guid,
        control_code: u32,
        level: u8,
        match_any_keyword: u64,
        match_all_keyword: u64,
        timeout: u32,
        enable_parameters: *const EnableTraceParameters,
    ) -> u32;
}

const WNODE_FLAG_TRACED_GUID: u32 = 0x0002_0000;
const EVENT_TRACE_CONTROL_QUERY: u32 = 0;
const EVENT_TRACE_CONTROL_STOP: u32 = 1;
const EVENT_TRACE_CONTROL_UPDATE: u32 = 2;
const ERROR_WMI_INSTANCE_NOT_FOUND: u32 = 4201;

fn init_props_buf() -> Vec<u64> {
    let mut buf = vec![0u64; 256]; // 2048 bytes, 8-byte aligned
    unsafe {
        let p = &mut *(buf.as_mut_ptr() as *mut EventTraceProperties);
        p.wnode.buffer_size = (buf.len() * 8) as u32;
        p.wnode.flags = WNODE_FLAG_TRACED_GUID;
        p.logger_name_offset = std::mem::size_of::<EventTraceProperties>() as u32;
    }
    buf
}

fn stop_stale_session() {
    let wname: Vec<u16> = SESSION.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buf = init_props_buf();
    let rc = unsafe { ControlTraceW(0, wname.as_ptr(), buf.as_mut_ptr() as *mut EventTraceProperties, EVENT_TRACE_CONTROL_STOP) };
    if rc != 0 && rc != ERROR_WMI_INSTANCE_NOT_FOUND {
        let _ = std::process::Command::new("logman")
            .args(["stop", SESSION, "-ets"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }
}

/// Bumps the running ETW session's buffer cap and restricts DNS-Client in kernel space to event 3008.
fn tune_running_session() {
    let wname: Vec<u16> = SESSION.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buf = init_props_buf();
    unsafe {
        let props = buf.as_mut_ptr() as *mut EventTraceProperties;
        if ControlTraceW(0, wname.as_ptr(), props, EVENT_TRACE_CONTROL_QUERY) != 0 {
            return;
        }
        let session_handle = (*props).wnode.historical_context;
        if (*props).maximum_buffers < 64 {
            (*props).maximum_buffers = 64;
            (*props).wnode.buffer_size = (buf.len() * 8) as u32;
            (*props).logger_name_offset = std::mem::size_of::<EventTraceProperties>() as u32;
            let _ = ControlTraceW(0, wname.as_ptr(), props, EVENT_TRACE_CONTROL_UPDATE);
        }
        if session_handle != 0 {
            // 1c95126e-7eea-49a9-a3fe-a378b03ddb4d (DNS-Client: only event 3008)
            let dns_guid = Guid {
                d1: 0x1c95_126e,
                d2: 0x7eea,
                d3: 0x49a9,
                d4: [0xa3, 0xfe, 0xa3, 0x78, 0xb0, 0x3d, 0xdb, 0x4d],
            };
            let filter_ids = EventFilterEventId { filter_in: 1, reserved: 0, count: 1, events: [3008] };
            let mut filter_desc = EventFilterDescriptor {
                ptr: &filter_ids as *const _ as usize as u64,
                size: std::mem::size_of::<EventFilterEventId<1>>() as u32,
                kind: 0x8000_0200, // EVENT_FILTER_TYPE_EVENT_ID
            };
            let params = EnableTraceParameters {
                version: 2, // ENABLE_TRACE_PARAMETERS_VERSION_2
                enable_property: 0,
                control_flags: 0,
                source_id: Guid { d1: 0, d2: 0, d3: 0, d4: [0; 8] },
                enable_filter_desc: &mut filter_desc,
                filter_desc_count: 1,
            };
            // EVENT_CONTROL_CODE_ENABLE_PROVIDER = 1, TRACE_LEVEL_VERBOSE = 5
            let _ = EnableTraceEx2(session_handle, &dns_guid, 1, 5, !0u64, 0, 0, &params);

            // 7dd42a49-5329-4832-8dfd-43d979153a88 (Kernel-Network: only send/recv byte events)
            let net_guid = Guid {
                d1: 0x7dd4_2a49,
                d2: 0x5329,
                d3: 0x4832,
                d4: [0x8d, 0xfd, 0x43, 0xd9, 0x79, 0x15, 0x3a, 0x88],
            };
            let net_ids = EventFilterEventId {
                filter_in: 1,
                reserved: 0,
                count: 8,
                events: [10, 11, 26, 27, 42, 43, 58, 59],
            };
            let mut net_desc = EventFilterDescriptor {
                ptr: &net_ids as *const _ as usize as u64,
                size: std::mem::size_of::<EventFilterEventId<8>>() as u32,
                kind: 0x8000_0200,
            };
            let net_params = EnableTraceParameters {
                version: 2,
                enable_property: 0,
                control_flags: 0,
                source_id: Guid { d1: 0, d2: 0, d3: 0, d4: [0; 8] },
                enable_filter_desc: &mut net_desc,
                filter_desc_count: 1,
            };
            let _ = EnableTraceEx2(session_handle, &net_guid, 1, 5, 0x10 | 0x20, 0, 0, &net_params);
        }
    }
}

pub fn start() -> Result<Counters, String> {
    // A previous instance that crashed leaves its session behind; stop it first via ControlTraceW.
    stop_stale_session();

    let counters: Counters = Arc::new(Mutex::new(HashMap::new()));
    let c = counters.clone();
    let provider = Provider::by_guid(KERNEL_NETWORK)
        .any(0x10 | 0x20) // KERNEL_NETWORK_KEYWORD_IPV4 | IPV6
        .add_callback(move |record: &EventRecord, locator: &SchemaLocator| {
            // measured in batches (a busy network sends thousands a second)
            thread_local!(static N: std::cell::Cell<u64> = const { std::cell::Cell::new(0) });
            N.with(|n| {
                let v = n.get() + 1;
                n.set(if v >= 1000 { crate::perf::tick("etw"); crate::perf::count("etw events", v); 0 } else { v });
            });
            let id = record.event_id();
            let send = matches!(id, 10 | 26 | 42 | 58);
            let recv = matches!(id, 11 | 27 | 43 | 59);
            if !send && !recv {
                return;
            }
            let Ok(schema) = locator.event_schema(record) else { return };
            let parser = Parser::create(record, &schema);
            let size: u32 = parser.try_parse("size").unwrap_or(0);
            if size == 0 {
                return;
            }
            // Filter out pure-loopback IPC traffic unless one endpoint is a registered proxy port.
            // Short-circuit on `saddr` so external NIC / TUN packets skip parsing `daddr`.
            if parser.try_parse::<IpAddr>("saddr").map(is_loopback_ip).unwrap_or(false)
                && parser.try_parse::<IpAddr>("daddr").map(is_loopback_ip).unwrap_or(false)
            {
                let sport: u16 = parser.try_parse("sport").unwrap_or(0);
                let dport: u16 = parser.try_parse("dport").unwrap_or(0);
                if !is_proxy_port(sport) && !is_proxy_port(dport) {
                    return;
                }
            }
            let pid: u32 = parser.try_parse("PID").unwrap_or(0);
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
    tune_running_session();
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
