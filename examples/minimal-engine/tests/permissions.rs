//! Permissions through the engine's own server and client over the memory transport: a script defines a role and gives it to a
//! connection, the checks answer, the caps refuse with a reason, and what a connection held goes when it ends.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use torque::app::client::{ClientNet, GameClient};
use torque::app::server::Listen;
use torque::engine::permissions::{Permissions, SubjectKey};
use torque_example_minimal_engine::{builder, client, files, server};

const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_200);
const CLIENT: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_201);

fn connected() -> GameClient {
    let files = files().unwrap();
    let engine = builder(&files).unwrap().build().unwrap();
    let server = server(&engine, Listen::Memory(SERVER)).expect("the server starts");
    let mut client = client(
        &engine,
        ClientNet::Memory {
            client: CLIENT,
            server: SERVER,
        },
    )
    .expect("the client");
    client.start_hosting(server);
    client.call("connectTo", &[&SERVER.to_string()]).unwrap();
    for _ in 0..200 {
        client.step(32);
    }
    client
}

fn eval(client: &mut GameClient, code: &str) -> String {
    let vm = client.server.as_mut().unwrap().vm_mut();
    String::from_utf8_lossy(&vm.eval(code.as_bytes()).unwrap()).into_owned()
}

#[test]
fn a_script_defines_roles_and_grants_them_to_a_connection() {
    let mut client = connected();
    let answers = eval(
        &mut client,
        "%c = ClientGroup.getObject(0);\n\
         declarePermission(\"props.spawn\", \"Spawn a prop\");\n\
         $r = definePermissionRole(\"builder\", \"props.* chat.mute\");\n\
         $before = %c.hasPermission(\"props.spawn\");\n\
         $added = %c.addRole(\"builder\");\n\
         $again = %c.addRole(\"builder\");\n\
         $after = %c.hasPermission(\"props.spawn\") @ %c.hasPermission(\"props.spawn.big\") @ %c.hasPermission(\"tools.paint\");\n\
         $roles = %c.getRoles();\n\
         $has = %c.hasRole(\"builder\");\n\
         %c.grant(\"tools.*\");\n\
         $tool = %c.hasPermission(\"tools.paint\");\n\
         %c.revoke(\"tools.*\");\n\
         $tool2 = %c.hasPermission(\"tools.paint\");\n\
         $removed = %c.removeRole(\"builder\");\n\
         $gone = %c.hasPermission(\"props.spawn\");\n\
         return $r SPC $before SPC $added SPC $again SPC $after SPC $roles SPC $has SPC $tool SPC $tool2 SPC $removed SPC $gone;",
    );
    // definePermissionRole 1, before 0, added 1, again 0, props.spawn 1 + props.spawn.big 1 + tools.paint 0 joined by `@`
    // ("110"), roles "builder", has 1, tool 1, tool2 0, removed 1, gone 0.
    assert_eq!(answers, "1 0 1 0 110 builder 1 1 0 1 0");
}

#[test]
fn a_refused_grant_says_why_and_changes_nothing() {
    let mut client = connected();
    let answer = eval(
        &mut client,
        "%c = ClientGroup.getObject(0);\n\
         return %c.addRole(\"nobody\") SPC %c.grant(\"a.*.b\") SPC %c.grant(\"Bad Name\") SPC definePermissionRole(\"Bad Role\", \"x\") SPC %c.getRoles();",
    );
    assert_eq!(answer.trim(), "0 0 0 0");
    client.step(64);
    let log = client.server_log.join(" | ");
    assert!(log.contains("there is no role `nobody`"), "{log}");
    assert!(
        log.contains("is not a segment") || log.contains("last segment"),
        "{log}"
    );
}

#[test]
fn the_caps_refuse_a_33rd_role_and_a_65th_grant() {
    let mut client = connected();
    let answer = eval(
        &mut client,
        "%c = ClientGroup.getObject(0);\n\
         for (%i = 0; %i < 33; %i++) { definePermissionRole(\"r\" @ %i, \"\"); }\n\
         for (%i = 0; %i < 33; %i++) { $roles = $roles + %c.addRole(\"r\" @ %i); }\n\
         for (%i = 0; %i < 65; %i++) { $grants = $grants + %c.grant(\"g.x\" @ %i); }\n\
         return $roles SPC $grants;",
    );
    assert_eq!(answer, "32 64");
}

#[test]
fn what_a_connection_held_under_its_own_key_goes_when_it_ends() {
    let mut client = connected();
    eval(
        &mut client,
        "%c = ClientGroup.getObject(0); definePermissionRole(\"builder\", \"props.*\"); %c.addRole(\"builder\");",
    );
    let perms = client
        .server
        .as_ref()
        .unwrap()
        .vm()
        .extension::<Permissions>()
        .expect("the server has permissions");
    // The example's clients sign in with no identity, so they are held by the connection.
    let key = (0..64)
        .map(SubjectKey::Connection)
        .find(|k| perms.has_role(k, "builder"))
        .expect("the connection holds the role");
    client.call("disconnect", &[]).unwrap();
    for _ in 0..200 {
        client.step(32);
    }
    assert!(
        !perms.has_role(&key, "builder"),
        "forgotten with the connection"
    );
}
