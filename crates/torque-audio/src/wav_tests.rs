use super::*;

/// A WAV file of `chunks` (`(id, contents)`, each padded to an even size) with the RIFF size of its contents.
pub fn riff(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut body = b"WAVE".to_vec();
    for (id, c) in chunks {
        body.extend_from_slice(*id);
        body.extend_from_slice(&(c.len() as u32).to_le_bytes());
        body.extend_from_slice(c);
        if c.len() % 2 == 1 {
            body.push(0);
        }
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

pub fn fmt(format: u16, channels: u16, rate: u32, bits: u16) -> Vec<u8> {
    let mut f = Vec::new();
    f.extend_from_slice(&format.to_le_bytes());
    f.extend_from_slice(&channels.to_le_bytes());
    f.extend_from_slice(&rate.to_le_bytes());
    let block = u32::from(channels) * u32::from(bits / 8);
    f.extend_from_slice(&(rate * block).to_le_bytes());
    f.extend_from_slice(&(block as u16).to_le_bytes());
    f.extend_from_slice(&bits.to_le_bytes());
    f
}

/// A PCM WAV of `frames` frames.
pub fn pcm_wav(channels: u16, rate: u32, bits: u16, frames: usize) -> Vec<u8> {
    let n = frames * usize::from(channels) * usize::from(bits / 8);
    let data: Vec<u8> = (0..n).map(|i| (i * 7 % 251) as u8).collect();
    riff(&[(b"fmt ", fmt(1, channels, rate, bits)), (b"data", data)])
}

#[test]
fn pcm_formats_read() {
    for (ch, rate, bits) in [
        (1, 22_050, 16),
        (1, 11_025, 8),
        (2, 44_100, 16),
        (2, 8_000, 8),
    ] {
        let (pcm, rep) = read_wav(&pcm_wav(ch, rate, bits, 100));
        let pcm = pcm.unwrap();
        assert_eq!((pcm.channels, pcm.bits, pcm.rate), (ch, bits, rate));
        assert_eq!(pcm.frames(), 100);
        assert_eq!(rep, WavReport::default());
    }
}

#[test]
fn the_last_fmt_and_data_win_and_other_chunks_are_skipped() {
    let file = riff(&[
        (b"LIST", vec![1, 2, 3]),
        (b"fmt ", fmt(1, 1, 8_000, 8)),
        (b"data", vec![1; 10]),
        (b"cue ", vec![0; 24]),
        (b"fmt ", fmt(1, 2, 16_000, 16)),
        (b"data", vec![2; 8]),
    ]);
    let pcm = read_wav(&file).0.unwrap();
    assert_eq!((pcm.channels, pcm.bits, pcm.rate), (2, 16, 16_000));
    assert_eq!(pcm.data, vec![2; 8]);
}

#[test]
fn a_fmt_chunk_with_an_extension_and_odd_chunks_read() {
    let mut f = fmt(1, 1, 22_050, 16);
    f.extend_from_slice(&[0, 0]);
    let file = riff(&[(b"fmt ", f), (b"junk", vec![9; 5]), (b"data", vec![3; 6])]);
    assert_eq!(read_wav(&file).0.unwrap().data, vec![3; 6]);
}

#[test]
fn non_pcm_data_before_fmt_and_no_data_give_no_samples() {
    let adpcm = riff(&[(b"fmt ", fmt(0x11, 1, 22_050, 4)), (b"data", vec![0; 64])]);
    assert_eq!(read_wav(&adpcm).0, Err(WavError::NoSamples));
    let early = riff(&[(b"data", vec![0; 4]), (b"fmt ", fmt(1, 1, 22_050, 16))]);
    assert_eq!(read_wav(&early).0, Err(WavError::NoSamples));
    assert_eq!(read_wav(b"").0, Err(WavError::NoSamples));
    assert_eq!(
        read_wav(&riff(&[(b"fmt ", fmt(1, 1, 0, 16)), (b"data", vec![0; 4])])).0,
        Err(WavError::BadRate)
    );
}

#[test]
fn a_truncated_data_chunk_keeps_its_declared_size() {
    let mut file = pcm_wav(1, 22_050, 16, 100);
    file.truncate(file.len() - 50);
    let pcm = read_wav(&file).0.unwrap();
    assert_eq!((pcm.size, pcm.data.len()), (200, 150));
    assert_eq!(pcm.frames(), 100);
}

#[test]
fn a_smpl_header_is_read_whatever_the_chunk_holds() {
    // TGE reads the 60-byte header from a 20-byte chunk: the read runs on to the end of this file, and the walk ends there.
    let file = riff(&[
        (b"fmt ", fmt(1, 1, 22_050, 16)),
        (b"data", vec![5; 4]),
        (b"smpl", vec![0; 20]),
        (b"data", vec![6; 4]),
    ]);
    let (pcm, rep) = read_wav(&file);
    assert_eq!(pcm.unwrap().data, vec![5; 4]);
    assert_eq!(rep.smpl_stops, 0);
    let ok = riff(&[
        (b"fmt ", fmt(1, 1, 22_050, 16)),
        (b"smpl", vec![0; 60]),
        (b"data", vec![6; 4]),
    ]);
    let (pcm, rep) = read_wav(&ok);
    assert_eq!(pcm.unwrap().data, vec![6; 4]);
    assert_eq!(rep.smpl_stops, 0);
}

/// Rules that read a `smpl` header only when the chunk holds it.
#[derive(Debug)]
struct WholeHeaders;

impl WavRules for WholeHeaders {
    fn reads_smpl(&self, rest: u32) -> bool {
        u64::from(rest) >= SMPL_HEADER_BYTES
    }

    fn smpl_stop_line(&self, dir: &str, file: &str) -> Option<String> {
        Some(format!("{dir}|{file}"))
    }
}

#[test]
fn a_hosts_rules_can_stop_the_walk_at_a_smpl_chunk() {
    let file = riff(&[
        (b"fmt ", fmt(1, 1, 22_050, 16)),
        (b"data", vec![5; 4]),
        (b"smpl", vec![0; 20]),
        (b"data", vec![6; 4]),
    ]);
    let (pcm, rep) = read_wav_with(&file, &WholeHeaders);
    assert_eq!(pcm.unwrap().data, vec![5; 4]);
    assert_eq!(rep.smpl_stops, 1);
    let mut messages = Vec::new();
    let pcm = crate::buffer::decode(b"a/b.wav", &file, &WholeHeaders, &mut messages).unwrap();
    assert_eq!(pcm.data, vec![5; 4]);
    assert_eq!(messages, vec![(true, "a|b.wav".to_string())]);
    let mut messages = Vec::new();
    crate::buffer::decode(b"a/b.wav", &file, &TgeWav, &mut messages).unwrap();
    assert!(messages.is_empty());
}

#[test]
fn a_huge_data_chunk_is_refused_without_allocating() {
    let mut file = riff(&[(b"fmt ", fmt(1, 1, 22_050, 16)), (b"data", vec![0; 4])]);
    let at = file.len() - 8;
    file[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(read_wav(&file).0, Err(WavError::TooLarge(u32::MAX)));
}
