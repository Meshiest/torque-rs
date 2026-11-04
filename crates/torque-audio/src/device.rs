//! The sound card (feature `device`, the window client only): a `cpal` output stream whose callback mixes the shared [`Audio`]. While a
//! device runs, its callback moves the voices on; the host must not also [`Audio::advance`] them. Never built by a test.

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::system::Audio;

/// A running output stream (dropping it stops the sound).
pub struct Device {
    _stream: cpal::Stream,
    /// The system's audio layer cpal plays through ("WASAPI", "ASIO", "CoreAudio", "ALSA", ...).
    pub backend: String,
    pub name: String,
    pub rate: u32,
    pub channels: u16,
}

fn fill(audio: &Arc<Mutex<Audio>>, out: &mut [f32], channels: usize, rate: u32) {
    out.fill(0.0);
    if let Ok(mut a) = audio.lock() {
        a.render(out, channels, rate);
    }
    for x in out.iter_mut() {
        *x = x.clamp(-1.0, 1.0);
    }
}

impl Device {
    /// The default output device playing `audio`, or why not.
    pub fn open(audio: Arc<Mutex<Audio>>) -> Result<Device, String> {
        let host = cpal::default_host();
        let backend = host.id().name().to_string();
        let device = host
            .default_output_device()
            .ok_or_else(|| "no output device".to_string())?;
        let name = device.name().unwrap_or_else(|_| "output".to_string());
        let config = device.default_output_config().map_err(|e| e.to_string())?;
        let rate = config.sample_rate().0;
        let channels = config.channels();
        let stream_config: cpal::StreamConfig = config.clone().into();
        let err = |e| eprintln!("audio device: {e}");
        let ch = usize::from(channels);
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                let a = audio.clone();
                device.build_output_stream(
                    &stream_config,
                    move |data: &mut [f32], _| fill(&a, data, ch, rate),
                    err,
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                let a = audio.clone();
                let mut scratch = Vec::new();
                device.build_output_stream(
                    &stream_config,
                    move |data: &mut [i16], _| {
                        scratch.resize(data.len(), 0.0);
                        fill(&a, &mut scratch, ch, rate);
                        for (d, s) in data.iter_mut().zip(&scratch) {
                            *d = (s * 32_767.0) as i16;
                        }
                    },
                    err,
                    None,
                )
            }
            cpal::SampleFormat::U16 => {
                let a = audio.clone();
                let mut scratch = Vec::new();
                device.build_output_stream(
                    &stream_config,
                    move |data: &mut [u16], _| {
                        scratch.resize(data.len(), 0.0);
                        fill(&a, &mut scratch, ch, rate);
                        for (d, s) in data.iter_mut().zip(&scratch) {
                            *d = ((s + 1.0) * 32_767.5) as u16;
                        }
                    },
                    err,
                    None,
                )
            }
            other => return Err(format!("sample format {other:?}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Device {
            _stream: stream,
            backend,
            name,
            rate,
            channels,
        })
    }
}
