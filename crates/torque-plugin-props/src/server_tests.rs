use super::*;

#[test]
fn an_address_is_its_canonical_ip_and_ipv6_shares_its_64() {
    let key = |t: &str| address_key(t).map(|k| k.to_string());
    // Mapped and plain IPv4 are one key, whatever the tag, brackets and port.
    assert_eq!(key("IP:1.2.3.4:28000").as_deref(), Some("1.2.3.4"));
    assert_eq!(key("IP6:[::ffff:1.2.3.4]:28000"), key("IP:1.2.3.4:50001"));
    assert_eq!(key("1.2.3.4"), key("::ffff:1.2.3.4"));
    // Two addresses of one /64 share a key; another /64 does not.
    let a = key("IP6:[2001:db8:1:2::10]:28000");
    assert_eq!(a.as_deref(), Some("2001:db8:1:2::/64"));
    assert_eq!(a, key("IP6:[2001:db8:1:2:ffff:1:2:3%7]:9"));
    assert_ne!(a, key("IP6:[2001:db8:1:3::10]:28000"));
    assert_eq!(key("fe80::1%3").as_deref(), Some("fe80:0:0:0::/64"));
    // Text that is no address has no key (its connection stands in for it).
    for junk in [
        "local",
        "",
        "IP:",
        "IP:999.1.1.1:5",
        "IP6:[nope]:1",
        "IP:1.2.3.4:5:6:7",
    ] {
        assert_eq!(key(junk), None, "{junk:?}");
    }
}
