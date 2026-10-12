//! Port inspection. Used to *report* conflicts — never to stop the other process
//! (trd.md §7: port conflicts are surfaced, unrelated processes are never terminated).

use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::time::Duration;

use windows::Win32::Foundation::NO_ERROR;
use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows::Win32::Networking::WinSock::AF_INET;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortOwner {
    pub pid: u32,
    /// Executable file name if it could be read (another user's process may hide it).
    pub image: Option<String>,
}

/// True if nothing is listening on `port` for the loopback address (or all addresses).
pub fn is_free(port: u16) -> bool {
    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).is_ok()
        && listener_owner(port).is_none()
}

/// The process listening on `port` (IPv4), if any.
pub fn listener_owner(port: u16) -> Option<PortOwner> {
    let mut size = 0u32;
    // SAFETY: first call with no buffer queries the required size.
    unsafe {
        GetExtendedTcpTable(
            None,
            &mut size,
            false,
            AF_INET.0 as u32,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        );
    }
    let mut buf = vec![0u8; size as usize + 1024];
    size = buf.len() as u32;
    // SAFETY: `buf` is writable and at least `size` bytes.
    let rc = unsafe {
        GetExtendedTcpTable(
            Some(buf.as_mut_ptr() as *mut _),
            &mut size,
            false,
            AF_INET.0 as u32,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        )
    };
    if rc != NO_ERROR.0 {
        return None;
    }
    // SAFETY: on success the buffer holds a MIB_TCPTABLE_OWNER_PID with `dwNumEntries` rows.
    let table = unsafe { &*(buf.as_ptr() as *const MIB_TCPTABLE_OWNER_PID) };
    let rows: &[MIB_TCPROW_OWNER_PID] =
        unsafe { std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize) };
    rows.iter()
        .find(|r| u16::from_be(r.dwLocalPort as u16) == port)
        .map(|r| PortOwner {
            pid: r.dwOwningPid,
            image: super::job::identity(r.dwOwningPid).and_then(|i| {
                i.image
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            }),
        })
}

/// Bounded TCP health check against loopback.
pub fn accepts_connections(port: u16, timeout: Duration) -> bool {
    TcpStream::connect_timeout(
        &SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into(),
        timeout,
    )
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_our_own_listener_as_the_owner() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(!is_free(port));
        let owner = listener_owner(port).expect("listener must be found");
        assert_eq!(owner.pid, std::process::id());
        assert!(owner.image.unwrap().to_lowercase().ends_with(".exe"));
        assert!(accepts_connections(port, Duration::from_secs(1)));
        drop(listener);
        assert!(is_free(port));
        assert!(!accepts_connections(port, Duration::from_millis(200)));
    }
}
