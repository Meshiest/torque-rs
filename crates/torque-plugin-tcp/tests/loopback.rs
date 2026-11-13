//! TCPObject and HTTPObject against peers on 127.0.0.1: connects, lines both ways, a failed connect, a listen that accepts,
//! and HTTP requests. The scripts log each callback into `$log` in the order it came.

use crate::common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::thread;
use torque_ts_vm::testing::{eval, global};

use common::{answer, closed_port, listener, logging, pump, read_request, vm};

#[test]
fn a_connect_sends_and_receives_lines_and_the_last_one_comes_at_the_disconnect() {
    let (peer, port) = listener();
    let server = thread::spawn(move || {
        let (stream, _) = peer.accept().expect("the script connects");
        let mut reader = BufReader::new(stream.try_clone().expect("a second handle"));
        let mut first = String::new();
        reader.read_line(&mut first).expect("the script's line");
        answer(stream, b"hello\r\nworld\n\0partial");
        first
    });
    let mut vm = vm();
    eval(&mut vm, &logging("Client"));
    eval(
        &mut vm,
        &format!(
            r#"
            function Client::onConnected(%this) {{ $log = $log @ "connected;"; %this.send("ping", " pong\n"); }}
            new TCPObject(Client);
            Client.connect("127.0.0.1:{port}");
            "#
        ),
    );
    assert!(
        pump(&mut vm, |vm| global(vm, "$done") == "1"),
        "the exchange ends"
    );
    assert_eq!(
        global(&vm, "$log"),
        "connected;line:hello;line:world;line:;line:partial;disconnect;"
    );
    assert_eq!(server.join().expect("the peer"), "ping pong\n");
    assert_eq!(
        eval(&mut vm, "return Client.getGroup() == TCPGroup.getId();"),
        "1"
    );
}

#[test]
fn a_name_is_looked_up_before_the_connect_and_a_missing_one_fails_alone() {
    let (peer, port) = listener();
    let server = thread::spawn(move || {
        let (stream, _) = peer.accept().expect("the script connects");
        answer(stream, b"named\n");
    });
    let mut vm = vm();
    eval(&mut vm, &logging("Named"));
    eval(
        &mut vm,
        &format!(r#"new TCPObject(Named); Named.connect("ip:game.test:{port}");"#),
    );
    assert!(pump(&mut vm, |vm| global(vm, "$done") == "1"));
    assert_eq!(global(&vm, "$log"), "dns;connected;line:named;disconnect;");
    server.join().expect("the peer");

    eval(
        &mut vm,
        r#"$log = ""; $done = 0; Named.connect("nowhere.example:80");"#,
    );
    assert!(pump(&mut vm, |vm| global(vm, "$done") == "1"));
    assert_eq!(global(&vm, "$log"), "dnsfailed;");
}

#[test]
fn a_connect_to_a_closed_port_fails() {
    let port = closed_port();
    let mut vm = vm();
    eval(&mut vm, &logging("Lost"));
    eval(
        &mut vm,
        &format!(r#"new TCPObject(Lost); Lost.connect("127.0.0.1:{port}");"#),
    );
    assert!(pump(&mut vm, |vm| global(vm, "$done") == "1"));
    assert_eq!(global(&vm, "$log"), "failed;");
}

#[test]
fn a_listening_object_offers_each_connection_and_a_new_object_takes_it() {
    let port = closed_port();
    let mut vm = vm();
    eval(
        &mut vm,
        &format!(
            r#"
            function Server::onConnectRequest(%this, %address, %id)
            {{
                $address = %address;
                %peer = new TCPObject(Peer, %id);
                $peer = %peer;
            }}
            function Peer::onLine(%this, %line) {{ %this.send("echo " @ %line @ "\n"); }}
            function Peer::onDisconnect(%this) {{ $gone = 1; }}
            new TCPObject(Server);
            Server.listen({port});
            "#
        ),
    );
    let client = thread::spawn(move || {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("the script listens");
        stream.write_all(b"hi\nthere\n").expect("the lines");
        let mut reader = BufReader::new(stream.try_clone().expect("a second handle"));
        let mut echoed = String::new();
        for _ in 0..2 {
            reader.read_line(&mut echoed).expect("an echo");
        }
        drop(stream);
        echoed
    });
    assert!(
        pump(&mut vm, |vm| global(vm, "$gone") == "1"),
        "the peer leaves"
    );
    assert_eq!(client.join().expect("the client"), "echo hi\necho there\n");
    assert!(global(&vm, "$address").starts_with("IP:127.0.0.1:"));
    assert_eq!(
        eval(&mut vm, "return $peer.getGroup() == TCPGroup.getId();"),
        "1"
    );
    // An id no listen socket offered gives an object with no connection; a second argument is refused.
    assert_eq!(
        eval(&mut vm, "return isObject(new TCPObject(Stray, 999));"),
        "1"
    );
    assert_eq!(
        eval(&mut vm, "return isObject(new TCPObject(Two, 1, 2));"),
        "0"
    );
}

/// An HTTP server for one request: it answers `reply` and gives back the request it read.
fn http_server(reply: &'static [u8]) -> (u16, thread::JoinHandle<String>) {
    let (peer, port) = listener();
    let server = thread::spawn(move || {
        let (mut stream, _) = peer.accept().expect("the script connects");
        let request = read_request(&mut stream);
        answer(stream, reply);
        request
    });
    (port, server)
}

#[test]
fn an_http_get_sends_tges_request_and_gives_the_body_line_by_line() {
    let (port, server) =
        http_server(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nfirst line\r\nsecond");
    let mut vm = vm();
    eval(&mut vm, &logging("Feed"));
    eval(
        &mut vm,
        &format!(
            r#"new HTTPObject(Feed); Feed.get("127.0.0.1:{port}", "/news feed", "page=2&n=1");"#
        ),
    );
    assert!(pump(&mut vm, |vm| global(vm, "$done") == "1"));
    assert_eq!(
        global(&vm, "$log"),
        "connected;line:first line;line:second;disconnect;"
    );
    assert_eq!(
        server.join().expect("the server"),
        "GET /news%20feed?page=2%26n=1 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
    );
}

#[test]
fn an_http_post_sends_its_form_and_reads_a_chunked_answer() {
    let (port, server) = http_server(
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nok\nm\r\n3\r\nore\r\n0\r\n\r\n",
    );
    let mut vm = vm();
    eval(&mut vm, &logging("Form"));
    eval(
        &mut vm,
        &format!(
            r#"new HTTPObject(Form); Form.post("http://web.test:{port}/submit", "/submit", "", "name=ada&score=3");"#
        ),
    );
    assert!(pump(&mut vm, |vm| global(vm, "$done") == "1"));
    assert_eq!(
        global(&vm, "$log"),
        "dns;connected;line:ok;line:more;disconnect;"
    );
    assert_eq!(
        server.join().expect("the server"),
        "POST /submit HTTP/1.1\r\nHost: web.test\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 16\r\n\r\nname=ada&score=3"
    );
}

#[test]
fn a_deleted_object_closes_its_connection() {
    let (peer, port) = listener();
    let server = thread::spawn(move || {
        let (mut stream, _) = peer.accept().expect("the script connects");
        let mut rest = Vec::new();
        // Ends when the script's side closes.
        let _ = stream.read_to_end(&mut rest);
    });
    let mut vm = vm();
    eval(&mut vm, &logging("Gone"));
    eval(
        &mut vm,
        &format!(r#"new TCPObject(Gone); Gone.connect("127.0.0.1:{port}");"#),
    );
    assert!(pump(&mut vm, |vm| global(vm, "$log") == "connected;"));
    eval(&mut vm, "Gone.delete();");
    server.join().expect("the peer sees the close");
}

#[test]
fn a_host_name_with_a_line_break_is_refused_before_any_lookup() {
    let (peer, port) = listener();
    let mut vm = vm();
    eval(&mut vm, &logging("Smuggle"));
    eval(
        &mut vm,
        &format!(r#"new HTTPObject(Smuggle); Smuggle.get("bad\r\nX-Evil: 1.test:{port}", "/");"#),
    );
    assert!(pump(&mut vm, |vm| global(vm, "$done") == "1"));
    assert_eq!(global(&vm, "$log"), "dnsfailed;");
    peer.set_nonblocking(true).expect("non-blocking");
    assert!(peer.accept().is_err(), "nothing connected");
}
