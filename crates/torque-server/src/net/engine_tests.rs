use std::net::{IpAddr, Ipv4Addr};
use std::rc::Rc;

use torque_engine::rpc::{
    CONN_INBOX_CAP, CarrierFull, DEFAULT_SEND_QUEUE, Delivery as RpcDelivery, RpcCarrier,
};

use super::*;
use crate::net::rpc::RpcLanes;
use crate::net::transport::LanRules;

#[test]
fn the_byte_credit_follows_the_rate_and_saves_an_eighth_of_a_second() {
    let mut pace = Pace::new(DEFAULT_BYTE_RATE);
    let kind = LinkKind::MEMORY;
    assert_eq!(pace.room(0, kind), usize::from(DEFAULT_MAX_PACKET));
    pace.spend(usize::from(DEFAULT_MAX_PACKET));
    // One tick earns the rate's 32 ms.
    assert_eq!(
        pace.room(32, kind),
        (DEFAULT_BYTE_RATE as usize) * 32 / 1_000
    );
    // A long quiet spell saves an eighth of a second, no more; a datagram link caps it at its size.
    assert_eq!(pace.room(10_000, kind), DEFAULT_BYTE_RATE as usize / 8);
    assert_eq!(pace.room(20_000, LinkKind::PLAIN_UDP), 1_200);
}

#[test]
fn a_plain_link_backs_off_on_loss_and_a_validated_one_keeps_its_rate() {
    let mut plain = Pace::new(DEFAULT_BYTE_RATE);
    let mut validated = Pace::new(DEFAULT_BYTE_RATE);
    // One second of 20 packets, `lost` of them lost, as the session's running counts say.
    let second = |pace: &mut Pace, plain: bool, start: u32, lost: u64| {
        let before = PacketFates {
            delivered: u64::from(start) / 50,
            lost: 0,
        };
        let after = PacketFates {
            delivered: before.delivered + 20 - lost,
            lost,
        };
        pace.window = None;
        pace.observe(before, start, plain);
        pace.observe(after, start + 1_000, plain);
    };
    second(&mut plain, true, 0, 5);
    assert_eq!(
        plain.rate,
        DEFAULT_BYTE_RATE / 2,
        "25% loss halves the rate"
    );
    for k in 1..10 {
        second(&mut plain, true, k * 1_000, 20);
    }
    assert_eq!(plain.rate, MIN_BYTE_RATE, "never under the floor");
    second(&mut plain, true, 10_000, 0);
    assert_eq!(
        plain.rate,
        MIN_BYTE_RATE + MIN_BYTE_RATE / 10,
        "a clean second raises it a tenth"
    );
    second(&mut validated, false, 0, 20);
    assert_eq!(
        validated.rate, DEFAULT_BYTE_RATE,
        "the link's own congestion control decides"
    );
}

#[test]
fn plain_engine_udp_listens_only_on_private_addresses() {
    let rules = &LanRules::PLAIN;
    for ip in [[127, 0, 0, 1], [192, 168, 1, 4], [10, 2, 3, 4]] {
        assert!(check_plain_engine_bind(rules, IpAddr::V4(Ipv4Addr::from(ip))).is_ok());
    }
    for ip in [[0, 0, 0, 0], [8, 8, 8, 8], [203, 0, 113, 7]] {
        let ip = IpAddr::V4(Ipv4Addr::from(ip));
        let refusal = check_plain_engine_bind(rules, ip).expect_err("a public address");
        assert!(
            refusal.to_string().contains("private addresses only"),
            "{refusal}"
        );
    }
}

#[test]
fn the_rpc_lanes_hold_a_link_to_the_send_queue_and_a_peer_to_its_inbox_share() {
    let lanes = Rc::new(RpcLanes::default());
    let conn = Some(ConnId(3));
    let carrier = LaneCarrierProbe(Rc::clone(&lanes), conn);
    let batch = vec![0u8; 16 << 10];
    for _ in 0..DEFAULT_SEND_QUEUE / batch.len() {
        carrier.send(batch.clone()).expect("room");
    }
    assert_eq!(carrier.send(batch.clone()), Err(CarrierFull));
    let taken = lanes.take_out(conn);
    assert_eq!(taken.len(), DEFAULT_SEND_QUEUE / batch.len());
    lanes.set_unacked(conn, DEFAULT_SEND_QUEUE);
    assert_eq!(
        carrier.send(vec![1]),
        Err(CarrierFull),
        "what the session holds counts"
    );
    lanes.give_back(conn, taken);
    assert_eq!(lanes.take_out(conn).len(), DEFAULT_SEND_QUEUE / batch.len());

    let mut held = 0;
    while held + batch.len() <= CONN_INBOX_CAP {
        assert!(lanes.received(conn, batch.clone()));
        held += batch.len();
    }
    assert!(
        !lanes.received(conn, batch.clone()),
        "past the connection's share"
    );
    assert!(
        lanes.received(Some(ConnId(4)), batch.clone()),
        "another's share is its own"
    );
    let got = lanes.take_received();
    assert_eq!(got.len(), held / batch.len() + 1);
    assert!(lanes.received(conn, batch), "a take starts the next share");
}

/// The carrier a link of `conn` gets, through the trait only.
struct LaneCarrierProbe(Rc<RpcLanes>, Option<ConnId>);

impl LaneCarrierProbe {
    fn send(&self, batch: Vec<u8>) -> Result<(), CarrierFull> {
        crate::net::rpc::lane_carrier(&self.0, self.1).send(batch, RpcDelivery::Reliable)
    }
}

fn ip(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(a, b, c, d))
}

#[test]
fn an_accepted_ticket_is_refused_until_the_window_ends() {
    let mut cache = TicketCache::new([1; 16]);
    let from = ip(10, 0, 0, 1);
    assert_eq!(cache.admit(b"t", from, 0), Admit::Fresh);
    assert_eq!(
        cache.admit(b"t", from, 0),
        Admit::Fresh,
        "checking records nothing"
    );
    assert_eq!(cache.record(b"t", from, b"ann", 0), Admit::Fresh);
    assert_eq!(
        cache.admit(b"t", ip(10, 9, 9, 9), TICKET_REPLAY_MS - 1),
        Admit::Replayed
    );
    assert_eq!(cache.record(b"t", from, b"ann", 1), Admit::Replayed);
    assert_eq!(
        cache.admit(b"t", from, TICKET_REPLAY_MS),
        Admit::Fresh,
        "the window is over"
    );
    assert!(cache.is_empty());
}

#[test]
fn a_flood_of_valid_tickets_from_one_network_locks_out_only_that_network() {
    let mut cache = TicketCache::new([1; 16]);
    // One network's tickets, each a new identity, a second apart (under the server's rate): its share fills.
    let mut at = 0;
    for k in 0..TICKETS_PER_NETWORK {
        let (token, who) = (format!("t{k}"), format!("p{k}"));
        assert_eq!(
            cache.record(
                token.as_bytes(),
                ip(10, 0, 0, (k % 250) as u8),
                who.as_bytes(),
                at
            ),
            Admit::Fresh
        );
        at += 1_000 / TICKETS_PER_S + 1;
    }
    assert_eq!(
        cache.admit(b"more", ip(10, 0, 0, 77), at),
        Admit::Full,
        "the flooding network is out"
    );
    // Another network still signs in, and a replay is still refused.
    assert_eq!(cache.admit(b"other", ip(10, 0, 1, 1), at), Admit::Fresh);
    assert_eq!(
        cache.record(b"other", ip(10, 0, 1, 1), b"bob", at),
        Admit::Fresh
    );
    assert_eq!(cache.admit(b"t0", ip(10, 0, 2, 1), at), Admit::Replayed);
}

#[test]
fn one_identity_and_one_second_have_their_shares_too() {
    let mut cache = TicketCache::new([1; 16]);
    for k in 0..TICKETS_PER_IDENTITY {
        let token = format!("t{k}");
        let from = ip(10, k as u8, 0, 1);
        assert_eq!(
            cache.record(token.as_bytes(), from, b"ann", k as u32 * 100),
            Admit::Fresh
        );
    }
    assert_eq!(
        cache.record(b"t-more", ip(10, 99, 0, 1), b"ann", 5_000),
        Admit::Full
    );
    assert_eq!(
        cache.admit(b"t-more", ip(10, 99, 0, 1), 5_000),
        Admit::Fresh,
        "nothing was recorded"
    );
    // The whole server takes TICKETS_PER_S a second: the cache cannot fill within a window.
    let mut cache = TicketCache::new([2; 16]);
    let taken = (0..TICKETS_PER_S * 2)
        .filter(|k| {
            let (token, who) = (format!("t{k}"), format!("p{k}"));
            cache.record(
                token.as_bytes(),
                ip(10, *k as u8, 1, 1),
                who.as_bytes(),
                9_000,
            ) == Admit::Fresh
        })
        .count();
    assert_eq!(taken, TICKETS_PER_S as usize);
    assert!(TICKETS_PER_S as usize * (TICKET_REPLAY_MS as usize / 1_000) < TICKET_CACHE);
}

#[test]
fn the_byte_rate_follows_the_preference_each_tick() {
    let mut pace = Pace::new(DEFAULT_BYTE_RATE);
    pace.set_cap(MIN_BYTE_RATE);
    assert_eq!(pace.rate, MIN_BYTE_RATE);
    pace.set_cap(DEFAULT_BYTE_RATE);
    assert_eq!(
        pace.rate, MIN_BYTE_RATE,
        "a raised cap is earned back, not jumped to"
    );
}
