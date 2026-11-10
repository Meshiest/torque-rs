//! A predicting client rides a deck hung on a moving lift: its prediction meets the deck where the server's latest pose of
//! the lift and the deck's local pose put it, so it is corrected no more often than when the same deck moves by itself along
//! the same path (torque-trace-hash's `hierarchy::ride`, over a link that holds each datagram from the server a few ticks).

use torque_trace_hash::hierarchy::{Deck, ride};

/// Steps each datagram from the server waits: the client's moves pile up unacknowledged and a correction replays them.
const LATENCIES: [u32; 2] = [0, 3];

#[test]
fn a_hung_deck_corrects_its_rider_no_more_than_the_same_deck_moved_as_a_root() {
    for latency in LATENCIES {
        let hung = ride(Deck::Hung, latency).expect("the hung ride");
        let root = ride(Deck::Root(&hung.path), latency).expect("the root ride");
        let still = ride(Deck::Still, latency).expect("the still ride");
        println!(
            "latency {latency}: {} corrections hung, {} as a root, {} still; the hung deck placed within {} units",
            hung.corrections, root.corrections, still.corrections, hung.placed_error
        );
        assert!(
            hung.aboard && root.aboard && still.aboard,
            "the figure fell off the deck at latency {latency}"
        );
        // The figure's own moves, jumps and turns on a deck that never moves are predicted exactly: the corrections of a
        // moving deck are its motion's, which the client holds at the latest record through each replay.
        assert_eq!(
            still.corrections, 0,
            "latency {latency}: corrections on a deck that never moves"
        );
        assert_eq!(
            hung.path, root.path,
            "the root deck left the hung deck's path at latency {latency}"
        );
        assert!(
            hung.placed_error <= 1e-3,
            "the client placed the hung deck {} units from the server's pose at latency {latency}",
            hung.placed_error
        );
        assert!(
            hung.corrections <= root.corrections,
            "latency {latency}: {} corrections hung, {} as a root",
            hung.corrections,
            root.corrections
        );
    }
}
