//! The RPC part of the reference, from the sample engine.

use crate::engines::add_rpcs;
use crate::model::{Reference, RpcDirection};

#[test]
fn the_sample_engine_gives_the_rpc_natives_and_its_rpcs() {
    let mut reference = Reference::default();
    add_rpcs(&mut reference).expect("the sample engine builds");

    let natives: Vec<&str> = reference.natives.iter().map(|n| n.name.as_str()).collect();
    for name in ["rpcToServer", "rpcToClient", "rpcDrops"] {
        assert!(natives.contains(&name), "{name} in {natives:?}");
    }

    let names: Vec<&str> = reference.rpcs.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["sample.echo", "sample.note", "sample.notice"]);
    let echo = &reference.rpcs[0];
    assert_eq!(echo.direction, RpcDirection::ToServer);
    assert!(echo.has_reply);
    let request: Vec<&str> = echo.request.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(request, ["text", "count", "scale", "target"]);
    assert_eq!(echo.request[0].max_len, 64);
    assert_eq!(echo.request[1].bits, 8);
    assert!(echo.request[2].finite);
    let notice = &reference.rpcs[2];
    assert_eq!(notice.direction, RpcDirection::ToClient);
    assert!(!notice.reliable);
    assert_eq!(
        notice.script_handler.as_deref(),
        Some("rpcClient_sample_notice")
    );
}
