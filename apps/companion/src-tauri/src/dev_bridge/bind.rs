//! Address rules (spec §7, AC-3): the listener binds exactly one RFC1918 IPv4
//! — never `0.0.0.0`, a public, loopback or link-local address.

use std::net::{IpAddr, Ipv4Addr, SocketAddrV4, UdpSocket};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BindError {
    #[error("Enter an IPv4 address, e.g. 192.168.0.20")]
    NotIpv4,
    #[error("0.0.0.0 would listen on every network; pick one home-network address")]
    Unspecified,
    #[error("Link-local addresses (169.254.x.x) can't be used")]
    LinkLocal,
    #[error("Loopback addresses can't be reached from your network")]
    Loopback,
    #[error("Only home-network addresses (10.x, 172.16–31.x, 192.168.x) can be used")]
    NotPrivate,
    #[error("Pick a port between 1024 and 65535")]
    BadPort,
}

/// Parse `raw` as an IPv4 (IPv6 is refused).
pub fn parse_ipv4(raw: &str) -> Result<Ipv4Addr, BindError> {
    match raw.trim().parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => Ok(ip),
        _ => Err(BindError::NotIpv4),
    }
}

/// The bind rule. Only RFC1918 passes.
pub fn check_bind_ip(ip: Ipv4Addr) -> Result<Ipv4Addr, BindError> {
    if ip.is_unspecified() {
        Err(BindError::Unspecified)
    } else if ip.is_link_local() {
        Err(BindError::LinkLocal)
    } else if ip.is_loopback() {
        Err(BindError::Loopback)
    } else if !ip.is_private() {
        Err(BindError::NotPrivate)
    } else {
        Ok(ip)
    }
}

/// Validate a stored/entered bind address + port into a socket address.
pub fn bind_addr(raw_ip: &str, port: u16) -> Result<SocketAddrV4, BindError> {
    if port < 1024 {
        return Err(BindError::BadPort);
    }
    let ip = check_bind_ip(parse_ipv4(raw_ip)?)?;
    Ok(SocketAddrV4::new(ip, port))
}

/// An allowed caller: any concrete unicast IPv4 (the NAS is RFC1918, but a
/// loopback caller is harmless — the listener never binds loopback).
pub fn check_caller(raw: &str) -> Result<Ipv4Addr, BindError> {
    let ip = parse_ipv4(raw)?;
    if ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast() {
        return Err(BindError::NotIpv4);
    }
    Ok(ip)
}

/// Parse an allowlist, dropping entries that no longer validate.
pub fn parse_allowlist(entries: &[String]) -> Vec<Ipv4Addr> {
    entries
        .iter()
        .filter_map(|e| check_caller(e).ok())
        .collect()
}

/// This PC's private IPv4 for the panel's dropdown. Without an interface
/// enumeration crate this is the address of the default route (a UDP
/// `connect` sends no packet); empty when offline or not on a home network.
pub fn local_private_ipv4s() -> Vec<String> {
    let found = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .and_then(|s| s.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).map(|()| s))
        .and_then(|s| s.local_addr());
    match found {
        Ok(std::net::SocketAddr::V4(a)) if check_bind_ip(*a.ip()).is_ok() => {
            vec![a.ip().to_string()]
        }
        _ => Vec::new(),
    }
}

/// Whether `ip` is still an address of this PC: binding a UDP socket to it
/// succeeds only while some interface carries it (no packet is sent).
pub fn ip_is_local(ip: Ipv4Addr) -> bool {
    UdpSocket::bind((ip, 0)).is_ok()
}

/// Consecutive failed bound-IP probes before the listener closes itself: one
/// miss (resume from sleep, a Wi-Fi roam) is re-checked on the next tick.
pub const BIND_FAILS_TO_CLOSE: u8 = 2;

/// Counts consecutive failed bound-IP probes.
#[derive(Debug, Default)]
pub struct BindWatch {
    fails: u8,
}

impl BindWatch {
    /// Record one probe; `false` once the listener should close.
    pub fn observe(&mut self, ok: bool) -> bool {
        self.fails = if ok { 0 } else { self.fails.saturating_add(1) };
        self.fails < BIND_FAILS_TO_CLOSE
    }
}

/// The spec §7 fallback rule: keep listening on `ip` only while `is_local`
/// says this PC still has it; otherwise the reason the listener closed.
pub fn bound_ip_check(ip: Ipv4Addr, is_local: impl Fn(Ipv4Addr) -> bool) -> Result<(), String> {
    if is_local(ip) {
        Ok(())
    } else {
        Err(format!(
            "Dev bridge stopped: {ip} is no longer an address of this PC (network changed). Pick an address and turn it on again."
        ))
    }
}
