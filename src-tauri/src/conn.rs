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

/// Calls a GetExtended*Table function using a thread-local scratch buffer (cutting kernel table
/// walks in half once sized and avoiding 8 intermediate heap allocations per second).
fn for_each_row<T: Copy>(
    f: unsafe extern "system" fn(*mut c_void, *mut u32, i32, u32, i32, u32) -> u32,
    af: u32,
    class: i32,
    mut visit: impl FnMut(T),
) {
    thread_local! {
        static BUF: std::cell::RefCell<Vec<u64>> = std::cell::RefCell::new(vec![0u64; 4096]);
    }
    BUF.with(|cell| {
        let mut buf = cell.borrow_mut();
        if buf.len() < 512 {
            buf.resize(4096, 0);
        }
        let mut size: u32 = (buf.len() * 8) as u32;
        let mut ok = false;
        for _ in 0..5 {
            let ptr = buf.as_mut_ptr() as *mut c_void;
            let r = unsafe { f(ptr, &mut size, 0, af, class, 0) };
            if r == 0 {
                ok = true;
                break;
            }
            if r == ERROR_INSUFFICIENT_BUFFER {
                let new_len = (size as usize + 4096) / 8 + 1;
                buf.resize(new_len, 0);
                size = (buf.len() * 8) as u32;
                continue;
            }
            return;
        }
        if !ok {
            return;
        }
        unsafe {
            let base = buf.as_ptr() as *const u8;
            let n = *(base as *const u32) as usize;
            let offset = std::mem::align_of::<T>().max(4);
            let rows = base.add(offset) as *const T;
            let max = (buf.len() * 8).saturating_sub(offset) / std::mem::size_of::<T>();
            for i in 0..n.min(max) {
                visit(*rows.add(i));
            }
        }
        // Keep steady-state buffer reasonable if a spike temporarily grew it huge
        if buf.len() > 65_536 {
            buf.truncate(16_384);
            buf.shrink_to_fit();
        }
    });
}

fn read_table<T: Copy>(
    f: unsafe extern "system" fn(*mut c_void, *mut u32, i32, u32, i32, u32) -> u32,
    af: u32,
    class: i32,
) -> Vec<T> {
    let mut out = Vec::new();
    for_each_row::<T>(f, af, class, |r| out.push(r));
    out
}

pub fn snapshot() -> Vec<Socket> {
    let mut out = Vec::with_capacity(512);
    for_each_row::<TcpRow4>(GetExtendedTcpTable, AF_INET, TCP_TABLE_OWNER_PID_ALL, |r| {
        let listen = r.state == 2;
        out.push(Socket {
            proto: Proto::Tcp,
            local: v4(r.local_addr),
            local_port: port(r.local_port),
            remote: if listen { None } else { Some((v4(r.remote_addr), port(r.remote_port))) },
            state: tcp_state(r.state),
            pid: r.pid,
        });
    });
    for_each_row::<TcpRow6>(GetExtendedTcpTable, AF_INET6, TCP_TABLE_OWNER_PID_ALL, |r| {
        let listen = r.state == 2;
        out.push(Socket {
            proto: Proto::Tcp,
            local: v6(r.local_addr),
            local_port: port(r.local_port),
            remote: if listen { None } else { Some((v6(r.remote_addr), port(r.remote_port))) },
            state: tcp_state(r.state),
            pid: r.pid,
        });
    });
    for_each_row::<UdpRow4>(GetExtendedUdpTable, AF_INET, UDP_TABLE_OWNER_PID, |r| {
        out.push(Socket {
            proto: Proto::Udp,
            local: v4(r.local_addr),
            local_port: port(r.local_port),
            remote: None,
            state: "BOUND",
            pid: r.pid,
        });
    });
    for_each_row::<UdpRow6>(GetExtendedUdpTable, AF_INET6, UDP_TABLE_OWNER_PID, |r| {
        out.push(Socket {
            proto: Proto::Udp,
            local: v6(r.local_addr),
            local_port: port(r.local_port),
            remote: None,
            state: "BOUND",
            pid: r.pid,
        });
    });
    out
}
