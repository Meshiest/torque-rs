//! The plain defaults: the bind policy under the plain LAN rules, the in-memory transport, how a bind port is chosen,
//! `inet_addr` and a machine over a folder.

use std::cell::RefCell;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::rc::Rc;

use torque_server::net::transport::{
    BindError, BindPolicy, LanRules, MemoryTransport, PolicyRefusal, PortChoice, RECV_BUFFER_BYTES,
    Recv, SharedTransport, Transport, TransportNetwork, parse_bind_argument,
};
use torque_server::platform::{Network, NoNetwork, Platform, inet_addr};

fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(a, b, c, d))
}

#[test]
fn the_plain_private_set_is_the_private_link_local_and_loopback_ranges() {
    let rules = &LanRules::PLAIN;
    for ip in [
        v4(10, 1, 2, 3),
        v4(172, 16, 0, 1),
        v4(172, 31, 255, 255),
        v4(192, 168, 1, 1),
        v4(169, 254, 0, 9),
        v4(127, 0, 0, 1),
        v4(127, 0, 0, 2),
    ] {
        assert!(rules.is_private(ip), "{ip}");
    }
    for ip in [
        v4(8, 8, 8, 8),
        v4(172, 32, 0, 1),
        v4(0, 0, 0, 0),
        v4(25, 0, 0, 1),
    ] {
        assert!(!rules.is_private(ip), "{ip}");
    }
    assert!(rules.is_private(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    let mapped = IpAddr::V6(Ipv4Addr::new(192, 168, 0, 1).to_ipv6_mapped());
    assert!(rules.is_private(mapped));
}

#[test]
fn the_broadcast_listener_passes_only_query_requests_from_the_lan() {
    let rules = &LanRules::PLAIN;
    let lan = SocketAddr::new(v4(192, 168, 0, 5), 28000);
    let public = SocketAddr::new(v4(8, 8, 8, 8), 28000);
    for &request in rules.broadcast_requests {
        assert!(rules.broadcast_ok(lan, &[request, 0, 0]));
        assert!(!rules.broadcast_ok(public, &[request, 0, 0]));
    }
    assert!(!rules.broadcast_ok(lan, &[]));
    assert!(!rules.broadcast_ok(lan, &[0xff]));
}

#[test]
fn the_policy_binds_only_its_address_and_refuses_public_and_ipv6() {
    let rules = &LanRules::PLAIN;
    let ip = v4(192, 168, 0, 2);
    let policy = BindPolicy::new(ip, false, rules).expect("a private address");
    assert_eq!(policy.check(ip), Ok(()));
    assert_eq!(
        policy.check(v4(192, 168, 0, 3)),
        Err(PolicyRefusal::NotTheBindAddress(v4(192, 168, 0, 3)))
    );
    let six = IpAddr::V6(Ipv6Addr::LOCALHOST);
    assert_eq!(policy.check(six), Err(PolicyRefusal::Ipv6(six)));
    assert_eq!(
        BindPolicy::new(v4(8, 8, 8, 8), false, rules),
        Err(PolicyRefusal::Public(v4(8, 8, 8, 8)))
    );
    assert!(BindPolicy::new(v4(8, 8, 8, 8), true, rules).is_ok());
    assert!(BindPolicy::new(v4(0, 0, 0, 0), false, rules).is_err());
    // The script's address: empty is the policy's own, a text `inet_addr` takes must name the same one.
    assert_eq!(policy.resolve(b""), Ok(ip));
    assert_eq!(policy.resolve(b"192.168.0.2"), Ok(ip));
    assert_eq!(policy.resolve(b"0xc0.0xa8.0.2"), Ok(ip));
    assert_eq!(
        policy.resolve(b"not an address"),
        Err(PolicyRefusal::Unparsable)
    );
}

#[test]
fn a_bind_argument_is_a_standard_ipv4_address() {
    assert_eq!(parse_bind_argument("127.0.0.1"), Ok(v4(127, 0, 0, 1)));
    assert!(parse_bind_argument("::1").is_err());
    assert!(parse_bind_argument("0177.1").is_err());
    assert!(parse_bind_argument("localhost").is_err());
}

#[test]
fn inet_addr_takes_the_classic_forms() {
    assert_eq!(inet_addr(b"127.0.0.1"), Some(0x7f00_0001));
    assert_eq!(inet_addr(b"0177.1"), Some(0x7f00_0001));
    assert_eq!(inet_addr(b"0x7f.1"), Some(0x7f00_0001));
    assert_eq!(inet_addr(b"10.65536"), Some(0x0a01_0000));
    assert_eq!(inet_addr(b"3232235521"), Some(0xc0a8_0001));
    assert_eq!(inet_addr(b"255.255.255.255"), None);
    assert_eq!(inet_addr(b"1.2.3.4.5"), None);
    assert_eq!(inet_addr(b"256.1.1.1"), None);
    assert_eq!(inet_addr(b" 1.2.3.4"), None);
    assert_eq!(inet_addr(b"1..2"), None);
    assert_eq!(inet_addr(b""), None);
    assert_eq!(inet_addr(b"08"), None);
}

#[test]
fn the_memory_transport_queues_both_ways_once_bound() {
    let (mut transport, end) = MemoryTransport::new();
    let peer = SocketAddr::new(v4(10, 0, 0, 9), 4000);
    let mut buf = vec![0u8; RECV_BUFFER_BYTES];
    assert!(!transport.send_to(peer, b"early"));
    assert!(end.inject(peer, b"hello"));
    assert!(matches!(transport.recv(&mut buf), Recv::Empty));
    transport
        .bind(v4(10, 0, 0, 1), 28000)
        .expect("a memory bind");
    assert!(end.is_bound());
    match transport.recv(&mut buf) {
        Recv::Datagram { len, from } => {
            assert_eq!((len, from), (5, peer));
            assert_eq!(&buf[..len], b"hello");
        }
        _ => panic!("the injected datagram"),
    }
    assert!(transport.send_to(peer, b"reply"));
    assert_eq!(end.take_sent(), vec![(peer, b"reply".to_vec())]);
    assert_eq!(end.binds(), vec![SocketAddr::new(v4(10, 0, 0, 1), 28000)]);
    assert!(!transport.is_real());
}

#[test]
fn an_off_band_network_binds_a_free_port_for_a_lan_band_port_and_keeps_it() {
    let band = PortChoice::OffBand {
        first: 28000,
        last: 28099,
    };
    assert_eq!(PortChoice::Asked.port_for(28000), 28000);
    assert_eq!(band.port_for(28000), 0);
    assert_eq!(band.port_for(28099), 0);
    assert_eq!(band.port_for(40000), 40000);

    let ip = v4(127, 0, 0, 1);
    let policy = BindPolicy::new(ip, false, &LanRules::PLAIN).expect("loopback");
    let (transport, end) = MemoryTransport::with_policy(policy);
    let shared: SharedTransport = Rc::new(RefCell::new(transport));
    let network = TransportNetwork::new(shared, policy).with_ports(band);
    assert_eq!(network.fixed_address(), Some("127.0.0.1".to_string()));
    assert!(network.bind(b"", 28000));
    assert!(network.bind(b"", 41000));
    assert!(!network.bind(b"10.0.0.1", 41000));
    let ports: Vec<u16> = end.binds().iter().map(SocketAddr::port).collect();
    assert_eq!(ports, vec![0, 41000]);
}

#[test]
fn a_machine_over_a_folder_names_it_with_forward_slashes() {
    let machine = Platform::over("C:\\games\\server\\", 4096, "Test OS");
    assert_eq!(machine.module_directory, "C:/games/server");
    assert_eq!(machine.profile_path, machine.module_directory);
    assert_eq!(machine.total_ram_mb, 4096);
    assert_eq!(machine.os_long, "Test OS");
    assert_eq!(Platform::over("/", 1, "").module_directory, "/");
    assert!(NoNetwork.bind(b"", 28000));
    assert_eq!(NoNetwork.shown_port(28001), 28001);
}

#[test]
fn a_bind_error_is_an_error_whose_source_is_the_policys_refusal() {
    let refusal = BindPolicy::new(v4(8, 8, 8, 8), false, &LanRules::PLAIN).unwrap_err();
    let error: Box<dyn std::error::Error> = Box::new(BindError::Policy(refusal));
    assert_eq!(error.to_string(), refusal.to_string());
    assert_eq!(
        error.source().map(ToString::to_string),
        Some(refusal.to_string())
    );
    let os: &dyn std::error::Error = &BindError::Os(std::io::ErrorKind::AddrInUse);
    assert!(os.source().is_none());
}
