//! Joining by address and the choice of who may join a hosted game, over the loopback address: the join screen's address box
//! and Connect button (and Enter), the plain error line for an address that cannot be joined, a game hosted for the LAN and
//! one hosted for everyone (the server on every interface, joined through 127.0.0.1).

use std::net::Ipv4Addr;

use torque::client::input::{WM_LBUTTONDOWN, WM_LBUTTONUP};
use torque_example_fps::client::{ArenaGame, ClientOptions, NetMode, parse_join_address};
use torque_example_fps::files::GameFiles;
use torque_example_fps::server::{ArenaServer, ServerOptions};

const VK_RETURN: u8 = 0x0d;
const VK_PERIOD: u8 = 0xbe;

fn client(name: &str) -> ArenaGame {
    let mut game = ArenaGame::new(
        GameFiles::bundled().unwrap(),
        ClientOptions {
            net: NetMode::Loopback,
            name: name.to_string(),
            port: 0,
            map: Some("fixtures/floor.mis".to_string()),
            ..ClientOptions::default()
        },
    )
    .unwrap();
    game.host.window_resized((800, 600));
    game.step(32);
    game
}

/// The center of a control placed straight on the screen.
fn center(game: &ArenaGame, name: &str) -> (i32, i32) {
    let id = game.vm().objects().find(name.as_bytes()).expect(name);
    let field = |f: &[u8]| -> Vec<i32> {
        let text = game.vm().objects().field(id, f, b"").unwrap_or_default();
        String::from_utf8_lossy(&text)
            .split_whitespace()
            .map(|n| n.parse().unwrap())
            .collect()
    };
    let (pos, ext) = (field(b"position"), field(b"extent"));
    (pos[0] + ext[0] / 2, pos[1] + ext[1] / 2)
}

fn click(game: &mut ArenaGame, at: (i32, i32)) {
    let lparam = ((at.1 as u32 & 0xffff) << 16) | (at.0 as u32 & 0xffff);
    game.host.mouse_move(at);
    game.host.post(WM_LBUTTONDOWN, 1, lparam);
    game.host.post(WM_LBUTTONUP, 0, lparam);
}

fn press(game: &mut ArenaGame, vk: u8) {
    game.key(vk, true);
    game.key(vk, false);
}

fn click_button(game: &mut ArenaGame, name: &str) {
    let at = center(game, name);
    click(game, at);
    game.step(32);
}

fn eval(game: &mut ArenaGame, code: &str) -> String {
    game.call("eval", &[code]).unwrap()
}

fn status(game: &mut ArenaGame) -> String {
    eval(game, "return JoinStatus.getValue();")
}

fn step_both(a: &mut ArenaGame, b: &mut ArenaGame, ms: u32) {
    for _ in 0..ms / 32 {
        a.step(32);
        b.step(32);
    }
}

/// `host` hosts with `scope`; its game starts.
fn host(host: &mut ArenaGame, b: &mut ArenaGame, scope: &str) {
    host.call("hostGame", &[scope]).unwrap();
    for _ in 0..200 {
        step_both(host, b, 32);
        if host.in_game() {
            break;
        }
    }
    assert!(host.in_game(), "the {scope} game started");
}

/// `b` types `127.0.0.1:<port>` into the join screen's box and presses Connect; it ends up in the game.
fn join_by_address(a: &mut ArenaGame, b: &mut ArenaGame) {
    let port = a.hosted_address().unwrap().port();
    b.call("showJoin", &[]).unwrap();
    eval(b, &format!("AddressEdit.setValue(\"127.0.0.1:{port}\");"));
    click_button(b, "ConnectButton");
    assert_eq!(status(b), format!("Connecting to 127.0.0.1:{port}..."));
    for _ in 0..200 {
        step_both(a, b, 32);
        if b.in_game() {
            break;
        }
    }
    assert!(b.in_game(), "the second player joined by address");
    assert!(b.host.input_errors.is_empty(), "{:?}", b.host.input_errors);
}

#[test]
fn a_typed_address_and_the_connect_button_join_a_game() {
    let (mut a, mut b) = (client("Ann"), client("Bob"));
    host(&mut a, &mut b, "lan");
    join_by_address(&mut a, &mut b);
    assert_eq!(
        b.global("$Pref::Client::LastAddress"),
        format!("127.0.0.1:{}", a.hosted_address().unwrap().port()),
        "the address is remembered"
    );
    step_both(&mut a, &mut b, 1000);
    assert!(a.own_look().is_some() && b.own_look().is_some());
}

#[test]
fn typed_keys_and_enter_connect_and_a_bare_address_means_port_28000() {
    let mut g = client("Ann");
    g.call("showJoin", &[]).unwrap();
    let at = center(&g, "AddressEdit");
    click(&mut g, at);
    for vk in [
        b'1', b'2', b'7', VK_PERIOD, b'0', VK_PERIOD, b'0', VK_PERIOD, b'1',
    ] {
        press(&mut g, vk);
    }
    assert_eq!(eval(&mut g, "return AddressEdit.getValue();"), "127.0.0.1");
    press(&mut g, VK_RETURN);
    g.step(32);
    assert_eq!(status(&mut g), "Connecting to 127.0.0.1:28000...");
    assert!(g.host.input_errors.is_empty(), "{:?}", g.host.input_errors);
}

#[test]
fn an_address_that_cannot_be_joined_shows_a_plain_error() {
    let mut g = client("Ann");
    g.call("showJoin", &[]).unwrap();
    for (typed, said) in [
        ("", "Type the game's address, like 192.168.1.20:28000."),
        (
            "example.com",
            "Names are not looked up. Type the address in numbers, like 192.168.1.20.",
        ),
        (
            "1.2.3.4:abc",
            "That is not an address. Use numbers like 192.168.1.20:28000.",
        ),
        ("1.2.3.4:0", "Ports are numbers from 1 to 65535."),
        (
            "1.2.3.4:70000",
            "That is not an address. Use numbers like 192.168.1.20:28000.",
        ),
        ("0.0.0.0:28000", "That address cannot be a game."),
        ("192.168.1.255", "That address cannot be a game."),
        ("240.0.0.1", "That address cannot be a game."),
        ("0.1.2.3:28000", "That address cannot be a game."),
        (
            "999.1.1.1",
            "That is not an address. Use numbers like 192.168.1.20:28000.",
        ),
    ] {
        eval(&mut g, &format!("AddressEdit.setValue(\"{typed}\");"));
        click_button(&mut g, "ConnectButton");
        assert_eq!(status(&mut g), said, "for {typed:?}");
        assert_eq!(
            eval(&mut g, "return isObject(ServerConnection);"),
            "0",
            "no connection is made for {typed:?}"
        );
    }
    assert!(g.host.input_errors.is_empty(), "{:?}", g.host.input_errors);
    assert_eq!(
        g.global("$Pref::Client::LastAddress"),
        "",
        "a bad one is not kept"
    );
}

#[test]
fn the_address_forms_the_box_takes() {
    let ok = |t: &str| parse_join_address(t).map(|a| a.to_string());
    assert_eq!(ok("192.168.1.20"), Ok("192.168.1.20:28000".to_string()));
    assert_eq!(
        ok(" 192.168.1.20:2000 "),
        Ok("192.168.1.20:2000".to_string())
    );
    assert_eq!(ok("IP:10.0.0.5:28001"), Ok("10.0.0.5:28001".to_string()));
    assert_eq!(ok("localhost"), Ok("127.0.0.1:28000".to_string()));
    assert_eq!(ok("203.0.113.5:28000"), Ok("203.0.113.5:28000".to_string()));
    for bad in [
        "255.255.255.255",
        "224.0.0.1",
        "10.0.0.255",
        "240.0.0.1",
        "0.1.2.3",
    ] {
        assert!(ok(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_game_hosted_for_the_lan_starts_and_a_second_client_joins_it() {
    let (mut a, mut b) = (client("Ann"), client("Bob"));
    host(&mut a, &mut b, "lan");
    assert!(a.hosted_address().unwrap().ip().is_loopback());
    assert_eq!(a.global("$Pref::Arena::HostScope"), "lan");
    join_by_address(&mut a, &mut b);
}

#[test]
fn a_game_hosted_for_everyone_starts_and_a_client_joins_over_loopback() {
    let (mut a, mut b) = (client("Ann"), client("Bob"));
    host(&mut a, &mut b, "public");
    let bound = a.hosted_address().unwrap();
    // A harness binds no socket beyond the loopback address, whoever may join.
    assert!(bound.ip().is_loopback(), "bound to {bound}");
    assert_eq!(a.global("$Pref::Arena::HostScope"), "public");
    // The host joined its own game, and the chat box tells it what a public game needs.
    let lines = eval(
        &mut a,
        "return ChatLines.getLineText(1) @ \"|\" @ ChatLines.getLineText(2);",
    );
    assert_eq!(
        lines,
        format!(
            "Outside players: forward UDP {} to this computer|and allow it through the firewall.",
            bound.port()
        )
    );
    join_by_address(&mut a, &mut b);
}

#[test]
fn the_host_screen_offers_the_choice_and_remembers_it() {
    let mut g = client("Ann");
    click_button(&mut g, "HostButton");
    assert_eq!(
        eval(&mut g, "return HostLast.getValue();"),
        "Last time you hosted: LAN only."
    );
    // An empty choice takes the last one.
    g.call("hostGame", &["public"]).unwrap();
    g.call("hostGame", &[""]).unwrap();
    assert_eq!(g.global("$Arena::HostScope"), "public");
    g.call("showHost", &[]).unwrap();
    assert_eq!(
        eval(&mut g, "return HostLast.getValue();"),
        "Last time you hosted: Public."
    );
    // The two buttons are on the screen and press the way the others do.
    click_button(&mut g, "HostLanButton");
    assert_eq!(g.global("$Arena::HostScope"), "lan");
    assert!(g.server.is_some(), "the pressed button started the game");
}

#[test]
fn a_server_listens_on_every_interface_only_when_asked_to() {
    let files = GameFiles::bundled().unwrap();
    let options = |address, public| ServerOptions {
        address,
        port: 0,
        name: "Test".to_string(),
        mission: "fixtures/floor.mis".to_string(),
        public,
    };
    let refused = ArenaServer::start(&files, &options(Ipv4Addr::UNSPECIFIED, false));
    assert!(refused.is_err(), "0.0.0.0 needs the public choice");
    let other = ArenaServer::start(&files, &options(Ipv4Addr::new(8, 8, 8, 8), true));
    assert!(other.is_err(), "no other public address is allowed");
}

/// The success case binds every interface of this computer: run by hand.
#[test]
#[ignore = "binds all interfaces; run by hand"]
fn a_server_listens_on_every_interface_when_asked_to() {
    let files = GameFiles::bundled().unwrap();
    let all = ArenaServer::start(
        &files,
        &ServerOptions {
            address: Ipv4Addr::UNSPECIFIED,
            port: 0,
            name: "Test".to_string(),
            mission: "fixtures/floor.mis".to_string(),
            public: true,
        },
    )
    .unwrap();
    assert!(all.address().unwrap().ip().is_unspecified());
}

/// A server on every interface answers the LAN list and a connection to this computer's LAN address (a client over the real
/// LAN sockets; skipped on a computer with no LAN address).
#[test]
#[ignore = "binds all interfaces and queries the real LAN; run by hand"]
fn a_server_on_every_interface_is_listed_and_joined_through_the_lan_address() {
    let Some(lan) = torque_example_fps::server::lan_addresses().first().copied() else {
        return;
    };
    let files = GameFiles::bundled().unwrap();
    let mut server = ArenaServer::start_dedicated(
        &files,
        &ServerOptions {
            address: Ipv4Addr::UNSPECIFIED,
            port: 0,
            name: "Everyone".to_string(),
            mission: "fixtures/floor.mis".to_string(),
            public: true,
        },
    )
    .unwrap();
    let port = server.address().unwrap().port();
    let mut game = ArenaGame::new(
        GameFiles::bundled().unwrap(),
        ClientOptions {
            net: NetMode::Lan,
            name: "Bob".to_string(),
            port,
            ..ClientOptions::default()
        },
    )
    .unwrap();
    game.host.window_resized((800, 600));
    game.call("showJoin", &[]).unwrap();
    for _ in 0..150 {
        server.tick();
        game.step(32);
        if game.global("$Arena::ServerRows") != "0" && !game.global("$Arena::ServerRows").is_empty()
        {
            break;
        }
    }
    // Seen from this computer it answers on its loopback and its LAN address, so the list may hold it twice.
    assert_eq!(game.global("$ServerInfo::Name"), "Everyone");
    assert!(
        game.global("$Arena::ServerRows")
            .parse::<u32>()
            .is_ok_and(|n| n >= 1),
        "the LAN list shows it"
    );
    eval(
        &mut game,
        &format!("AddressEdit.setValue(\"{lan}:{port}\");"),
    );
    click_button(&mut game, "ConnectButton");
    for _ in 0..300 {
        server.tick();
        game.step(32);
        if game.in_game() {
            break;
        }
    }
    assert!(game.in_game(), "joined through {lan}:{port}");
}
