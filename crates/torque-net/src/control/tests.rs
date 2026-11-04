use super::*;
use crate::events::free_charge;
use crate::moves::Move;

const GB: u8 = 18;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        ((self.next() >> 32) as usize) % n.max(1)
    }
    fn f(&mut self) -> f32 {
        ((self.next() >> 40) as f32 - 8_388_608.0) / 64.0
    }
    fn p3(&mut self) -> [f32; 3] {
        [self.f(), self.f(), self.f()]
    }
    fn ghost(&mut self) -> u32 {
        (self.next() >> 20) as u32 & ((1 << GB) - 1)
    }
}

/// CRC-32 by the bit-at-a-time definition: the reflected polynomial 0xEDB88320, init 0xFFFFFFFF, no final xor. Independent of the
/// table in `crc.rs`.
fn bitwise_crc(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    crc
}

fn bytes_of<D: ControlData>(d: &D) -> (Vec<u8>, usize) {
    let mut w = BitWriter::new();
    d.write(&mut w, GB);
    let bits = w.bits();
    (w.into_bytes(), bits)
}

fn fly() -> CameraPacketData {
    CameraPacketData {
        energy: 0.5,
        recharge: 0.125,
        position: [1.0, 2.0, 3.0],
        pitch: 0.25,
        yaw: -1.5,
        mode: CameraMode::Fly,
    }
}

#[test]
fn the_checksum_of_a_hand_built_fly_camera_is_the_raw_crc_of_its_227_bits() {
    // Built by hand, with no use of `write`: energy 0.5, recharge 0.125, the position 1, 2, 3, pitch 0.25 and yaw -1.5 as raw
    // floats, then mode 2 in three bits.
    let mut w = BitWriter::new();
    for f in [0.5f32, 0.125, 1.0, 2.0, 3.0, 0.25, -1.5] {
        w.write_bits(f.to_bits(), 32);
    }
    w.write_bits(2, 3);
    assert_eq!(w.bits(), 227);
    let hand = w.into_bytes();
    assert_eq!(
        hand,
        [
            0, 0, 0, 0x3f, 0, 0, 0, 0x3e, 0, 0, 0x80, 0x3f, 0, 0, 0, 0x40, 0, 0, 0x40, 0x40, 0, 0,
            0x80, 0x3e, 0, 0, 0xc0, 0xbf, 0x02
        ]
    );
    let (written, bits) = bytes_of(&fly());
    assert_eq!((bits, &written), (227, &hand));
    // No final xor, init all ones.
    assert_eq!(bitwise_crc(&hand), 0x0a46_9b12);
    assert_eq!(fly().checksum(GB), 0x0a46_9b12);
    assert_eq!(checksum(&fly(), GB), bitwise_crc(&hand));
}

#[test]
fn the_checksum_follows_every_field_and_covers_the_whole_packet_data() {
    let base = fly().checksum(GB);
    let mut seen = vec![base];
    let mut tweak = |f: &dyn Fn(&mut CameraPacketData)| {
        let mut d = fly();
        f(&mut d);
        let c = d.checksum(GB);
        assert!(!seen.contains(&c));
        seen.push(c);
    };
    tweak(&|d| d.energy = 0.75);
    tweak(&|d| d.recharge = 0.5);
    tweak(&|d| d.position[2] = 3.5);
    tweak(&|d| d.pitch = 0.5);
    tweak(&|d| d.yaw = 0.5);
    tweak(&|d| d.mode = CameraMode::Stationary);
    // And the checksum is a function of the bytes only.
    let mut rng = Rng(u64::from(base) + 1);
    for _ in 0..200 {
        let d = random_data(&mut rng);
        let (bytes, bits) = bytes_of(&d);
        assert_eq!(d.checksum(GB), bitwise_crc(&bytes[..bits.div_ceil(8)]));
    }
}

fn random_mode(rng: &mut Rng) -> CameraMode {
    match rng.below(5) {
        0 => CameraMode::Stationary,
        1 => CameraMode::FreeRotate,
        2 => CameraMode::Fly,
        3 => CameraMode::Orbit {
            distances: rng.p3(),
            flag: rng.below(2) == 1,
            target: rng.ghost(),
        },
        _ => CameraMode::OrbitPoint {
            distances: rng.p3(),
            point: rng.p3(),
        },
    }
}

fn random_data(rng: &mut Rng) -> CameraPacketData {
    CameraPacketData {
        energy: rng.f(),
        recharge: rng.f(),
        position: rng.p3(),
        pitch: rng.f(),
        yaw: rng.f(),
        mode: random_mode(rng),
    }
}

#[test]
fn camera_packet_data_round_trips_in_every_mode() {
    let mut rng = Rng(0xabcdef);
    for _ in 0..3000 {
        let d = random_data(&mut rng);
        let (bytes, bits) = bytes_of(&d);
        let mut r = BitReader::with_bit_len(&bytes, bits);
        let back = CameraPacketData::read(&mut r, GB, &mut free_charge()).unwrap();
        assert_eq!(r.bits_left(), 0);
        // Mode 4's point is compressed (a lossy form): everything else must be equal, and the second write must equal the first.
        let (again, bits2) = bytes_of(&back);
        if !matches!(d.mode, CameraMode::OrbitPoint { .. }) {
            assert_eq!(back, d);
            assert_eq!((again, bits2), (bytes, bits));
        } else {
            assert_eq!(bits2, bits);
        }
    }
}

#[test]
fn mode_four_writes_the_point_compressed_at_scale_one_hundredth_against_the_camera() {
    let d = CameraPacketData {
        position: [10.0, 20.0, 30.0],
        mode: CameraMode::OrbitPoint {
            distances: [1.0, 2.0, 3.0],
            point: [10.5, 20.0, 29.0],
        },
        ..fly()
    };
    let (bytes, bits) = bytes_of(&d);
    // 64 (energy, recharge) + 96 + 64 + 3 + 96 (distances) + 2 + 3 * 16.
    assert_eq!(bits, 64 + 96 + 64 + 3 + 96 + 2 + 48);
    let mut r = BitReader::with_bit_len(&bytes, bits);
    let back = CameraPacketData::read(&mut r, GB, &mut free_charge()).unwrap();
    assert_eq!(
        back.mode,
        CameraMode::OrbitPoint {
            distances: [1.0, 2.0, 3.0],
            point: [10.5, 20.0, 29.0]
        }
    );
    // The writer's compression point is the camera position afterwards.
    let mut w = BitWriter::new();
    d.write(&mut w, GB);
    assert_eq!(w.compression_point(), [10.0, 20.0, 30.0]);
}

#[test]
fn mode_three_reads_its_target_index_whether_or_not_the_flag_is_set() {
    for flag in [false, true] {
        let d = CameraPacketData {
            mode: CameraMode::Orbit {
                distances: [1.0, 2.0, 3.0],
                flag,
                target: 262_143,
            },
            ..fly()
        };
        let (bytes, bits) = bytes_of(&d);
        assert_eq!(bits, 64 + 96 + 64 + 3 + 96 + 1 + 18);
        let mut r = BitReader::with_bit_len(&bytes, bits);
        assert_eq!(
            CameraPacketData::read(&mut r, GB, &mut free_charge()).unwrap(),
            d
        );
    }
}

#[test]
fn camera_modes_above_four_and_every_truncation_are_errors() {
    for mode in 5..8u32 {
        let mut w = BitWriter::new();
        for _ in 0..7 {
            w.write_u32(0);
        }
        w.write_bits(mode, 3);
        let mut r = BitReader::new(w.bytes());
        assert_eq!(
            CameraPacketData::read(&mut r, GB, &mut free_charge()),
            Err(NetError::Invalid("camera mode")),
            "mode {mode}"
        );
    }
    let d = CameraPacketData {
        mode: CameraMode::Orbit {
            distances: [1.0, 2.0, 3.0],
            flag: true,
            target: 5,
        },
        ..fly()
    };
    let (bytes, bits) = bytes_of(&d);
    for cut in 0..bits {
        let mut r = BitReader::with_bit_len(&bytes, cut);
        assert!(
            CameraPacketData::read(&mut r, GB, &mut free_charge()).is_err(),
            "cut {cut}"
        );
    }
    let mut r = BitReader::with_bit_len(&bytes, bits);
    assert_eq!(
        CameraPacketData::read(&mut r, GB, &mut |_| false),
        Err(NetError::Invalid("work budget"))
    );
}

fn head(control: ControlSection<CameraPacketData>) -> ServerHead<CameraPacketData> {
    ServerHead {
        move_ack: 0xdead_beef,
        damage: Damage::default(),
        control,
        camera_object: None,
        camera_data: None,
        first_person: None,
        fov: None,
    }
}

fn head_bits(h: &ServerHead<CameraPacketData>) -> usize {
    let mut w = BitWriter::new();
    h.write(&mut w, GB);
    w.bits()
}

#[test]
fn the_sections_are_37_283_and_134_bits_with_a_227_bit_fly_camera() {
    let none = head_bits(&head(ControlSection::None));
    assert_eq!(none, HEAD_MIN_BITS);
    // The mismatching section is 247 bits (flag + flag + index + 227), the matching one 98 (flag + flag + 96).
    let resend = head_bits(&head(ControlSection::Resend {
        index: 5,
        data: fly(),
    }));
    assert_eq!(resend - none + 1, 247);
    let matching = head_bits(&head(ControlSection::Matching {
        position: [1.0, 2.0, 3.0],
    }));
    assert_eq!(matching - none + 1, 98);
    // A camera object is its flag, its index and its packet data; a first-person setting two bits, as a fov nine.
    let mut h = head(ControlSection::None);
    h.camera_object = Some(3);
    h.camera_data = Some(fly());
    h.first_person = Some(false);
    h.fov = Some(90);
    assert_eq!(head_bits(&h), none + 18 + 227 + 1 + 8);
    // A camera object without its data is left out in Torque's form.
    h.camera_data = None;
    assert_eq!(head_bits(&h), none + 1 + 8);
}

#[test]
fn a_format_without_the_camera_data_and_the_first_person_pair_round_trips() {
    let format = ControlFormat {
        moves: crate::moves::MoveFormat { trigger_keys: 5 },
        first_person: false,
        camera_data: false,
    };
    let mut h = head(ControlSection::Resend {
        index: 5,
        data: fly(),
    });
    h.camera_object = Some(3);
    h.camera_data = Some(fly());
    h.first_person = Some(true);
    h.fov = Some(90);
    let mut w = BitWriter::new();
    h.write_with(&mut w, GB, format);
    // The ack, the damage flag, the resend (2 flags, the index, the data), the camera object's flag and index, the fov pair.
    assert_eq!(w.bits(), 32 + 1 + 2 + 18 + 227 + 1 + 18 + 9);
    let bits = w.bits();
    let bytes = w.into_bytes();
    let mut r = BitReader::with_bit_len(&bytes, bits);
    let back = ServerHead::read_in(
        &mut r,
        GB,
        format,
        &mut free_charge(),
        &mut |_, r, charge| CameraPacketData::read(r, GB, charge),
    )
    .unwrap();
    assert_eq!(r.bits_left(), 0);
    assert_eq!(back.camera_object, Some(3));
    assert_eq!((back.camera_data, back.first_person), (None, None));
    assert_eq!(back.control, h.control);
    // The client's block: five trigger flags, no first-person pair.
    let block = ClientBlock {
        checksum: 9,
        first_person: Some(false),
        fov: Some(80),
        ..ClientBlock::default()
    };
    let mut w = BitWriter::new();
    block.write_with(&mut w, format);
    let torque = {
        let mut t = BitWriter::new();
        block.write(&mut t);
        t.bits()
    };
    assert_eq!(w.bits() + 2, torque);
    let bytes = w.into_bytes();
    let back =
        ClientBlock::read_with(&mut BitReader::new(&bytes), format, &mut free_charge()).unwrap();
    assert_eq!(back.first_person, None);
    assert_eq!((back.checksum, back.fov), (9, Some(80)));
    // A state in that format never sends the first-person pair and reads blocks in it.
    let mut c = ControlState::with_format(format);
    c.set_first_person(false);
    let (h, _) = c.head(GB, None);
    assert_eq!(h.first_person, None);
    let (read, _) = c
        .read_client(&mut BitReader::new(&bytes), &mut free_charge())
        .unwrap();
    assert_eq!(read.fov, Some(80));
}

#[test]
fn a_server_head_round_trips_and_sets_the_readers_compression_point() {
    let mut rng = Rng(77);
    for _ in 0..2000 {
        let control = match rng.below(3) {
            0 => ControlSection::None,
            1 => ControlSection::Matching { position: rng.p3() },
            _ => ControlSection::Resend {
                index: rng.ghost(),
                data: CameraPacketData {
                    mode: if rng.below(2) == 0 {
                        CameraMode::Fly
                    } else {
                        CameraMode::Orbit {
                            distances: rng.p3(),
                            flag: true,
                            target: rng.ghost(),
                        }
                    },
                    ..random_data(&mut rng)
                },
            },
        };
        let camera_object = (rng.below(4) == 0).then(|| rng.ghost());
        let camera_data = camera_object.map(|_| CameraPacketData {
            mode: CameraMode::Stationary,
            ..random_data(&mut rng)
        });
        let h = ServerHead {
            move_ack: rng.next() as u32,
            damage: Damage {
                flash: (rng.below(2) == 1).then(|| rng.below(128) as u8),
                white_out: (rng.below(2) == 1).then(|| rng.below(128) as u8),
            },
            control,
            camera_object,
            camera_data,
            first_person: (rng.below(2) == 1).then(|| rng.below(2) == 1),
            fov: (rng.below(2) == 1).then(|| rng.below(256) as u8),
        };
        let mut w = BitWriter::new();
        h.write(&mut w, GB);
        let wcp = w.compression_point();
        let bits = w.bits();
        let bytes = w.into_bytes();
        let mut r = BitReader::with_bit_len(&bytes, bits);
        let back = ServerHead::read(&mut r, GB, &mut free_charge()).unwrap();
        assert_eq!(back, h);
        assert_eq!(r.bits_left(), 0);
        assert_eq!(
            r.compression_point().map(f32::to_bits),
            wcp.map(f32::to_bits)
        );
        let want = match (&h.camera_data, &h.control) {
            (Some(cam), _) => cam.position,
            (None, ControlSection::Matching { position }) => *position,
            (None, ControlSection::Resend { data, .. }) => data.position,
            (None, ControlSection::None) => [0.0; 3],
        };
        assert_eq!(wcp, want);
    }
}

#[test]
fn damage_levels_are_written_with_no_clamp_the_low_seven_bits_of_the_truncated_product() {
    // writeFloat(f, 7) = writeInt((S32)(127 * f), 7): no clamp, only the low 7 bits go out. A negative level wraps in the
    // two's complement and a level above 1 wraps modulo 128. (It is applied to the damage flash and to whiteOut / 1.5.)
    for (level, code) in [
        (1.0f32, 127u8),
        (0.5, 63),
        (0.999, 126),
        // -0.5 * 127 = -63.5 -> -63 = ...1000001 in the low 7 bits.
        (-0.5, 0x41),
        (-1.0, 1),
        // 2.0 * 127 = 254 -> 254 & 0x7f = 126.
        (2.0, 126),
        // 1.01 * 127 = 128.27 -> 128 -> 0.
        (1.01, 0),
        (3.0, 125),
        // NaN converts to 0; the infinities saturate (i32::MAX's low 7 bits are 127, i32::MIN's 0).
        (f32::NAN, 0),
        (f32::INFINITY, 127),
        (f32::NEG_INFINITY, 0),
    ] {
        assert_eq!(
            Damage::from_levels(level, 0.0).flash,
            Some(code),
            "flash {level}"
        );
    }
    // The white-out goes out as whiteOut / 1.5.
    assert_eq!(Damage::from_levels(0.0, 1.5).white_out, Some(127));
    assert_eq!(Damage::from_levels(0.0, -0.75).white_out, Some(0x41));
    assert_eq!(Damage::from_levels(0.0, 3.0).white_out, Some(126));
    // The wire carries the code as is, and the flag is set by "not zero", not by the code.
    let mut h = head(ControlSection::None);
    h.damage = Damage::from_levels(-0.5, 0.0);
    let mut w = BitWriter::new();
    h.write(&mut w, GB);
    let mut r = BitReader::new(w.bytes());
    r.read_u32().unwrap();
    assert_eq!(r.read_bits(2), Ok(0b11));
    assert_eq!(r.read_bits(7), Ok(0x41));
}

#[test]
fn the_damage_flash_is_two_optional_seven_bit_codes() {
    assert_eq!(Damage::from_levels(0.0, 0.0), Damage::default());
    // 0.5 * 127 = 63.5 -> 63; a white-out of 1.5 is sent as 1.5 / 1.5 = 1.0 -> 127.
    assert_eq!(
        Damage::from_levels(0.5, 1.5),
        Damage {
            flash: Some(63),
            white_out: Some(127)
        }
    );
    // A level that is not zero is flagged even when its code is 0.
    assert_eq!(Damage::from_levels(0.001, 0.0).flash, Some(0));
    for damage in [
        Damage::default(),
        Damage {
            flash: Some(3),
            white_out: None,
        },
        Damage {
            flash: None,
            white_out: Some(100),
        },
    ] {
        let mut h = head(ControlSection::None);
        h.damage = damage;
        let mut w = BitWriter::new();
        h.write(&mut w, GB);
        let want = HEAD_MIN_BITS
            + if damage == Damage::default() { 0 } else { 2 }
            + 7 * usize::from(damage.flash.is_some())
            + 7 * usize::from(damage.white_out.is_some());
        assert_eq!(w.bits(), want);
    }
}

struct Obj {
    index: Option<u32>,
    damage: Damage,
    state: CameraState,
}

impl ControlObject for Obj {
    fn ghost_index(&self) -> Option<u32> {
        self.index
    }
    fn damage(&self) -> Damage {
        self.damage
    }
    fn packet_data(&self) -> Box<dyn ControlData> {
        Box::new(self.state.resolve())
    }
}

fn obj(index: Option<u32>) -> Obj {
    Obj {
        index,
        damage: Damage::default(),
        state: CameraState {
            energy: 0.0,
            recharge: 0.0,
            position: [5.0, 6.0, 7.0],
            pitch: 0.0,
            yaw: 1.0,
            mode: ModeState::Fly,
        },
    }
}

#[test]
fn the_connection_resends_until_the_client_reports_the_checksum_then_sends_the_position() {
    let mut c = ControlState::new();
    let mut o = obj(Some(12));
    // A new connection's checksum is 0: the first section after the ack is a mismatch.
    let (h, step) = c.head(GB, Some(&o));
    assert_eq!(step, Step3::Resend);
    let ControlSection::Resend { index, data } = &h.control else {
        panic!("{h:?}");
    };
    assert_eq!(*index, 12);
    // The client adopts it and reports its checksum.
    c.client_checksum = checksum(data.as_ref(), GB);
    let (h, step) = c.head(GB, Some(&o));
    assert_eq!(step, Step3::Matching);
    assert_eq!(
        h.control,
        ControlSection::Matching {
            position: [5.0, 6.0, 7.0]
        }
    );
    // The camera moves: a mismatch again, with the new position.
    o.state.position[0] = 5.5;
    assert_eq!(c.head(GB, Some(&o)).1, Step3::Resend);
    // An unacked ghost and no control object write nothing.
    assert_eq!(c.head(GB, Some(&obj(None))).1, Step3::None);
    assert_eq!(c.head(GB, None).1, Step3::None);
    assert_eq!(
        c.stats,
        ControlStats {
            none: 2,
            matching: 1,
            resend: 2
        }
    );
    // The matching section leaves the compression point at the position.
    o.state.position[0] = 5.0;
    let mut w = BitWriter::new();
    c.write_head(&mut w, GB, Some(&o));
    assert_eq!(w.compression_point(), [5.0, 6.0, 7.0]);
}

/// A control chain top with a datablock range for its fov.
struct Fov {
    value: f32,
    min: f32,
    max: f32,
    sets: Vec<f32>,
}

impl Fov {
    fn new(value: f32) -> Fov {
        Fov {
            value,
            min: 1.0,
            max: 179.0,
            sets: Vec::new(),
        }
    }
}

impl FovTarget for Fov {
    fn set_camera_fov(&mut self, fov: f32) {
        self.sets.push(fov);
        self.value = fov.clamp(self.min, self.max);
    }
    fn camera_fov(&self) -> f32 {
        self.value
    }
}

#[test]
fn the_head_carries_the_ack_the_damage_and_one_fov_update() {
    let mut c = ControlState::new();
    let mut o = obj(Some(1));
    o.damage = Damage::from_levels(1.0, 0.0);
    assert_eq!(c.head(GB, Some(&o)).0.fov, None);
    let mut t = Fov::new(90.0);
    // 90 -> 120 changes the truncated value: armed, sent once as 120.
    assert!(c.set_control_camera_fov(120.0, Some(&mut t)));
    let (h, _) = c.head(GB, Some(&o));
    assert_eq!(h.fov, Some(120));
    assert_eq!(h.damage.flash, Some(127));
    assert_eq!(c.head(GB, Some(&o)).0.fov, None);
    // The ack is `lastMoveAck - list size`.
    let block = MoveBlock {
        start: 0,
        moves: vec![Move::default(); 4],
    };
    let mut w = BitWriter::new();
    ClientBlock {
        camera_flag: false,
        checksum: 0x1234,
        moves: block,
        first_person: None,
        fov: Some(80),
    }
    .write(&mut w);
    let mut r = BitReader::new(w.bytes());
    let (got, absorbed) = c.read_client(&mut r, &mut free_charge()).unwrap();
    assert_eq!(absorbed.appended, 4);
    assert_eq!((got.checksum, got.fov), (0x1234, Some(80)));
    assert_eq!(c.client_checksum, 0x1234);
    assert_eq!(c.head(GB, None).0.move_ack, 0);
    c.moves.clear_moves(4);
    assert_eq!(c.head(GB, None).0.move_ack, 4);
}

#[test]
fn set_control_camera_fov_does_nothing_without_a_control_object() {
    let mut c = ControlState::new();
    assert!(!c.set_control_camera_fov(120.0, None));
    assert!(!c.fov_pending());
    assert_eq!(c.fov(), 90.0);
    c.apply_client_fov(120, None);
    assert!(!c.fov_pending());
    assert_eq!(c.fov(), 90.0);
}

#[test]
fn set_control_camera_fov_clamps_and_arms_only_on_a_changed_integer() {
    // (request, what the object is told)
    for (req, told) in [
        (500.0f32, 179.0f32),
        (179.0, 179.0),
        (179.5, 179.0),
        (120.5, 120.5),
        (1.5, 1.5),
        (1.0, 1.0),
        (0.5, 1.0),
        (-3.0, 1.0),
        (f32::NEG_INFINITY, 1.0),
        (f32::INFINITY, 179.0),
        // A NaN is unordered in both compares and takes the `<= 1` branch.
        (f32::NAN, 1.0),
    ] {
        assert_eq!(clamp_fov(req), told, "{req}");
        let mut c = ControlState::new();
        let mut t = Fov::new(90.0);
        assert!(c.set_control_camera_fov(req, Some(&mut t)));
        assert_eq!(t.sets, [told], "{req}");
        assert_eq!(c.fov(), told);
        assert!(c.fov_pending(), "{req}: 90 -> {told}");
        let (h, _) = c.head(GB, None);
        assert_eq!(h.fov, Some(told as u8), "{req}");
    }
    // An unchanged integer part does not arm (the head sends flag(0), not nine more bits).
    for req in [90.0f32, 90.4, 90.99] {
        let mut c = ControlState::new();
        let mut t = Fov::new(90.0);
        c.set_control_camera_fov(req, Some(&mut t));
        assert!(!c.fov_pending(), "{req}");
        assert_eq!(c.fov(), req);
        let (h, _) = c.head(GB, None);
        assert_eq!(h.fov, None);
        let mut a = BitWriter::new();
        h.write(&mut a, GB);
        assert_eq!(a.bits(), HEAD_MIN_BITS);
    }
    // The stored value is the object's read-back: a datablock range of 20..=100 stores 100 for a request of 150.
    let mut c = ControlState::new();
    let mut t = Fov::new(90.0);
    t.min = 20.0;
    t.max = 100.0;
    c.set_control_camera_fov(150.0, Some(&mut t));
    assert_eq!((c.fov(), t.camera_fov()), (100.0, 100.0));
    assert_eq!(c.head(GB, None).0.fov, Some(100));
    // A request the object clamps back to the stored integer arms nothing.
    c.set_control_camera_fov(101.0, Some(&mut t));
    assert!(!c.fov_pending());
    // The armed flag stays until a head consumes it, whatever happens in between.
    c.set_control_camera_fov(30.0, Some(&mut t));
    c.set_control_camera_fov(31.0, Some(&mut t));
    assert_eq!(c.head(GB, None).0.fov, Some(31));
    assert_eq!(c.head(GB, None).0.fov, None);
}

#[test]
fn the_clients_fov_byte_is_clamped_applied_and_clears_the_update_flag_when_taken_whole() {
    // The client sends what the server already holds: the flag the server armed is cleared.
    let mut c = ControlState::new();
    let mut t = Fov::new(90.0);
    c.set_control_camera_fov(120.0, Some(&mut t));
    assert!(c.fov_pending());
    c.apply_client_fov(120, Some(&mut t));
    assert!(!c.fov_pending());
    assert_eq!(c.head(GB, None).0.fov, None);
    // The byte is clamped like the float: 255 gives 179, 0 gives 1.
    let mut c = ControlState::new();
    let mut t = Fov::new(90.0);
    c.apply_client_fov(255, Some(&mut t));
    assert_eq!((t.sets.clone(), c.fov()), (vec![179.0], 179.0));
    // The object took it as asked and the client has it: nothing to correct, so not pending although the integer changed.
    assert!(!c.fov_pending());
    c.apply_client_fov(0, Some(&mut t));
    assert_eq!((t.sets.last().copied(), c.fov()), (Some(1.0), 1.0));
    assert!(!c.fov_pending());
    // The object clamps to its datablock (20..=100): the client's 150 is told as 150, read back as 100; (int)100 != 150, so the update
    // flag stays armed and step 6 corrects the client.
    let mut c = ControlState::new();
    let mut t = Fov::new(90.0);
    t.min = 20.0;
    t.max = 100.0;
    c.apply_client_fov(150, Some(&mut t));
    assert_eq!(t.sets, [150.0]);
    assert_eq!(c.fov(), 100.0);
    assert!(c.fov_pending());
    assert_eq!(c.head(GB, None).0.fov, Some(100));
}

#[test]
fn credit_starts_at_thirty_and_is_capped() {
    let mut c = ControlState::new();
    assert_eq!(c.credit, 30);
    let mut credit = c.credit;
    c.moves.absorb(&MoveBlock {
        start: 0,
        moves: vec![Move::default(); 10],
    });
    let n = c.moves.take_for_tick(&mut credit).len();
    c.credit = credit;
    assert_eq!((n, c.credit), (10, 20));
    c.add_credit(5);
    assert_eq!(c.credit, 25);
    c.add_credit(1000);
    assert_eq!(c.credit, 30);
    c.add_credit(u32::MAX);
    assert_eq!(c.credit, 30);
}

fn target(ghost: Option<u32>) -> Target {
    Target {
        ghost,
        box_min: [0.0, 10.0, -4.0],
        box_max: [2.0, 20.0, 6.0],
    }
}

#[test]
fn an_orbit_camera_without_its_object_degrades_as_listed() {
    let state = |mode| CameraState {
        mode,
        ..obj(None).state
    };
    let d = [1.0, 2.0, 3.0];
    // Mode 3: a ghosted target stays; an unghosted one becomes mode 4 at the box centre; none becomes mode 2.
    assert_eq!(
        state(ModeState::Orbit {
            distances: d,
            flag: true,
            target: Some(target(Some(9)))
        })
        .resolve()
        .mode,
        CameraMode::Orbit {
            distances: d,
            flag: true,
            target: 9
        }
    );
    assert_eq!(
        state(ModeState::Orbit {
            distances: d,
            flag: true,
            target: Some(target(None))
        })
        .resolve()
        .mode,
        CameraMode::OrbitPoint {
            distances: d,
            point: [1.0, 15.0, 1.0]
        }
    );
    assert_eq!(
        state(ModeState::Orbit {
            distances: d,
            flag: true,
            target: None
        })
        .resolve()
        .mode,
        CameraMode::Fly
    );
    // The rest pass through, the ShapeBase part with them.
    let p = [4.0, 5.0, 6.0];
    assert_eq!(
        state(ModeState::OrbitPoint {
            distances: d,
            point: p
        })
        .resolve()
        .mode,
        CameraMode::OrbitPoint {
            distances: d,
            point: p
        }
    );
    assert_eq!(state(ModeState::Stationary).resolve().mode.number(), 0);
    assert_eq!(state(ModeState::FreeRotate).resolve().mode.number(), 1);
    let mut s = state(ModeState::Fly);
    s.recharge = 0.25;
    assert_eq!(s.resolve().recharge, 0.25);
}

#[test]
fn the_first_person_setting_goes_out_once_when_set_and_the_clients_is_taken_without_an_echo() {
    let mut c = ControlState::new();
    assert!(c.first_person() && !c.first_person_pending());
    assert_eq!(c.head(GB, None).0.first_person, None);
    c.set_first_person(false);
    assert!(c.first_person_pending());
    assert_eq!(c.head(GB, None).0.first_person, Some(false));
    assert_eq!(c.head(GB, None).0.first_person, None);
    // Set to what it is, it still goes out (the update is armed by the call).
    c.set_first_person(false);
    assert_eq!(c.head(GB, None).0.first_person, Some(false));
    // The client's setting is applied, and a pending one is not sent back.
    c.set_first_person(false);
    let mut w = BitWriter::new();
    ClientBlock {
        first_person: Some(true),
        ..ClientBlock::default()
    }
    .write(&mut w);
    let mut r = BitReader::new(w.bytes());
    let (got, _) = c.read_client(&mut r, &mut free_charge()).unwrap();
    assert_eq!(got.first_person, Some(true));
    assert!(c.first_person() && !c.first_person_pending());
    assert_eq!(c.head(GB, None).0.first_person, None);
}

#[test]
fn the_box_centre_is_the_sum_halved_and_stored_once() {
    let t = Target {
        ghost: None,
        box_min: [16_777_216.0, 0.1, -0.1],
        box_max: [1.0, 0.2, 0.3],
    };
    let c = t.centre();
    // 16777216 + 1 = 16777217 in f64, halved: 8388608.5, stored to f32: ties to even, 8388608.
    assert_eq!(c[0], 8_388_608.0);
    assert_eq!(c[1], ((f64::from(0.2f32) + f64::from(0.1f32)) * 0.5) as f32);
}

#[test]
fn a_client_block_round_trips_and_hostile_bytes_never_panic() {
    let mut rng = Rng(31337);
    for _ in 0..500 {
        let n = rng.below(32);
        let b = ClientBlock {
            camera_flag: rng.below(2) == 1,
            checksum: rng.next() as u32,
            moves: MoveBlock {
                start: rng.next() as u32,
                moves: (0..n)
                    .map(|_| {
                        Move::from_raw(
                            [
                                rng.below(33) as u32,
                                rng.below(33) as u32,
                                rng.below(33) as u32,
                            ],
                            [rng.below(65536) as u32, 0, rng.below(65536) as u32],
                            rng.below(2) == 1,
                            [rng.below(2) == 1; crate::moves::MAX_TRIGGER_KEYS],
                        )
                    })
                    .collect(),
            },
            first_person: (rng.below(2) == 1).then(|| rng.below(2) == 1),
            fov: (rng.below(2) == 1).then(|| rng.below(256) as u8),
        };
        let mut w = BitWriter::new();
        b.write(&mut w);
        let bits = w.bits();
        let bytes = w.into_bytes();
        let mut r = BitReader::with_bit_len(&bytes, bits);
        let back = ClientBlock::read(&mut r, &mut free_charge()).unwrap();
        let mut want = b.clone();
        want.moves.moves.truncate(31);
        assert_eq!(back, want);
        assert_eq!(r.bits_left(), 0);
    }
    // Hostile bytes into every reader.
    let mut c = ControlState::new();
    for _ in 0..8000 {
        let len = rng.below(400);
        let bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        let _ = ClientBlock::read(&mut BitReader::new(&bytes), &mut free_charge());
        let _ = c.read_client(&mut BitReader::new(&bytes), &mut free_charge());
        let _ = ServerHead::read(&mut BitReader::new(&bytes), GB, &mut free_charge());
        let _ = CameraPacketData::read(&mut BitReader::new(&bytes), GB, &mut free_charge());
        assert!(c.moves.len() <= crate::cap::NET_MOVE_LIST);
    }
    // Mutated valid heads.
    let h = head(ControlSection::Resend {
        index: 5,
        data: random_data(&mut rng),
    });
    let mut w = BitWriter::new();
    h.write(&mut w, GB);
    let bits = w.bits();
    let base = w.into_bytes();
    for _ in 0..5000 {
        let mut bytes = base.clone();
        for _ in 0..1 + rng.below(4) {
            let i = rng.below(bytes.len());
            bytes[i] ^= 1 << rng.below(8);
        }
        let mut r = BitReader::with_bit_len(&bytes, bits);
        let _ = ServerHead::read(&mut r, GB, &mut free_charge());
    }
}

// ---- a head over any ControlData, read by class ----

/// A control object of another class: one word and a position (the compression point is set to it first).
#[derive(Debug, Clone, PartialEq)]
struct Word {
    word: u32,
    position: [f32; 3],
}

impl ControlData for Word {
    fn position(&self) -> [f32; 3] {
        self.position
    }
    fn write(&self, w: &mut BitWriter, _ghost_bits: u8) {
        w.set_compression_point(self.position);
        w.write_u32(self.word);
        for c in self.position {
            w.write_f32(c);
        }
    }
    fn box_clone(&self) -> Box<dyn ControlData> {
        Box::new(self.clone())
    }
    fn same_as(&self, other: &dyn ControlData) -> bool {
        (other as &dyn std::any::Any)
            .downcast_ref::<Word>()
            .is_some_and(|o| o == self)
    }
}

/// Index 7 is a `Word`, everything else a Camera; counts its calls.
#[derive(Default)]
struct ByIndex {
    calls: usize,
}

impl ControlReader for ByIndex {
    fn read_control(
        &mut self,
        index: u32,
        r: &mut BitReader,
        ghost_bits: u8,
        charge: &mut ChargeFn,
    ) -> Result<Box<dyn ControlData>, NetError> {
        self.calls += 1;
        if index == 7 {
            let word = r.read_u32()?;
            let position = [r.read_f32()?, r.read_f32()?, r.read_f32()?];
            r.set_compression_point(position);
            Ok(Box::new(Word { word, position }))
        } else {
            CameraReader.read_control(index, r, ghost_bits, charge)
        }
    }
}

struct WordObj(Word);

impl ControlObject for WordObj {
    fn ghost_index(&self) -> Option<u32> {
        Some(7)
    }
    fn damage(&self) -> Damage {
        Damage::default()
    }
    fn packet_data(&self) -> Box<dyn ControlData> {
        Box::new(self.0.clone())
    }
}

#[test]
fn a_head_over_another_class_resends_its_data_and_reads_back_through_the_class_reader() {
    let word = Word {
        word: 0xdead_beef,
        position: [1.5, -2.0, 3.25],
    };
    let o = WordObj(word.clone());
    let mut c = ControlState::new();
    let (mut h, step) = c.head(GB, Some(&o));
    assert_eq!(step, Step3::Resend);
    // A camera object of the Camera class beside it.
    h.camera_object = Some(2);
    h.camera_data = Some(Box::new(fly()));
    let mut w = BitWriter::new();
    h.write(&mut w, GB);
    assert_eq!(w.compression_point(), fly().position);
    let bytes = w.into_bytes();
    let mut reader = ByIndex::default();
    let back = ServerHead::read_with(
        &mut BitReader::new(&bytes),
        GB,
        &mut free_charge(),
        &mut reader,
    )
    .unwrap();
    assert_eq!(back, h);
    assert_eq!(reader.calls, 2);
    // The client reports the checksum of what it read: the position goes raw.
    let ControlSection::Resend { data, .. } = &back.control else {
        panic!("{back:?}");
    };
    c.client_checksum = checksum(data.as_ref(), GB);
    let (h, step) = c.head(GB, Some(&o));
    assert_eq!(step, Step3::Matching);
    assert_eq!(
        h.control,
        ControlSection::Matching {
            position: word.position
        }
    );
}

#[test]
fn boxed_control_data_compares_and_checksums_as_its_contents() {
    let a: Box<dyn ControlData> = Box::new(CameraPacketData::fly([1.0, 2.0, 3.0], 0.0, 0.0));
    let b: Box<dyn ControlData> = Box::new(CameraPacketData::fly([1.0, 2.0, 3.0], 0.0, 0.0));
    let c: Box<dyn ControlData> = Box::new(Word {
        word: 0,
        position: [1.0, 2.0, 3.0],
    });
    assert!(a.as_ref() == b.as_ref());
    assert!(a.as_ref() != c.as_ref());
    assert!(a.same_as(&b as &dyn ControlData));
    assert_eq!(checksum(&a, GB), checksum(b.as_ref(), GB));
    assert!(a.clone().as_ref() == a.as_ref());
}
