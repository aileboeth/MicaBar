//! OS socket tables (TCP/UDP, IPv4/IPv6) with owning PIDs, via IP Helper.

use std::ffi::c_void;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[link(name = "iphlpapi")]
extern "system" {
    fn GetExtendedTcpTable(
        table: *mut c_void,
        size: *mut u32,
        order: i32,
        af: u32,
        class: i32,
        reserved: u32,
    ) -> u32;
    fn GetExtendedUdpTable(
        table: *mut c_void,
        size: *mut u32,
        order: i32,
        af: u32,
        class: i32,
        reserved: u32,
    ) -> u32;
}

const AF_INET: u32 = 2;
const AF_INET6: u32 = 23;
const TCP_TABLE_OWNER_PID_ALL: i32 = 5;
const UDP_TABLE_OWNER_PID: i32 = 1;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TcpRow4 {
    pub state: u32,
    pub local_addr: u32,
    pub local_port: u32,
    pub remote_addr: u32,
    pub remote_port: u32,
    pub pid: u32,
}

/// Raw IPv4 TCP rows (network byte order fields), for SetTcpEntry.
pub fn snapshot_raw_v4() -> Vec<TcpRow4> {
    read_table::<TcpRow4>(GetExtendedTcpTable, AF_INET, TCP_TABLE_OWNER_PID_ALL)
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TcpRow6 {
    local_addr: [u8; 16],
    local_scope: u32,
    local_port: u32,
    remote_addr: [u8; 16],
    remote_scope: u32,
    remote_port: u32,
    state: u32,
    pid: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UdpRow4 {
    local_addr: u32,
    local_port: u32,
    pid: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UdpRow6 {
    local_addr: [u8; 16],
    local_scope: u32,
    local_port: u32,
    pid: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Proto {
    Tcp,
    Udp,
}

#[derive(Clone, Debug)]
pub struct Socket {
    pub proto: Proto,
    pub local: IpAddr,
    pub local_port: u16,
    /// None for UDP (connectionless) sockets.
    pub remote: Option<(IpAddr, u16)>,
    pub state: &'static str,
    pub pid: u32,
}

fn port(p: u32) -> u16 {
    u16::from_be(p as u16)
}

fn v4(a: u32) -> IpAddr {
    IpAddr::V4(Ipv4Addr::from(a.to_ne_bytes()))
}

fn v6(a: [u8; 16]) -> IpAddr {
    let ip = Ipv6Addr::from(a);
    match ip.to_ipv4_mapped() {
        Some(v4) => IpAddr::V4(v4),
        None => IpAddr::V6(ip),
    }
}

pub fn tcp_state(s: u32) -> &'static str {
    match s {
        1 => "CLOSED",
        2 => "LISTEN",
        3 => "SYN_SENT",
        4 => "SYN_RCVD",
        5 => "ESTABLISHED",
        6 => "FIN_WAIT1",
        7 => "FIN_WAIT2",
        8 => "CLOSE_WAIT",
        9 => "CLOSING",
        10 => "LAST_ACK",
        11 => "TIME_WAIT",
        12 => "DELETE_TCB",
        _ => "UNKNOWN",
    }
}

/// Calls a GetExtended*Table function, growing the buffer until it fits,
/// and returns the rows that follow the leading `dwNumEntries` field.
fn read_table<T: Copy>(
    f: unsafe extern "system" fn(*mut c_void, *mut u32, i32, u32, i32, u32) -> u32,
    af: u32,
    class: i32,
) -> Vec<T> {
    let mut size: u32 = 0;
    let mut buf: Vec<u64> = Vec::new(); // u64 for alignment
    for _ in 0..5 {
        let ptr = if buf.is_empty() { std::ptr::null_mut() } else { buf.as_mut_ptr() as *mut c_void };
        let r = unsafe { f(ptr, &mut size, 0, af, class, 0) };
        if r == 0 && !buf.is_empty() {
            break;
        }
        if r == ERROR_INSUFFICIENT_BUFFER || (r == 0 && buf.is_empty()) {
            buf = vec![0u64; (size as usize + 1024) / 8 + 1];
            continue;
        }
        return Vec::new();
    }
    if buf.is_empty() {
        return Vec::new();
    }
    unsafe {
        let base = buf.as_ptr() as *const u8;
        let n = *(base as *const u32) as usize;
        // Rows start after the u32 count, aligned to the row's alignment (4).
        let rows = base.add(std::mem::align_of::<T>().max(4)) as *const T;
        let max = (buf.len() * 8 - 4) / std::mem::size_of::<T>();
        (0..n.min(max)).map(|i| *rows.add(i)).collect()
    }
}

pub fn snapshot() -> Vec<Socket> {
    let mut out = Vec::with_capacity(512);
    for r in read_table::<TcpRow4>(GetExtendedTcpTable, AF_INET, TCP_TABLE_OWNER_PID_ALL) {
        let listen = r.state == 2;
        out.push(Socket {
            proto: Proto::Tcp,
            local: v4(r.local_addr),
            local_port: port(r.local_port),
            remote: if listen { None } else { Some((v4(r.remote_addr), port(r.remote_port))) },
            state: tcp_state(r.state),
            pid: r.pid,
        });
    }
    for r in read_table::<TcpRow6>(GetExtendedTcpTable, AF_INET6, TCP_TABLE_OWNER_PID_ALL) {
        let listen = r.state == 2;
        out.push(Socket {
            proto: Proto::Tcp,
            local: v6(r.local_addr),
            local_port: port(r.local_port),
            remote: if listen { None } else { Some((v6(r.remote_addr), port(r.remote_port))) },
            state: tcp_state(r.state),
            pid: r.pid,
        });
    }
    for r in read_table::<UdpRow4>(GetExtendedUdpTable, AF_INET, UDP_TABLE_OWNER_PID) {
        out.push(Socket {
            proto: Proto::Udp,
            local: v4(r.local_addr),
            local_port: port(r.local_port),
            remote: None,
            state: "BOUND",
            pid: r.pid,
        });
    }
    for r in read_table::<UdpRow6>(GetExtendedUdpTable, AF_INET6, UDP_TABLE_OWNER_PID) {
        out.push(Socket {
            proto: Proto::Udp,
            local: v6(r.local_addr),
            local_port: port(r.local_port),
            remote: None,
            state: "BOUND",
            pid: r.pid,
        });
    }
    out
}
