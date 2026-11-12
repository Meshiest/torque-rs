use torque_net::bits::{BitReader, BitWriter};
use torque_testkit::Rng;

use super::*;

const ON: BlockSections = BlockSections { links: true };

fn round_trip(block: &GhostBlock, ghost_bits: u8) -> (GhostBlock, usize) {
    let mut w = BitWriter::new();
    block.write(&mut w, ON, ghost_bits);
    let bits = w.bits();
    let bytes = w.into_bytes();
    let mut r = BitReader::with_bit_len(&bytes, bits);
    let read = GhostBlock::read(&mut r, ON, ghost_bits, &mut |_| true).expect("a block");
    assert_eq!(r.bits_left(), 0, "the block read what it wrote");
    (read, bits)
}

fn pose(snap: bool, scale: Option<[u16; 3]>) -> PoseField {
    PoseField {
        translation: [10_240_000, -10_240_000, 1],
        dropped: 3,
        parts: [0, 32_766, 16_383],
        scale,
        snap,
    }
}

#[test]
fn every_form_of_the_block_reads_back_as_written() {
    let blocks = [
        GhostBlock::default(),
        GhostBlock {
            link: Some(LinkField::Root),
            pose: None,
        },
        GhostBlock {
            link: Some(LinkField::Waiting),
            pose: Some(pose(true, None)),
        },
        GhostBlock {
            link: Some(LinkField::Parent {
                ghost: 4095,
                node: Some(u16::MAX),
            }),
            pose: Some(pose(false, Some([1, 2, u16::MAX]))),
        },
        GhostBlock {
            link: Some(LinkField::Parent {
                ghost: 0,
                node: None,
            }),
            pose: None,
        },
    ];
    for block in &blocks {
        let (read, bits) = round_trip(block, 12);
        assert_eq!(&read, block);
        assert!(bits <= MAX_BLOCK_BITS);
    }
    // Nothing sent costs two bits; a block that is off costs none.
    assert_eq!(round_trip(&GhostBlock::default(), 12).1, 2);
    let mut w = BitWriter::new();
    blocks[3].write(&mut w, BlockSections::default(), 12);
    assert_eq!(w.bits(), 0);
}

#[test]
fn the_widest_block_fits_its_bound() {
    let block = GhostBlock {
        link: Some(LinkField::Parent {
            ghost: u32::MAX,
            node: Some(1),
        }),
        pose: Some(pose(true, Some([7; 3]))),
    };
    let (read, bits) = round_trip(&block, 32);
    assert_eq!(read, block);
    assert_eq!(bits, MAX_BLOCK_BITS);
}

#[test]
fn hostile_bytes_never_panic_and_stay_within_the_bound() {
    let mut rng = Rng::new(0x5eed_b10c);
    for _ in 0..20_000 {
        let len = rng.index(40);
        let bytes: Vec<u8> = (0..len).map(|_| rng.byte()).collect();
        let ghost_bits = (rng.below(33)) as u8;
        let mut r = BitReader::new(&bytes);
        let start = r.position();
        if GhostBlock::read(&mut r, ON, ghost_bits, &mut |_| true).is_ok() {
            assert!(r.position() - start <= MAX_BLOCK_BITS);
        }
    }
}

#[test]
fn a_spent_budget_stops_the_read() {
    let mut w = BitWriter::new();
    GhostBlock::default().write(&mut w, ON, 12);
    let bytes = w.into_bytes();
    let mut r = BitReader::new(&bytes);
    assert!(GhostBlock::read(&mut r, ON, 12, &mut |_| false).is_err());
}
