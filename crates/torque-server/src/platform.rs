//! The machine and the network a host's natives report (the game folder, the memory, the operating system, `setNetPort`'s
//! bind). The host fills them in ([`Platform::over`]).

use std::path::Path;

/// Where `setNetPort` binds. The default, [`NoNetwork`], binds nothing and says yes, so the scripts go on as on a machine whose
/// port is free. A server with a socket binds through [`TransportNetwork`](crate::net::transport::TransportNetwork).
pub trait Network {
    /// Binds the server's UDP port. `address` is `$Pref::Net::BindAddress` as given (empty for any address, which also stands
    /// for an address [`inet_addr`] refuses); `port` is already inside the range a server binds (1024 to 65535). False when the
    /// bind fails (the port is in use): the native then prints its failure line and runs the host's failure callback.
    fn bind(&self, address: &[u8], port: u16) -> bool;

    /// The one address a bind with an empty `address` really uses, as text (a host bound to one interface): `setNetPort` prints
    /// it in its `Binding server port to ...` line, as it prints a set `$Pref::Net::BindAddress`, instead of `default IP` while
    /// the socket is not on every interface. None (the default) for a network that binds nowhere in particular.
    fn fixed_address(&self) -> Option<String> {
        None
    }

    /// The number `setNetPort`'s `UDP initialized on port <n>` line names after a bind that asked `asked`: the asked one,
    /// unless the host bound another ([`crate::net::transport::PortChoice::OffBand`], a harness's server off a band of ports),
    /// which it names.
    fn shown_port(&self, asked: u16) -> u16 {
        asked
    }
}

/// Binds nothing; every port is "free".
#[derive(Debug, Default, Clone, Copy)]
pub struct NoNetwork;

impl Network for NoNetwork {
    fn bind(&self, _address: &[u8], _port: u16) -> bool {
        true
    }
}

/// The machine as a host's natives report it.
pub struct Platform {
    /// The game folder with `/` separators and no trailing one.
    pub module_directory: String,
    /// The folder a profile's files go to: the game folder unless the host says otherwise.
    pub profile_path: String,
    /// The total memory, in MB.
    pub total_ram_mb: u64,
    /// The operating system's long description.
    pub os_long: String,
    pub network: Box<dyn Network>,
}

impl Platform {
    /// A machine over the game folder `root`: the folder's path with `/` separators and no trailing one is the module directory
    /// and the profile path; no network.
    pub fn over(root: impl AsRef<Path>, total_ram_mb: u64, os_long: impl Into<String>) -> Platform {
        let shown = root.as_ref().to_string_lossy().replace('\\', "/");
        let folder = shown
            .strip_suffix('/')
            .filter(|f| !f.is_empty() && !f.ends_with(':'))
            .unwrap_or(&shown)
            .to_string();
        Platform {
            module_directory: folder.clone(),
            profile_path: folder,
            total_ram_mb,
            os_long: os_long.into(),
            network: Box::new(NoNetwork),
        }
    }
}

/// WinSock's `inet_addr` on `$Pref::Net::BindAddress`: the address in host order, or None for `INADDR_NONE` (a text it cannot
/// parse, and also `255.255.255.255`, which is the same bits). The classic rules: one to four parts separated by `.`, each
/// decimal, octal (a leading `0`) or hex (`0x`); the first parts are bytes, the last fills the remaining bytes; nothing else,
/// not even white space, may follow.
pub fn inet_addr(text: &[u8]) -> Option<u32> {
    let mut values = [0u64; 4];
    let mut count = 0;
    for part in text.split(|b| *b == b'.') {
        *values.get_mut(count)? = number(part)?;
        count += 1;
    }
    let (last, bytes) = values.get(..count)?.split_last()?;
    let mut address: u64 = 0;
    for byte in bytes {
        if *byte > 0xff {
            return None;
        }
        address = (address << 8) | byte;
    }
    // The last part fills the bytes the others did not.
    let left = 4 - bytes.len();
    if *last >= 1u64 << (8 * left) {
        return None;
    }
    address = (address << (8 * left)) | last;
    let address = u32::try_from(address).ok()?;
    (address != u32::MAX).then_some(address)
}

/// One part of an address: decimal, octal or hex, at least one digit, below 2^32.
fn number(part: &[u8]) -> Option<u64> {
    let (digits, radix) = match part {
        [b'0', b'x' | b'X', rest @ ..] => (rest, 16),
        [b'0', rest @ ..] if !rest.is_empty() => (rest, 8),
        _ => (part, 10),
    };
    if digits.is_empty() {
        return None;
    }
    let mut value: u64 = 0;
    for b in digits {
        let digit = u64::from(char::from(*b).to_digit(radix)?);
        value = value * u64::from(radix) + digit;
        if value > u64::from(u32::MAX) {
            return None;
        }
    }
    Some(value)
}
