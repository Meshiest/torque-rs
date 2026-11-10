//! The console side: TCPObject's and HTTPObject's classes, their behaviours (`processArguments`, `onAdd`, the destructor's
//! disconnect) and their methods.

use std::rc::Rc;

use torque_ts_vm::classes::EngineClass;
use torque_ts_vm::effects::Effects;
use torque_ts_vm::sim::{ObjId, Objects, SimObject};
use torque_ts_vm::vm::{Native, NativeCall, Vm, VmError};

use crate::address::http_target;
use crate::host::{Tag, TcpHost};
use crate::http::Request;

/// A TCPObject's adoption of an accepted connection, between `processArguments` and `onAdd`.
struct Adopt(Tag);

/// TGE's `dAtoi` of an id: leading digits, 0 for none.
fn id_number(text: &[u8]) -> Tag {
    let text = text.trim_ascii_start();
    text.iter()
        .take_while(|b| b.is_ascii_digit())
        .fold(0u32, |n, d| {
            n.wrapping_mul(10).wrapping_add(u32::from(d - b'0'))
        })
}

/// TCPObject's behaviour: `processArguments`, `onAdd` and the destructor's disconnect.
struct TcpClass(Rc<TcpHost>);

impl EngineClass for TcpClass {
    fn process_arguments(&self, _: &Objects, object: &mut SimObject, args: &[Vec<u8>]) -> bool {
        let [id] = args else {
            return false;
        };
        // An `ext` a subclass's constructor set is kept: the object then takes no connection.
        if object.ext.is_none() {
            object.ext = Some(Box::new(Adopt(id_number(id))));
        }
        true
    }

    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        vm.link_name_namespace_always(id);
        if let Some(group) = vm.objects().find(b"TCPGroup") {
            vm.objects_mut().add(group, id);
        }
        let adopt = vm
            .objects_mut()
            .get_mut(id)
            .and_then(|o| o.ext.take_if(|ext| ext.is::<Adopt>()))
            .and_then(|ext| ext.downcast::<Adopt>().ok())
            .map(|adopt| adopt.0);
        if let Err(cap) = self.0.add_object(id, adopt) {
            vm.warn(format!(
                "TCPObject {id}: accepted connection refused: {cap} connections are open"
            ));
        }
        Ok(true)
    }

    fn on_remove(&self, _: &mut Vm, id: ObjId) -> Result<(), VmError> {
        self.0.remove_object(id);
        Ok(())
    }
}

/// HTTPObject's behaviour: its request state.
struct HttpClass(Rc<TcpHost>);

impl EngineClass for HttpClass {
    fn on_add(&self, _: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        self.0.make_http(id);
        Ok(true)
    }
}

/// Charges the bytes of a method's arguments; false when the run cannot pay.
fn charge_args(vm: &Vm, call: &NativeCall) -> bool {
    let bytes: usize = call.argv.iter().skip(2).map(Vec::len).sum();
    vm.charge_work(bytes as u64)
}

fn arg(call: &NativeCall, i: usize) -> &[u8] {
    call.argv.get(i).map_or(&[][..], Vec::as_slice)
}

/// Registers TCPObject and HTTPObject, their behaviours and methods in `vm`, with `host` as the engine's extension.
pub(crate) fn install(vm: &mut Vm, host: Rc<TcpHost>) {
    let Some(class) = vm.register_class_typed("TCPObject", "SimObject", Vec::new(), true) else {
        vm.error("TCPObject: not registered: the name is taken".to_string());
        return;
    };
    if let Some(def) = vm.objects_mut().classes.get_mut(class) {
        def.takes_arguments = true;
    }
    vm.set_class_behaviour("TCPObject", Rc::new(TcpClass(Rc::clone(&host))));
    if vm
        .register_class_typed("HTTPObject", "TCPObject", Vec::new(), true)
        .is_some()
    {
        vm.set_class_behaviour("HTTPObject", Rc::new(HttpClass(Rc::clone(&host))));
    }
    vm.set_extension(Rc::clone(&host));
    register_tcp_methods(vm, &host);
    register_http_methods(vm, &host);
}

fn register_tcp_methods(vm: &mut Vm, host: &Rc<TcpHost>) {
    let h = Rc::clone(host);
    vm.register_in(
        "TCPObject",
        "connect",
        3,
        3,
        "(string address) Connects to `[ip:]host[:port]` (port 28000 when none is given): onDNSResolved or onDNSFailed for a name, then onConnected or onConnectFailed.",
        move |vm, c| {
            if !charge_args(vm, c) {
                return Native::Fail(VmError::StepLimit);
            }
            if let Some(obj) = c.this_object {
                h.connect(vm, obj, arg(c, 2), false);
            }
            Native::Void
        },
    );
    let h = Rc::clone(host);
    vm.register_in(
        "TCPObject",
        "listen",
        3,
        3,
        "(int port) Listens for connections on that port: onConnectRequest(address, id) for each; `new TCPObject(name, id)` takes one.",
        move |vm, c| {
            if let Some(obj) = c.this_object {
                // TGE keeps a port number's low 16 bits (`U16(dAtoi(..))`).
                let port = (id_number(arg(c, 2)) & 0xFFFF) as u16;
                h.listen(vm, obj, port);
            }
            Native::Void
        },
    );
    let h = Rc::clone(host);
    vm.register_in(
        "TCPObject",
        "send",
        3,
        0,
        "(string text, ...) Sends each argument as it is, in order, on the open connection.",
        move |vm, c| {
            if !charge_args(vm, c) {
                return Native::Fail(VmError::StepLimit);
            }
            if let Some(obj) = c.this_object {
                for data in c.argv.iter().skip(2) {
                    h.queue(vm, obj, data);
                }
            }
            Native::Void
        },
    );
    let h = Rc::clone(host);
    vm.register_in(
        "TCPObject",
        "disconnect",
        2,
        2,
        "() Closes the connection, the listen socket or the connect under way, if any (no callback).",
        move |_, c| {
            if let Some(obj) = c.this_object {
                h.disconnect(obj);
            }
            Native::Void
        },
    );
    for name in ["connect", "listen", "send"] {
        vm.add_native_effects("TCPObject", name, Effects::NETWORK);
    }
}

fn register_http_methods(vm: &mut Vm, host: &Rc<TcpHost>) {
    let h = Rc::clone(host);
    vm.register_in(
        "HTTPObject",
        "get",
        4,
        5,
        "(string address, string path, string query) Sends a GET for path?query once connected (address as connect's, or http:// or https://); each line of the answer's body is onLine.",
        move |vm, c| {
            if !charge_args(vm, c) {
                return Native::Fail(VmError::StepLimit);
            }
            let Some(obj) = c.this_object else {
                return Native::Void;
            };
            let request = Request {
                host: host_header(arg(c, 2)),
                path: arg(c, 3).to_vec(),
                query: c.argv.get(4).cloned(),
                post: None,
            };
            h.request(vm, obj, arg(c, 2), request);
            Native::Void
        },
    );
    let h = Rc::clone(host);
    vm.register_in(
        "HTTPObject",
        "post",
        6,
        6,
        "(string address, string path, string query, string body) Sends a POST of the form body for path?query once connected; each line of the answer's body is onLine.",
        move |vm, c| {
            if !charge_args(vm, c) {
                return Native::Fail(VmError::StepLimit);
            }
            let Some(obj) = c.this_object else {
                return Native::Void;
            };
            let query = arg(c, 4);
            let request = Request {
                host: host_header(arg(c, 2)),
                path: arg(c, 3).to_vec(),
                query: (!query.is_empty()).then(|| query.to_vec()),
                post: Some(arg(c, 5).to_vec()),
            };
            h.request(vm, obj, arg(c, 2), request);
            Native::Void
        },
    );
    for name in ["get", "post"] {
        vm.add_native_effects("HTTPObject", name, Effects::NETWORK);
    }
}

/// The `Host` header of an HTTPObject's address: its host without its port.
fn host_header(address: &[u8]) -> String {
    http_target(&String::from_utf8_lossy(address)).map_or_else(String::new, |t| t.host.text())
}
