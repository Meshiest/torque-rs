//! HTTPObject's HTTPS against a TLS server on 127.0.0.1 with a certificate made for the test.

mod common;

use std::io::{Read, Write};
use std::sync::Arc;
use std::thread;

use common::{eval, global, listener, logging, pump, vm_with};
use torque_plugin_tcp::{TcpPlugin, client_config};

#[test]
fn an_https_get_checks_the_certificate_and_reads_the_answer() {
    let cert =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("a certificate");
    let cert_der = cert.cert.der().clone();
    let key =
        rustls::pki_types::PrivateKeyDer::try_from(cert.key_pair.serialize_der()).expect("the key");
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let server_config = rustls::ServerConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .expect("TLS versions")
        .with_no_client_auth()
        .with_single_cert(vec![cert_der.clone()], key)
        .expect("the server's settings");
    let (peer, port) = listener();
    let server = thread::spawn(move || {
        let (tcp, _) = peer.accept().expect("the script connects");
        let conn = rustls::ServerConnection::new(Arc::new(server_config)).expect("a TLS server");
        let mut tls = rustls::StreamOwned::new(conn, tcp);
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            tls.read_exact(&mut byte).expect("the request");
            head.push(byte[0]);
        }
        tls.write_all(b"HTTP/1.1 200 OK\r\n\r\nsecret\n")
            .expect("the answer");
        tls.conn.send_close_notify();
        let _ = tls.flush();
        String::from_utf8_lossy(&head).into_owned()
    });
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert_der).expect("the test's root");
    let plugin = TcpPlugin::default()
        .with_resolver(common::test_resolver())
        .with_tls_config(Arc::new(
            client_config(roots).expect("the client's settings"),
        ));
    let mut vm = vm_with(plugin);
    eval(&mut vm, &logging("Secure"));
    eval(
        &mut vm,
        &format!(r#"new HTTPObject(Secure); Secure.get("https://localhost:{port}", "/vault");"#),
    );
    assert!(pump(&mut vm, |vm| global(vm, "$done") == "1"));
    assert_eq!(global(&vm, "$log"), "dns;connected;line:secret;disconnect;");
    assert_eq!(
        server.join().expect("the server"),
        "GET /vault HTTP/1.1\r\nHost: localhost\r\n\r\n"
    );
}

#[test]
fn an_https_get_to_an_untrusted_certificate_fails_to_connect() {
    let cert =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("a certificate");
    let key =
        rustls::pki_types::PrivateKeyDer::try_from(cert.key_pair.serialize_der()).expect("the key");
    let server_config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("TLS versions")
    .with_no_client_auth()
    .with_single_cert(vec![cert.cert.der().clone()], key)
    .expect("the server's settings");
    let (peer, port) = listener();
    let server = thread::spawn(move || {
        let (tcp, _) = peer.accept().expect("the script connects");
        let conn = rustls::ServerConnection::new(Arc::new(server_config)).expect("a TLS server");
        let mut tls = rustls::StreamOwned::new(conn, tcp);
        let mut byte = [0u8; 1];
        let _ = tls.read(&mut byte);
    });
    // Trusts an empty set of roots: no certificate passes.
    let plugin = TcpPlugin::default()
        .with_resolver(common::test_resolver())
        .with_tls_config(Arc::new(
            client_config(rustls::RootCertStore::empty()).expect("the client's settings"),
        ));
    let mut vm = vm_with(plugin);
    eval(&mut vm, &logging("Wary"));
    eval(
        &mut vm,
        &format!(r#"new HTTPObject(Wary); Wary.get("https://localhost:{port}", "/");"#),
    );
    assert!(pump(&mut vm, |vm| global(vm, "$done") == "1"));
    assert_eq!(global(&vm, "$log"), "dns;failed;");
    server.join().expect("the server");
}
