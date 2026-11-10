//! [`TcpHost`]: one script engine's TCPObjects, their sockets, and the pass that services them each tick ([`poll`]).
//!
//! An object holds at most one socket at a time (TGE's tag), found by its id. The pass takes the reports of the connect
//! threads, accepts on the listen sockets, and reads and splits what each open connection received, calling the script back
//! for each event as it comes; it holds no borrow while a script runs, and after each callback it looks again whether the
//! socket is still the object's (the script may have disconnected, connected elsewhere or deleted the object).

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};

use crate::address::{Host, address_text, http_target, tcp_target};
use crate::http::{Http, Request};
use crate::limits::{PASS_STEPS, PASS_WORK, TcpLimits};
use crate::lines::{Lines, TooLong};
use crate::link::{self, Connector, Link, Progress, would_block};

/// A socket's number, as `onConnectRequest` gives it and `new TCPObject(name, id)` takes it.
pub(crate) type Tag = u32;

/// The most bytes one read takes from a socket.
const READ_CHUNK: usize = 16 << 10;

/// The global a listen socket binds to when it is set (TGE's), else every interface.
const BIND_ADDRESS: &str = "$Pref::Net::BindAddress";

/// What one tick's pass did: what a host's tick hook prints, and what the tests look at.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct PassReport {
    /// Bytes read from sockets.
    pub read: usize,
    /// Bytes written to sockets.
    pub sent: usize,
    /// Script callbacks made.
    pub callbacks: usize,
    /// Whether the pass stopped at a cap or at its budget with work left for the next tick.
    pub stopped: bool,
    /// The errors the callbacks ended with.
    pub errors: Vec<VmError>,
    /// The script's own connections open or connecting when the pass ended (what [`TcpLimits::sockets`] caps).
    pub sockets: usize,
    /// Accepted connections waiting for a script to take them when the pass ended.
    pub waiting: usize,
}

/// A script callback.
enum Callback {
    DnsResolved,
    DnsFailed,
    Connected,
    ConnectFailed,
    Line(Vec<u8>),
    Disconnect,
    ConnectRequest { address: String, tag: Tag },
}

impl Callback {
    fn method(&self) -> &'static [u8] {
        match self {
            Callback::DnsResolved => b"onDNSResolved",
            Callback::DnsFailed => b"onDNSFailed",
            Callback::Connected => b"onConnected",
            Callback::ConnectFailed => b"onConnectFailed",
            Callback::Line(_) => b"onLine",
            Callback::Disconnect => b"onDisconnect",
            Callback::ConnectRequest { .. } => b"onConnectRequest",
        }
    }
}

/// A TCPObject's own state (an HTTPObject's has its request and reading too).
#[derive(Default)]
struct Obj {
    tag: Option<Tag>,
    lines: Lines,
    http: Option<Http>,
}

/// A connect on its thread.
struct Connecting {
    progress: Receiver<Progress>,
    deadline: Instant,
    /// The address was a name, so a failure before the lookup answered is a DNS failure.
    named: bool,
    resolved: bool,
}

/// An open connection.
struct Open {
    link: Box<Link>,
    /// What was read and is not split yet, from `at`.
    inbox: Vec<u8>,
    at: usize,
    /// What waits to be written.
    outbox: VecDeque<u8>,
    /// When it was accepted, while no script has taken it.
    accepted: Option<Instant>,
}

impl Open {
    fn new(link: Box<Link>, accepted: Option<Instant>) -> Open {
        Open {
            link,
            inbox: Vec::new(),
            at: 0,
            outbox: VecDeque::new(),
            accepted,
        }
    }

    /// Writes what waits, at most `allowance` bytes; how many went, or `Err` once the connection broke.
    fn flush(&mut self, allowance: usize) -> Result<usize, Broken> {
        let mut sent = 0;
        while sent < allowance && !self.outbox.is_empty() {
            let (front, _) = self.outbox.as_slices();
            let take = front.len().min(allowance - sent);
            match self.link.write(&front[..take]) {
                Ok(0) => return Err(Broken),
                Ok(n) => {
                    self.outbox.drain(..n);
                    sent += n;
                }
                Err(e) if would_block(&e) => break,
                Err(_) => return Err(Broken),
            }
        }
        Ok(sent)
    }

    fn close(mut self) {
        let _ = self.flush(self.outbox.len());
        self.link.close();
    }
}

enum State {
    Connecting(Connecting),
    Open(Open),
    Listening(TcpListener),
}

/// Which [`State`] a socket is in, read without holding the table.
#[derive(Clone, Copy)]
enum Kind {
    Connecting,
    Open,
    Listening,
}

impl State {
    fn kind(&self) -> Kind {
        match self {
            State::Connecting(_) => Kind::Connecting,
            State::Open(_) => Kind::Open,
            State::Listening(_) => Kind::Listening,
        }
    }
}

/// A connection that broke while it was written.
#[derive(Debug)]
struct Broken;

struct Conn {
    /// The object the socket is for: none for an accepted connection no script has taken yet.
    owner: Option<ObjId>,
    state: State,
}

impl Conn {
    fn close(self) {
        if let State::Open(open) = self.state {
            open.close();
        }
    }
}

#[derive(Default)]
struct Table {
    last_tag: Tag,
    conns: BTreeMap<Tag, Conn>,
    objects: HashMap<ObjId, Obj>,
    /// Callbacks due at the next pass (a connect refused by a cap).
    deferred: VecDeque<(ObjId, Callback)>,
    /// Bytes written in this tick.
    sent: usize,
    /// Where the next pass starts in the sockets, so no socket is always first.
    turn: usize,
}

impl Table {
    fn new_tag(&mut self) -> Tag {
        loop {
            self.last_tag = self.last_tag.wrapping_add(1).max(1);
            if !self.conns.contains_key(&self.last_tag) {
                return self.last_tag;
            }
        }
    }

    /// The script's own connections (connecting or open), accepted ones a script took included.
    fn sockets(&self) -> usize {
        self.conns
            .values()
            .filter(|c| c.owner.is_some() && !matches!(c.state, State::Listening(_)))
            .count()
    }

    /// Accepted connections no script has taken yet.
    fn unadopted(&self) -> usize {
        self.conns.values().filter(|c| c.owner.is_none()).count()
    }

    fn listeners(&self) -> usize {
        self.conns
            .values()
            .filter(|c| matches!(c.state, State::Listening(_)))
            .count()
    }

    fn lookups(&self) -> usize {
        self.conns
            .values()
            .filter(|c| matches!(c.state, State::Connecting(_)))
            .count()
    }

    /// Closes `obj`'s socket, if it has one (`disconnect`: no callback).
    fn disconnect(&mut self, obj: ObjId) {
        let Some(tag) = self.objects.get_mut(&obj).and_then(|o| o.tag.take()) else {
            return;
        };
        if let Some(conn) = self.conns.remove(&tag) {
            conn.close();
        }
    }

    /// The socket `tag` if it is still `obj`'s.
    fn owned(&mut self, tag: Tag, obj: ObjId) -> Option<&mut Conn> {
        self.conns.get_mut(&tag).filter(|c| c.owner == Some(obj))
    }

    /// Takes the socket `tag` away from its object once it ended.
    fn end(&mut self, tag: Tag) -> Option<ObjId> {
        let conn = self.conns.remove(&tag)?;
        let owner = conn.owner?;
        if let Some(obj) = self.objects.get_mut(&owner)
            && obj.tag == Some(tag)
        {
            obj.tag = None;
            if let Some(http) = &mut obj.http {
                http.end();
            }
        }
        conn.close();
        Some(owner)
    }
}

/// One step of splitting a connection's inbox.
enum Split {
    /// The socket is gone or no longer the object's.
    Gone,
    /// The inbox is used up.
    Empty,
    /// Bytes used, and the line they ended, if any.
    Used(usize, Option<Vec<u8>>),
    TooLong,
}

/// One read from a socket.
enum Read {
    Got(usize),
    NotNow,
    Ended,
}

/// One tick's counts against its caps.
struct Pass<'a> {
    limits: &'a TcpLimits,
    report: PassReport,
}

impl Pass<'_> {
    /// Whether the pass must stop: its callbacks, its callbacks' steps or its own work are spent.
    fn spent(&mut self, vm: &Vm) -> bool {
        let spent = self.report.callbacks >= self.limits.callbacks_per_tick
            || vm.host_run_steps() >= PASS_STEPS
            || !vm.charge_work(0);
        self.report.stopped |= spent;
        spent
    }
}

/// One script engine's TCPObjects: kept as the engine's extension and by the classes' behaviours.
pub(crate) struct TcpHost {
    limits: TcpLimits,
    connector: Connector,
    table: RefCell<Table>,
    /// This engine's connect threads still alive, a disconnected connect's included (its thread waits on the system).
    threads: Arc<AtomicUsize>,
}

impl TcpHost {
    pub(crate) fn new(limits: TcpLimits, connector: Connector) -> TcpHost {
        TcpHost {
            limits,
            connector,
            table: RefCell::new(Table::default()),
            threads: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Calls the script back; false once the object is gone.
    fn fire(&self, vm: &mut Vm, obj: ObjId, callback: Callback, pass: &mut Pass) -> bool {
        if !vm.objects().contains(obj) {
            return false;
        }
        pass.report.callbacks += 1;
        let method = callback.method();
        let connected = matches!(callback, Callback::Connected);
        let args = match callback {
            Callback::Line(line) => vec![line],
            Callback::ConnectRequest { address, tag } => {
                vec![address.into_bytes(), tag.to_string().into_bytes()]
            }
            _ => Vec::new(),
        };
        if let Err(e) = vm.call_method(obj, method, &args) {
            pass.report.errors.push(e);
        }
        // `HTTPObject::onConnected` sends its request after the script's `onConnected`.
        if connected {
            let request = {
                let mut table = self.table.borrow_mut();
                table
                    .objects
                    .get_mut(&obj)
                    .and_then(|o| o.http.as_mut())
                    .map(Http::begin)
            };
            if let Some(request) = request {
                self.queue(vm, obj, &request);
            }
        }
        vm.objects().contains(obj)
    }

    /// One tick: the callbacks a cap refused last tick, then every socket in turn. See [`poll`].
    fn pass(&self, vm: &mut Vm) -> PassReport {
        let mut pass = Pass {
            limits: &self.limits,
            report: PassReport::default(),
        };
        let tags: Vec<Tag> = {
            let mut table = self.table.borrow_mut();
            table.sent = 0;
            let tags: Vec<Tag> = table.conns.keys().copied().collect();
            table.turn = table.turn.wrapping_add(1);
            let start = if tags.is_empty() {
                0
            } else {
                table.turn % tags.len()
            };
            tags[start..]
                .iter()
                .chain(&tags[..start])
                .copied()
                .collect()
        };
        loop {
            if pass.spent(vm) {
                return pass.report;
            }
            let Some((obj, callback)) = self.table.borrow_mut().deferred.pop_front() else {
                break;
            };
            self.fire(vm, obj, callback, &mut pass);
        }
        for tag in tags {
            if pass.spent(vm) {
                break;
            }
            let found = self
                .table
                .borrow()
                .conns
                .get(&tag)
                .map(|c| (c.owner, c.state.kind()));
            match found {
                Some((Some(obj), Kind::Connecting)) => {
                    self.service_connecting(vm, tag, obj, &mut pass);
                }
                Some((Some(obj), Kind::Listening)) => {
                    self.service_listening(vm, tag, obj, &mut pass);
                }
                Some((Some(obj), Kind::Open)) => self.service_open(vm, tag, obj, &mut pass),
                Some((None, _)) => self.service_unadopted(tag),
                None => {}
            }
        }
        pass.report.sent = self.table.borrow().sent;
        pass.report.sockets = self.table.borrow().sockets();
        pass.report.waiting = self.table.borrow().unadopted();
        pass.report
    }

    fn service_connecting(&self, vm: &mut Vm, tag: Tag, obj: ObjId, pass: &mut Pass) {
        loop {
            let polled = {
                let mut table = self.table.borrow_mut();
                let Some(conn) = table.owned(tag, obj) else {
                    return;
                };
                let State::Connecting(connecting) = &mut conn.state else {
                    return;
                };
                let got = connecting.progress.try_recv();
                if matches!(got, Ok(Progress::Resolved)) {
                    connecting.resolved = true;
                }
                let failed_as = if connecting.named && !connecting.resolved {
                    Callback::DnsFailed
                } else {
                    Callback::ConnectFailed
                };
                match got {
                    Ok(Progress::Resolved) => Some(Callback::DnsResolved),
                    Ok(Progress::Connected(link)) => {
                        conn.state = State::Open(Open::new(link, None));
                        Some(Callback::Connected)
                    }
                    Ok(Progress::DnsFailed) => {
                        table.end(tag);
                        Some(Callback::DnsFailed)
                    }
                    Ok(Progress::ConnectFailed) | Err(TryRecvError::Disconnected) => {
                        table.end(tag);
                        Some(Callback::ConnectFailed)
                    }
                    Err(TryRecvError::Empty) if Instant::now() >= connecting.deadline => {
                        table.end(tag);
                        Some(failed_as)
                    }
                    Err(TryRecvError::Empty) => None,
                }
            };
            let Some(callback) = polled else {
                return;
            };
            let more = matches!(callback, Callback::DnsResolved);
            if !self.fire(vm, obj, callback, pass) || !more || pass.spent(vm) {
                return;
            }
        }
    }

    fn service_listening(&self, vm: &mut Vm, tag: Tag, obj: ObjId, pass: &mut Pass) {
        for _ in 0..self.limits.accepts_per_tick {
            if pass.spent(vm) {
                return;
            }
            let accepted = {
                let mut table = self.table.borrow_mut();
                // Waiting connections have their own cap, and with the script's own never pass `sockets` together.
                let full = table.unadopted() >= self.limits.unadopted
                    || table.sockets() + table.unadopted() >= self.limits.sockets;
                let Some(conn) = table.owned(tag, obj) else {
                    return;
                };
                let State::Listening(listener) = &conn.state else {
                    return;
                };
                let (stream, peer) = match listener.accept() {
                    Ok(accepted) => accepted,
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => return,
                };
                // Past the cap the peer is closed at once, so a flood of connects holds nothing.
                if full || stream.set_nonblocking(true).is_err() {
                    continue;
                }
                let _ = stream.set_nodelay(true);
                let new = table.new_tag();
                let open = Open::new(Box::new(Link::Plain(stream)), Some(Instant::now()));
                table.conns.insert(
                    new,
                    Conn {
                        owner: None,
                        state: State::Open(open),
                    },
                );
                (address_text(peer), new)
            };
            let (address, new) = accepted;
            if !self.fire(
                vm,
                obj,
                Callback::ConnectRequest { address, tag: new },
                pass,
            ) {
                return;
            }
        }
    }

    /// An accepted connection no script took: closed once [`TcpLimits::adopt_within`] passed. It is not read meanwhile.
    fn service_unadopted(&self, tag: Tag) {
        let mut table = self.table.borrow_mut();
        let expired = table.conns.get(&tag).is_some_and(|c| match &c.state {
            State::Open(open) => open
                .accepted
                .is_some_and(|at| at.elapsed() >= self.limits.adopt_within),
            _ => false,
        });
        if expired && let Some(conn) = table.conns.remove(&tag) {
            conn.close();
        }
    }

    fn service_open(&self, vm: &mut Vm, tag: Tag, obj: ObjId, pass: &mut Pass) {
        if self.flush(tag).is_err() {
            return self.ended(vm, tag, obj, pass);
        }
        let mut read_here = 0usize;
        loop {
            if pass.spent(vm) {
                return;
            }
            match self.split(tag, obj) {
                Split::Gone => return,
                Split::TooLong => {
                    vm.warn(format!(
                        "TCPObject {obj}: the peer sent a line longer than {} bytes; disconnected",
                        self.limits.line
                    ));
                    if let Some(o) = self.table.borrow_mut().objects.get_mut(&obj) {
                        o.lines.clear();
                    }
                    return self.ended(vm, tag, obj, pass);
                }
                Split::Used(used, line) => {
                    let _ = vm.charge_work(used as u64 + 1);
                    if let Some(line) = line
                        && !self.fire(vm, obj, Callback::Line(line), pass)
                    {
                        return;
                    }
                    continue;
                }
                Split::Empty => {}
            }
            let allowance = (self.limits.read_per_connection.saturating_sub(read_here))
                .min(self.limits.read_per_tick.saturating_sub(pass.report.read))
                .min(READ_CHUNK);
            if allowance == 0 {
                pass.report.stopped = true;
                return;
            }
            match self.read(tag, obj, allowance) {
                Read::Got(n) => {
                    read_here += n;
                    pass.report.read += n;
                    let _ = vm.charge_work(n as u64);
                }
                Read::NotNow => return,
                Read::Ended => return self.ended(vm, tag, obj, pass),
            }
        }
    }

    /// One `onReceive` over what waits in the inbox of `tag`.
    fn split(&self, tag: Tag, obj: ObjId) -> Split {
        let mut table = self.table.borrow_mut();
        let Table { conns, objects, .. } = &mut *table;
        let Some(Conn {
            owner: Some(owner),
            state: State::Open(open),
        }) = conns.get_mut(&tag)
        else {
            return Split::Gone;
        };
        let Some(o) = objects.get_mut(owner).filter(|_| *owner == obj) else {
            return Split::Gone;
        };
        let buffer = &open.inbox[open.at..];
        if buffer.is_empty() {
            return Split::Empty;
        }
        let max = self.limits.line;
        let got = match &mut o.http {
            Some(http) => http
                .receive(&mut o.lines, buffer, max)
                .map(|r| (r.used, r.line)),
            None => o.lines.parse_line(buffer, 0, max).map(|s| (s.next, s.line)),
        };
        match got {
            Ok((used, line)) => {
                open.at += used.min(buffer.len());
                if open.at >= open.inbox.len() {
                    open.inbox.clear();
                    open.at = 0;
                }
                Split::Used(used, line)
            }
            Err(TooLong) => Split::TooLong,
        }
    }

    fn read(&self, tag: Tag, obj: ObjId, allowance: usize) -> Read {
        let mut table = self.table.borrow_mut();
        let Some(Conn {
            state: State::Open(open),
            ..
        }) = table.owned(tag, obj)
        else {
            return Read::NotNow;
        };
        // Read straight into the inbox's tail (the inbox is empty here: it is read only once split).
        let held = open.inbox.len();
        open.inbox.resize(held + allowance, 0);
        let got = open.link.read(&mut open.inbox[held..]);
        open.inbox.truncate(held + got.as_ref().map_or(0, |n| *n));
        match got {
            Ok(0) => Read::Ended,
            Ok(n) => Read::Got(n),
            Err(e) if would_block(&e) => Read::NotNow,
            Err(_) => Read::Ended,
        }
    }

    /// The peer closed `tag` (or it broke): the waiting bytes are the last line, then `onDisconnect`.
    fn ended(&self, vm: &mut Vm, tag: Tag, obj: ObjId, pass: &mut Pass) {
        let last = {
            let mut table = self.table.borrow_mut();
            if table.owned(tag, obj).is_none() {
                return;
            }
            let last = table.objects.get_mut(&obj).and_then(|o| {
                let line = o.lines.finish_last_line()?;
                match &mut o.http {
                    Some(http) => http.process_line(&mut o.lines, line),
                    None => Some(line),
                }
            });
            table.end(tag);
            last
        };
        if let Some(line) = last
            && !self.fire(vm, obj, Callback::Line(line), pass)
        {
            return;
        }
        self.fire(vm, obj, Callback::Disconnect, pass);
    }

    /// Writes what waits on `tag` within the tick's allowance; `Err` once the connection broke.
    fn flush(&self, tag: Tag) -> Result<usize, Broken> {
        let mut table = self.table.borrow_mut();
        let allowance = self.limits.send_per_tick.saturating_sub(table.sent);
        let Some(Conn {
            state: State::Open(open),
            ..
        }) = table.conns.get_mut(&tag)
        else {
            return Ok(0);
        };
        let sent = open.flush(allowance)?;
        table.sent += sent;
        Ok(sent)
    }

    /// `send`: `data` joins the object's queue (refused past [`TcpLimits::send_queue`]) and goes out as far as the tick's
    /// allowance lets it. Nothing happens without an open connection.
    pub(crate) fn queue(&self, vm: &mut Vm, obj: ObjId, data: &[u8]) {
        let tag = {
            let mut table = self.table.borrow_mut();
            let Some(tag) = table.objects.get(&obj).and_then(|o| o.tag) else {
                return;
            };
            let Some(Conn {
                state: State::Open(open),
                ..
            }) = table.owned(tag, obj)
            else {
                return;
            };
            if open.outbox.len().saturating_add(data.len()) > self.limits.send_queue {
                drop(table);
                vm.warn(format!(
                    "TCPObject {obj}: send refused: more than {} bytes would wait to be sent",
                    self.limits.send_queue
                ));
                return;
            }
            open.outbox.extend(data);
            tag
        };
        // A broken connection is found by the next pass's read.
        let _ = self.flush(tag);
    }

    /// `connect` (and `get`, `post`): the object's socket closes, and a connect to `address` starts. A cap refuses it with a
    /// warning and `onConnectFailed` at the next pass; an address TGE opens no socket for does nothing.
    pub(crate) fn connect(&self, vm: &mut Vm, obj: ObjId, address: &[u8], http: bool) {
        let text = String::from_utf8_lossy(address);
        let target = if http {
            http_target(&text)
        } else {
            tcp_target(&text)
        };
        let mut table = self.table.borrow_mut();
        table.disconnect(obj);
        // A callback still due for an earlier connect of the object is stale now (and a loop of refused connects keeps one).
        table.deferred.retain(|(o, _)| *o != obj);
        let Some(target) = target else {
            return;
        };
        if target.host == Host::Unusable {
            table.deferred.push_back((obj, Callback::DnsFailed));
            return;
        }
        let threads = self.threads.load(Ordering::Acquire);
        let refused = if table.sockets() >= self.limits.sockets {
            Some(format!("{} connections are open", self.limits.sockets))
        } else if table.lookups().max(threads) >= self.limits.lookups {
            Some(format!("{} connects are under way", self.limits.lookups))
        } else {
            None
        };
        let named = matches!(target.host, Host::Name(_));
        let started = match refused {
            Some(why) => Err(why),
            None => link::start(
                &self.connector,
                target,
                self.limits.connect_within,
                Arc::clone(&self.threads),
            )
            .map_err(|_| "every connect thread is busy".to_string()),
        };
        match started {
            Ok(progress) => {
                let tag = table.new_tag();
                table.conns.insert(
                    tag,
                    Conn {
                        owner: Some(obj),
                        state: State::Connecting(Connecting {
                            progress,
                            deadline: Instant::now() + self.limits.connect_within,
                            named,
                            resolved: false,
                        }),
                    },
                );
                if let Some(o) = table.objects.get_mut(&obj) {
                    o.tag = Some(tag);
                }
            }
            Err(why) => {
                table.deferred.push_back((obj, Callback::ConnectFailed));
                drop(table);
                vm.warn(format!("TCPObject {obj}: connect refused: {why}"));
            }
        }
    }

    /// `listen`: the object's socket closes, and a listen socket on `port` opens (on `$Pref::Net::BindAddress` when that is
    /// set). A cap or a failed bind is a console error.
    pub(crate) fn listen(&self, vm: &mut Vm, obj: ObjId, port: u16) {
        let bind = vm
            .global(BIND_ADDRESS)
            .and_then(|text| {
                String::from_utf8_lossy(&text)
                    .trim()
                    .parse::<Ipv4Addr>()
                    .ok()
            })
            .unwrap_or(Ipv4Addr::UNSPECIFIED);
        let mut table = self.table.borrow_mut();
        table.disconnect(obj);
        let opened = if table.listeners() >= self.limits.listeners {
            Err(format!("{} listen sockets are open", self.limits.listeners))
        } else {
            TcpListener::bind(SocketAddr::from((bind, port)))
                .and_then(|l| l.set_nonblocking(true).map(|()| l))
                .map_err(|e| e.to_string())
        };
        match opened {
            Ok(listener) => {
                let tag = table.new_tag();
                table.conns.insert(
                    tag,
                    Conn {
                        owner: Some(obj),
                        state: State::Listening(listener),
                    },
                );
                if let Some(o) = table.objects.get_mut(&obj) {
                    o.tag = Some(tag);
                }
            }
            Err(why) => {
                drop(table);
                vm.error(format!(
                    "TCPObject {obj}: listen on port {port} failed: {why}"
                ));
            }
        }
    }

    /// A TCPObject registered: it takes the accepted connection `adopt` names when no script took it yet. Past
    /// [`TcpLimits::sockets`] the waiting connection is closed instead and the object has none; that refusal is the `Err`.
    pub(crate) fn add_object(&self, obj: ObjId, adopt: Option<Tag>) -> Result<(), usize> {
        let mut table = self.table.borrow_mut();
        let full = table.sockets() >= self.limits.sockets;
        let mut state = Obj::default();
        let mut refused = Ok(());
        if let Some(tag) = adopt
            && let Some(conn) = table.conns.get_mut(&tag)
            && conn.owner.is_none()
            && let State::Open(open) = &mut conn.state
        {
            if full {
                if let Some(conn) = table.conns.remove(&tag) {
                    conn.close();
                }
                refused = Err(self.limits.sockets);
            } else {
                conn.owner = Some(obj);
                open.accepted = None;
                state.tag = Some(tag);
            }
        }
        table.objects.insert(obj, state);
        refused
    }

    pub(crate) fn make_http(&self, obj: ObjId) {
        if let Some(o) = self.table.borrow_mut().objects.get_mut(&obj) {
            o.http = Some(Http::default());
        }
    }

    /// `disconnect`: closes the object's socket, with no callback.
    pub(crate) fn disconnect(&self, obj: ObjId) {
        self.table.borrow_mut().disconnect(obj);
    }

    pub(crate) fn remove_object(&self, obj: ObjId) {
        let mut table = self.table.borrow_mut();
        table.disconnect(obj);
        table.objects.remove(&obj);
        table.deferred.retain(|(o, _)| *o != obj);
    }

    /// `get` and `post`: the request is kept for `onConnected`, then the connect starts.
    pub(crate) fn request(&self, vm: &mut Vm, obj: ObjId, address: &[u8], request: Request) {
        if let Some(http) = self
            .table
            .borrow_mut()
            .objects
            .get_mut(&obj)
            .and_then(|o| o.http.as_mut())
        {
            http.request = Some(request);
        }
        self.connect(vm, obj, address, true);
    }
}

/// Services every TCPObject and HTTPObject of `vm` once (what each server and client tick does, through the plugin's tick
/// hooks): the reports of the connects, the listen sockets' new connections, and what each open connection received, split
/// into lines for the scripts' callbacks, in that order, starting at another socket each tick. The pass is bounded: it reads
/// at most [`TcpLimits::read_per_tick`] (and [`TcpLimits::read_per_connection`] from one socket), makes at most
/// [`TcpLimits::callbacks_per_tick`] callbacks, whose scripts take at most [`PASS_STEPS`] steps together, and does at most
/// [`PASS_WORK`] work outside them; what it leaves waits for the next tick. A script engine without the plugin does nothing.
pub fn poll(vm: &mut Vm) -> PassReport {
    let Some(host) = vm.extension::<TcpHost>() else {
        return PassReport::default();
    };
    let saved = vm.begin_host_budget_with_runs(PASS_WORK, PASS_STEPS);
    let report = host.pass(vm);
    vm.end_host_budget(saved);
    report
}
