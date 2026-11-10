//! The `net` tier's limits and names. The guest keeps these so a script gets a clear answer; the host's RPC registry
//! enforces its own (rates per RPC and per connection, sizes, outstanding calls, the timeout).

/// The most bytes of one `send`'s value, encoded.
pub const MAX_SEND_BYTES: usize = 4 << 10;
/// The most bytes of one `call`'s arguments, encoded.
pub const MAX_CALL_BYTES: usize = 4 << 10;
/// Sends and calls a second, together, once the burst is spent.
pub const SENDS_PER_SECOND: u32 = 32;
/// Sends and calls in a burst.
pub const SEND_BURST: u32 = 64;
/// Bytes of sends and calls a second (and the burst's bytes).
pub const SEND_BYTES_PER_SECOND: u32 = 64 << 10;
/// The most calls of one RPC waiting for their reply.
pub const MAX_OUTSTANDING: usize = 16;
/// The most calls of all RPCs together waiting for their reply.
pub const MAX_OUTSTANDING_TOTAL: usize = 64;
/// The longest RPC name.
pub const MAX_RPC_NAME: usize = 64;

/// An RPC name: 1 to [`MAX_RPC_NAME`] bytes, parts of letters, digits and `_` separated by `.`.
pub fn check_rpc_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty()
        || name.len() > MAX_RPC_NAME
        || !name.split('.').all(|part| {
            !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
    {
        return Err("an RPC name is parts of letters, digits and `_` separated by `.`");
    }
    Ok(())
}
