//! The packet numbers the layers above a session key their records by: wide, in send order, with each packet's fate handed back
//! oldest first.
//!
//! A session numbers its packets with 16 bits and resolves them as acks come: a newer packet may be known to have arrived
//! before an older one is known lost. The event queues, the ghost managers and the channels key what they wrote into a packet
//! by a 32-bit number and expect the fates in send order, as TGE's packet layer gives them. [`PacketOrder`] gives each packet a
//! stream lane wrote into a new 32-bit number ([`PacketOrder::sent`]), keeps the fates the session reports
//! ([`PacketOrder::resolve`], [`PacketOrder::unsent`]) and hands them back in send order ([`PacketOrder::pop`]). It holds at
//! most [`MAX_UNRESOLVED`] unresolved packets while the host pops after each packet: past that the oldest is counted lost, which
//! a session that resolves every packet within its window never reaches.

use std::collections::VecDeque;

/// The packets a [`PacketOrder`] holds unresolved; a session resolves every packet within its window of 64.
pub const MAX_UNRESOLVED: usize = 256;

#[derive(Debug, Clone, Copy)]
struct Sent {
    seq: u16,
    wide: u32,
    fate: Option<bool>,
}

/// One side's packet numbers for the layers above its session (module docs).
#[derive(Debug, Clone)]
pub struct PacketOrder {
    next: u32,
    sent: VecDeque<Sent>,
}

impl Default for PacketOrder {
    fn default() -> PacketOrder {
        PacketOrder {
            next: 1,
            sent: VecDeque::new(),
        }
    }
}

impl PacketOrder {
    /// The wide number of the packet the session is building as `seq`, whose stream lane is being written. A packet that was
    /// not sent after all ([`PacketOrder::unsent`]) and is built again gets a new number.
    pub fn sent(&mut self, seq: u16) -> u32 {
        if self.sent.len() >= MAX_UNRESOLVED
            && let Some(oldest) = self.sent.iter_mut().find(|p| p.fate.is_none())
        {
            oldest.fate = Some(false);
        }
        let wide = self.next;
        self.next = self.next.wrapping_add(1);
        self.sent.push_back(Sent {
            seq,
            wide,
            fate: None,
        });
        wide
    }

    /// The packet the session built as `seq` was not sent (the link had no room): it is lost.
    pub fn unsent(&mut self, seq: u16) {
        if let Some(p) = self
            .sent
            .iter_mut()
            .rev()
            .find(|p| p.seq == seq && p.fate.is_none())
        {
            p.fate = Some(false);
        }
    }

    /// The session's notify: packet `seq` arrived or was lost. The oldest unresolved packet of that number takes it; one the
    /// order does not hold is ignored.
    pub fn resolve(&mut self, seq: u16, delivered: bool) {
        if let Some(p) = self
            .sent
            .iter_mut()
            .find(|p| p.seq == seq && p.fate.is_none())
        {
            p.fate = Some(delivered);
        }
    }

    /// The next fate in send order (the wide number and whether it arrived), once every older packet has one.
    pub fn pop(&mut self) -> Option<(u32, bool)> {
        let fate = self.sent.front()?.fate?;
        let front = self.sent.pop_front()?;
        Some((front.wide, fate))
    }

    /// Packets written and not yet handed back.
    #[must_use]
    pub fn unresolved(&self) -> usize {
        self.sent.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(order: &mut PacketOrder) -> Vec<(u32, bool)> {
        std::iter::from_fn(|| order.pop()).collect()
    }

    #[test]
    fn fates_come_back_in_send_order_whatever_order_they_resolve_in() {
        let mut order = PacketOrder::default();
        let wides: Vec<u32> = (10..14).map(|seq| order.sent(seq)).collect();
        assert_eq!(wides, [1, 2, 3, 4]);
        order.resolve(12, true);
        order.resolve(13, true);
        assert_eq!(order.pop(), None, "packet 10 is still in flight");
        order.resolve(10, false);
        assert_eq!(drain(&mut order), [(1, false)]);
        order.resolve(11, true);
        assert_eq!(drain(&mut order), [(2, true), (3, true), (4, true)]);
        assert_eq!(order.unresolved(), 0);
    }

    #[test]
    fn a_packet_built_again_after_no_room_gets_a_new_number() {
        let mut order = PacketOrder::default();
        assert_eq!(order.sent(5), 1);
        order.unsent(5);
        assert_eq!(order.sent(5), 2);
        order.resolve(5, true);
        assert_eq!(drain(&mut order), [(1, false), (2, true)]);
    }

    #[test]
    fn unknown_and_repeated_fates_are_ignored_and_the_oldest_past_the_cap_count_lost() {
        let mut order = PacketOrder::default();
        order.resolve(9, true);
        assert_eq!(order.pop(), None);
        let first = order.sent(0);
        order.resolve(0, true);
        order.resolve(0, false);
        assert_eq!(drain(&mut order), [(first, true)]);
        for seq in 0..(MAX_UNRESOLVED as u16 * 2) {
            order.sent(seq);
        }
        assert!(order.unresolved() <= MAX_UNRESOLVED * 2);
        let lost = drain(&mut order);
        assert_eq!(
            lost.len(),
            MAX_UNRESOLVED,
            "the oldest past the cap count lost"
        );
        assert!(lost.iter().all(|&(_, arrived)| !arrived));
    }
}
