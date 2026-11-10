use super::*;

fn ip(a: u8, b: u8, c: u8, d: u8) -> Host {
    Host::Ip(Ipv4Addr::new(a, b, c, d))
}

#[test]
fn a_tcp_address_reads_as_tge_reads_it() {
    let cases = [
        ("127.0.0.1:80", ip(127, 0, 0, 1), 80),
        ("IP:10.0.0.2:28001", ip(10, 0, 0, 2), 28_001),
        (
            "example.com",
            Host::Name("example.com".into()),
            DEFAULT_PORT,
        ),
        ("example.com:http", Host::Name("example.com".into()), 0),
        ("host:65537", Host::Name("host".into()), 1),
        ("host: 81x", Host::Name("host".into()), 81),
        ("", Host::Name(String::new()), DEFAULT_PORT),
    ];
    for (text, host, port) in cases {
        let target = tcp_target(text).expect(text);
        assert_eq!(
            (target.host, target.port, target.tls),
            (host, port, false),
            "{text}"
        );
    }
    assert_eq!(tcp_target("ipx:1:2"), None);
    assert_eq!(tcp_target("broadcast:28000"), None);
    assert_eq!(tcp_target("IP:BROADCAST"), None);
}

#[test]
fn an_http_address_may_name_its_scheme() {
    let https = http_target("HTTPS://api.example.com/v1/thing").expect("https");
    assert_eq!(
        (https.host.text(), https.port, https.tls),
        ("api.example.com".to_string(), 443, true)
    );
    let http = http_target("http://127.0.0.1:8080/").expect("http");
    assert_eq!(
        (http.host, http.port, http.tls),
        (ip(127, 0, 0, 1), 8080, false)
    );
    let bare = http_target("www.example.com:80").expect("bare");
    assert_eq!((bare.port, bare.tls), (80, false));
    assert_eq!(http_target("https://broadcast"), None);
}

#[test]
fn a_long_address_is_cut_where_tge_cuts_it() {
    let long = format!("{}:99", "a".repeat(300));
    let target = tcp_target(&long).expect("a name");
    assert_eq!(target.host.text().len(), MAX_ADDRESS);
    assert_eq!(
        target.port, DEFAULT_PORT,
        "the port number was past the cut"
    );
}

#[test]
fn an_accepted_peer_is_written_as_tge_writes_it() {
    let peer = "127.0.0.1:5000".parse().expect("an address");
    assert_eq!(address_text(peer), "IP:127.0.0.1:5000");
}

#[test]
fn a_name_with_a_blank_a_control_or_a_byte_past_ascii_is_unusable() {
    for name in [
        "evil\r\nX: 1",
        "a b",
        "tab\there",
        "del\u{7f}",
        "caf\u{e9}.test",
    ] {
        let target = tcp_target(&format!("{name}:80")).expect(name);
        assert_eq!(target.host, Host::Unusable, "{name:?}");
    }
    assert_eq!(
        tcp_target("ok-name.test:80").expect("ok").host,
        Host::Name("ok-name.test".into())
    );
}
