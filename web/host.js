// The browser host (`?host=1`): the page opens a room on the room service, runs the game's server in a Web Worker
// (`host-worker.js`), plays in it over a local link, and holds a WebRTC peer connection for each player who joins with
// the room's code (browsers make no peer connection in a worker). Each joiner's data channel is passed to the worker as a
// link; the worker's relay transport holds every cap on what joiners send (rates and sizes in wire bytes, links in all,
// per address and a second), and the page closes whatever link it refuses or closes.
//
// The room service is `?rooms=<ws or wss URL>`, else the page's own host at `/ws`. Its protocol is torque-server's
// `net::room` (text in binary WebSocket messages); joiners' signaling is the data channels' (`net::web::rtc::signal`:
// `offer`, `answer`, `candidate`). The page's root element carries `data-torque-room` (the code, `ABCD-EFGH`) and
// `data-torque-server` (`worker` once the server runs there).
//
// Played alone (`solo`, a page `build.mjs --solo` marked): the server in the worker and the host player's own link, with no
// room service. Players invited by link (`invites.js`) join it through `onHost`'s `adopt`: their peer connections are
// made there, and each comes here with its channel open and the address its traffic comes from.
//
// Caps here, for what the page itself holds: at most MAX_PEERS peer connections (MAX_PEERS_PER_IP from one address),
// one offer each (an offer asking for anything but data channels is refused), MAX_CANDIDATES candidate lines each way,
// one channel each (label `game`, unordered, no retransmits), and a send skipped while a channel holds more than MAX_BUFFERED
// bytes. A joiner whose channel brings a message that is not binary, over MAX_DATAGRAM bytes, or more than
// MAX_MESSAGES_PER_S in a second is ended before the message is posted to the worker. A joiner's
// candidates, in its offer or trickled, go through `candidateAllowed` (the desktop host's rules): IPv4 UDP only, no `.local`
// names, never loopback, link-local, multicast, broadcast, reserved ranges or this page's own addresses, and private ones
// only with `?lan=1` (players on the host's network). A joiner reaches the host without them: its own checks to the
// host's candidates are answered.

const MAX_PEERS = 64;
// Peer connections from one address (the room service's word for it), as the desktop host's.
const MAX_PEERS_PER_IP = 4;
export const MAX_CANDIDATES = 16;
const MAX_BUFFERED = 256 * 1024;
const MAX_MESSAGE = 8 * 1024;
// A game datagram's most bytes, and the most a link sends a second (torque-net's NET_MAX_DATAGRAM and
// NET_PACKETS_PER_CONN_PER_S; the worker's relay counts them finer).
const MAX_DATAGRAM = 1500;
const MAX_MESSAGES_PER_S = 512;
const MAX_ICE_SERVERS = 8;
const PING_MS = 10_000;
// How long a joiner has from its offer to its channel's opening.
const CONNECT_MS = 20_000;

// The room code's form for people.
const show = (code) => `${code.slice(0, 4)}-${code.slice(4)}`;

// A peer number as the room service writes it: decimal, no leading zero, 1 or more (0 is the host's own link).
export function peerNumber(text) {
  return typeof text === 'string' && /^[1-9]\d{0,9}$/.test(text) && Number(text) <= 0xffffffff ? Number(text) : null;
}

// An IPv4 address in canonical dotted form (no leading zeros, each number 255 or less) as its four numbers, or null. A
// form another parser could read otherwise (`010.0.0.1`, `127.000.000.001`) is refused.
export function ipv4(text) {
  const m = /^(0|[1-9]\d{0,2})\.(0|[1-9]\d{0,2})\.(0|[1-9]\d{0,2})\.(0|[1-9]\d{0,2})$/.exec(text ?? '');
  if (!m) return null;
  const octets = m.slice(1).map(Number);
  return octets.every((o) => o <= 255) && octets.join('.') === text ? octets : null;
}

// A link's receiver: `onmessage` for a data channel whose messages go to `deliver(data)`, until one is not binary, is over
// MAX_DATAGRAM bytes, or is past MAX_MESSAGES_PER_S in its second (by `now()`, in milliseconds): then `end()` is called,
// once, and nothing more is delivered. The host's joiners and an invited joiner's channel to its host both take it.
export function meteredReceiver(deliver, end, now = () => performance.now()) {
  let second = -1;
  let count = 0;
  let ended = false;
  return (m) => {
    if (ended) return;
    const s = Math.floor(now() / 1000);
    if (s !== second) {
      second = s;
      count = 0;
    }
    if (!(m.data instanceof ArrayBuffer) || m.data.byteLength > MAX_DATAGRAM || ++count > MAX_MESSAGES_PER_S) {
      ended = true;
      return end();
    }
    deliver(m.data);
  };
}

// The link number for a new invited joiner, from `next` on: the first not in `taken` (1 up to 2^32 - 1; 0 is the host's
// own link), and the `next` to start from after it, so a number just freed is not taken again at once.
export function nextLinkId(taken, next) {
  let id = next >= 1 && next <= 0xffffffff ? next : 1;
  while (taken.has(id)) id = id >= 0xffffffff ? 1 : id + 1;
  return [id, id >= 0xffffffff ? 1 : id + 1];
}

// Whether a data channel may still carry a joiner: connecting or open (one that closed before it was handed over never
// fires its close again).
export const channelLive = (ch) => ch?.readyState === 'connecting' || ch?.readyState === 'open';

// Whether the host may check `octets` (a public unicast address, or a private one with `lan`): torque-server's
// `candidate_allowed`.
export function addressAllowed([a, b, c, d], lan) {
  const reserved =
    a === 0 ||
    a >= 224 ||
    a === 127 ||
    (a === 169 && b === 254) ||
    (a === 192 && b === 0 && (c === 0 || c === 2)) ||
    (a === 198 && b === 51 && c === 100) ||
    (a === 203 && b === 0 && c === 113) ||
    (a === 100 && b >= 64 && b < 128) ||
    (a === 198 && (b === 18 || b === 19)) ||
    (a === 255 && b === 255 && c === 255 && d === 255);
  if (reserved) return false;
  const priv = a === 10 || (a === 172 && b >= 16 && b < 32) || (a === 192 && b === 168);
  return lan || !priv;
}

// A browser's mDNS name for a host candidate: a version 4 UUID in lowercase hex, then `.local`, and nothing else.
export const MDNS_NAME = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\.local$/;

// A joiner's candidate line (with or without `a=`) the host may take, as `{ line, ip }`, or null: well formed, UDP, a
// canonical IPv4 address the rules allow and not one of `own`, a port written plainly. The line handed on is rebuilt
// from the address and port as read here, so the browser reads the address this checked. With `mdns` (an invite link's
// candidates, on any server; never a room's) a host candidate may instead name an MDNS_NAME not in `own`, which goes to
// the browser's own ICE as it is (the page never resolves it; the browser does, on its own network only); any other
// `.local` is refused.
export function candidateAllowed(text, lan, own = new Set(), { mdns = false } = {}) {
  if (typeof text !== 'string') return null;
  const line = text.trim().replace(/^a=/, '');
  if (!line.startsWith('candidate:') || line.length > 512) return null;
  const words = line.split(' ');
  if (words.length < 8 || words[6] !== 'typ') return null;
  const [, , transport, , address, port] = words;
  const p = /^[1-9]\d{0,4}$/.test(port) ? Number(port) : 0;
  if (transport.toLowerCase() !== 'udp' || p < 1 || p > 65535) return null;
  const oneName = words.filter((w) => w.includes('.local')).length === 1;
  if (mdns && words[7] === 'host' && MDNS_NAME.test(address) && oneName && !own.has(address)) {
    words[5] = String(p);
    return { line: words.join(' '), ip: address };
  }
  if (line.includes('.local')) return null;
  const octets = ipv4(address);
  if (!octets) return null;
  const ip = octets.join('.');
  if (!addressAllowed(octets, lan) || own.has(ip)) return null;
  words[4] = ip;
  words[5] = String(p);
  return { line: words.join(' '), ip };
}

// An offer without its candidate lines, and those lines (at most MAX_CANDIDATES * 4: each then goes through the rules).
export function splitCandidates(sdp) {
  const kept = [];
  const lines = [];
  for (const l of sdp.split(/\r?\n/)) {
    if (l.startsWith('a=candidate:')) {
      if (lines.length < MAX_CANDIDATES * 4) lines.push(l.slice(2));
    } else {
      kept.push(l);
    }
  }
  return [kept.join('\r\n'), lines];
}

// Whether an SDP offer asks for data channels and nothing else.
export function dataOnly(sdp) {
  const sections = sdp.split(/\r?\n/).filter((l) => l.startsWith('m='));
  return sections.length > 0 && sections.every((l) => l.startsWith('m=application '));
}

// The room's ICE servers (its JSON body) as `RTCPeerConnection` takes them: STUN and TURN URLs, TURN with a username and
// credential; anything else is left out.
export function iceServersFrom(body) {
  let parsed;
  try {
    parsed = JSON.parse(body);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  const servers = [];
  for (const s of parsed.slice(0, MAX_ICE_SERVERS)) {
    const urls = typeof s?.urls === 'string' ? [s.urls] : Array.isArray(s?.urls) ? s.urls : [];
    const good = urls.filter((u) => typeof u === 'string' && u.length <= 256 && /^(stun|turns?):/.test(u)).slice(0, 4);
    if (!good.length) continue;
    const server = { urls: good };
    if (good.some((u) => u.startsWith('turn'))) {
      if (typeof s.username !== 'string' || typeof s.credential !== 'string') continue;
      server.username = s.username.slice(0, 256);
      server.credential = s.credential.slice(0, 256);
    }
    servers.push(server);
  }
  return servers;
}

// Hosts the game: `game` is the module (its files fetched), `say(text)` shows the page's status line; `solo`: played alone,
// no room, and the host player's client is `client`'s: `solo` (`startSoloClient`), `host` (`startHostClient`), or null
// for a game already running that joins the server itself (a page played alone switching to hosting in place).
// `onHost({ adopt })`, called on a solo page once the server runs: `adopt(pc, channel, ip, onEnd)` takes an invited
// joiner's peer connection and its channel (open, or opening) from `ip` and returns null, or why it was refused; `onEnd()`
// is called when that joiner is ended. `wasm`: the module compiled already (the worker then does not compile it again).
// Returns `{ endPeers(), stop() }`: every joiner ended; and that, with the worker ended too (its game is over).
export async function hostGame(game, say, { solo = false, client = 'solo', onHost = null, wasm = null } = {}) {
  const root = document.documentElement;
  const params = new URLSearchParams(location.search);
  const lan = params.has('lan');
  const roomsUrl = params.get('rooms') ?? `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws`;
  if (!solo && !/^wss?:\/\//.test(roomsUrl)) throw new Error(`?rooms= takes a ws:// or wss:// address, not ${roomsUrl}`);
  const workerUrl = new URL('./host-worker.js', import.meta.url);
  if (params.get('map')) workerUrl.searchParams.set('map', params.get('map'));
  const worker = new Worker(workerUrl, { type: 'module' });
  try {
    worker.postMessage({ start: { wasm } });
  } catch {
    // a browser that cannot hand the compiled module over: the worker compiles its own
    worker.postMessage({ start: { wasm: null } });
  }
  const peers = new Map();
  // This page's own candidate addresses: a joiner may not name them.
  const own = new Set();
  let room = null;
  let iceServers = [];

  const tell = (text) => {
    if (room?.readyState === WebSocket.OPEN) room.send(new TextEncoder().encode(text));
  };

  // Ends peer `id`: its connection closed, the worker told (unless it closed the link itself), the room told.
  function end(id, tellWorker = true) {
    const peer = peers.get(id);
    if (!peer) return;
    peers.delete(id);
    peer.pc?.close();
    if (tellWorker && peer.linked) worker.postMessage({ close: id });
    tell(`drop ${id}`);
    peer.onEnd?.();
  }

  // Peer `id`'s data channel: open, it is a link to the worker; each message goes to the worker within the page's caps.
  function wire(id, peer, ch) {
    peer.channel = ch;
    ch.binaryType = 'arraybuffer';
    const open = () => {
      if (peer.linked || peers.get(id) !== peer) return;
      peer.linked = true;
      worker.postMessage({ open: id, ip: peer.ip });
    };
    ch.onopen = open;
    ch.onmessage = meteredReceiver(
      (data) => worker.postMessage({ from: id, data }, [data]),
      () => end(id),
    );
    ch.onclose = () => end(id);
    if (ch.readyState === 'open') open();
  }

  // Whether one more joiner from `ip` may come in: null, or why not.
  function refusal(ip) {
    if (peers.size >= MAX_PEERS) return 'The game is full.';
    if (!ipv4(ip)) return 'The player did not connect from an address this page takes.';
    if ([...peers.values()].filter((p) => p.ip === ip).length >= MAX_PEERS_PER_IP) {
      return `${MAX_PEERS_PER_IP} players from one address are in already.`;
    }
    return null;
  }

  // A joiner's candidate line: taken within its count and the rules, else dropped.
  function candidate(peer, text) {
    if (!peer.pc || peer.got++ >= MAX_CANDIDATES) return;
    const taken = candidateAllowed(text, lan, own);
    if (taken) peer.pc.addIceCandidate({ candidate: taken.line, sdpMid: '0' }).catch(() => {});
  }

  async function offer(id, peer, sdp) {
    if (peer.pc || !dataOnly(sdp)) return end(id);
    const [kept, offered] = splitCandidates(sdp);
    let pc;
    try {
      pc = new RTCPeerConnection({ iceServers });
    } catch {
      return end(id);
    }
    peer.pc = pc;
    setTimeout(() => {
      if (peers.get(id) === peer && !peer.linked) end(id);
    }, CONNECT_MS);
    pc.onicecandidate = (e) => {
      const line = e.candidate?.candidate;
      if (!line) return;
      const address = line.split(' ')[4];
      if (address) own.add(address);
      if (peer.sent++ < MAX_CANDIDATES) tell(`to ${id}\ncandidate\n${line}`);
    };
    pc.oniceconnectionstatechange = () => {
      // `disconnected` can pass (ICE moving to another pair); the channel's close ends a joiner that does not come back.
      if (['failed', 'closed'].includes(pc.iceConnectionState)) end(id);
    };
    pc.ondatachannel = (e) => {
      const ch = e.channel;
      if (peer.channel || ch.label !== 'game' || ch.ordered || ch.maxRetransmits !== 0) return end(id);
      wire(id, peer, ch);
    };
    try {
      await pc.setRemoteDescription({ type: 'offer', sdp: kept });
      for (const line of offered) candidate(peer, line);
      await pc.setLocalDescription(await pc.createAnswer());
      tell(`to ${id}\nanswer\n${pc.localDescription.sdp}`);
    } catch {
      end(id);
    }
  }

  function fromRoom(text) {
    const newline = text.indexOf('\n');
    const head = (newline < 0 ? text : text.slice(0, newline)).split(' ');
    const body = newline < 0 ? '' : text.slice(newline + 1);
    const [verb, a, b] = head;
    if (verb === 'room') {
      if (!/^[0-9A-Z]{8}$/.test(a ?? '')) return;
      root.dataset.torqueRoom = show(a);
      iceServers = iceServersFrom(body);
      return;
    }
    if (verb === 'expired') return console.warn('The room closed: no one else can join this game.');
    if (verb === 'refused') return console.warn(`The room service refused: ${a}`);
    const id = peerNumber(a);
    if (id === null) return;
    if (verb === 'peer') {
      if (refusal(b) !== null || peers.has(id)) return tell(`drop ${id}`);
      peers.set(id, { ip: b, pc: null, channel: null, linked: false, sent: 0, got: 0 });
    } else if (verb === 'from') {
      const peer = peers.get(id);
      if (!peer) return;
      const cut = body.indexOf('\n');
      const kind = cut < 0 ? body : body.slice(0, cut);
      const rest = cut < 0 ? '' : body.slice(cut + 1);
      if (kind === 'offer') offer(id, peer, rest);
      else if (kind === 'candidate' && rest) candidate(peer, rest);
      else if (kind === 'closed') end(id);
    } else if (verb === 'left') {
      // Its signaling is over. A joiner that offered is left to its connection's deadline (its channel may open on its
      // side, and its room socket close, before this page has the channel); one that never offered is done.
      if (!peers.get(id)?.pc) end(id);
    }
  }

  const started = new Promise((ok, fail) => {
    worker.onmessage = (e) => {
      if ('started' in e.data) ok(e.data.started);
      else if ('failed' in e.data) fail(new Error(`the server did not start: ${e.data.failed}`));
    };
    worker.onerror = (e) => fail(new Error(`the server's worker: ${e.message}`));
  });
  say('Starting the server...');
  try {
    await started;
  } catch (e) {
    worker.terminate();
    throw e;
  }
  root.dataset.torqueServer = 'worker';
  worker.onmessage = (e) => {
    const m = e.data;
    if ('to' in m) {
      if (m.to === 0) return game.localDatagram(new Uint8Array(m.data));
      const ch = peers.get(m.to)?.channel;
      if (ch?.readyState === 'open' && ch.bufferedAmount < MAX_BUFFERED) ch.send(m.data);
    } else if ('closed' in m) {
      end(m.closed, false);
    }
  };

  // The host player's own link: link 0, from the loopback address.
  worker.postMessage({ open: 0, ip: '127.0.0.1' });
  game.setLocalSender((bytes) => {
    const copy = bytes.slice();
    worker.postMessage({ from: 0, data: copy.buffer }, [copy.buffer]);
  });

  let ping = null;
  const endPeers = () => {
    for (const id of [...peers.keys()]) end(id);
  };
  const stop = () => {
    endPeers();
    worker.onmessage = null;
    worker.terminate();
    removeEventListener('pagehide', stop);
    clearInterval(ping);
    if (room) {
      room.onclose = null;
      room.close();
    }
    delete root.dataset.torqueServer;
  };
  const handle = { endPeers, stop };
  // The game ends for every joiner when this page goes.
  addEventListener('pagehide', stop);
  if (solo) {
    let next = 1;
    onHost?.({
      adopt(pc, ch, ip, onEnd) {
        const refused = refusal(ip);
        if (refused) return refused;
        if (!channelLive(ch)) return 'The player left before joining.';
        const [id, after] = nextLinkId(peers, next);
        next = after;
        const peer = { ip, pc, channel: null, linked: false, sent: 0, got: 0, onEnd };
        peers.set(id, peer);
        pc.oniceconnectionstatechange = () => {
          if (['failed', 'closed'].includes(pc.iceConnectionState)) end(id);
        };
        // An invited joiner makes no channel of its own.
        pc.ondatachannel = () => end(id);
        wire(id, peer, ch);
        return null;
      },
    });
    if (client === 'host') game.startHostClient();
    else if (client === 'solo') game.startSoloClient();
    return handle;
  }
  room = new WebSocket(roomsUrl);
  room.binaryType = 'arraybuffer';
  room.onopen = () => tell('host');
  room.onmessage = (e) => {
    if (e.data instanceof ArrayBuffer && e.data.byteLength <= MAX_MESSAGE) {
      fromRoom(new TextDecoder().decode(e.data));
    }
  };
  room.onclose = () => console.warn('The room service closed the link: no one else can join this game.');
  ping = setInterval(() => tell('ping'), PING_MS);
  game.startHostClient();
  return handle;
}
