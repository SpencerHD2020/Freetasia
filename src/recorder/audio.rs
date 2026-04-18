use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use hound::{SampleFormat, WavSpec, WavWriter};

type SharedWriter = Arc<Mutex<Option<WavWriter<BufWriter<File>>>>>;

/// Handle to a running audio-capture session.
///
/// Audio is written to a WAV file at `output_path` as 32-bit float PCM.
/// The stream is stopped and the WAV file is finalised when this struct is
/// dropped or [`AudioRecorder::stop`] is called.
pub struct AudioRecorder {
    /// Keep the stream alive for the duration of the recording.
    _stream: Option<cpal::Stream>,
    writer: SharedWriter,
    running: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    /// Path of the WAV file being written.
    pub output_path: PathBuf,
    pub sample_rate: u32,
    pub channels: u16,
}

impl AudioRecorder {
    /// Start recording the default input device to `output_path`.
    ///
    /// Returns an error if no input device is available or if the sample
    /// format is unsupported.
    pub fn start(output_path: PathBuf) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("No audio input device found")?;

        let config = device
            .default_input_config()
            .context("Failed to query default input config")?;

        let sample_rate = config.sample_rate().0;
        let channels = config.channels();

        let spec = WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };

        let writer: SharedWriter = Arc::new(Mutex::new(Some(
            WavWriter::create(&output_path, spec)
                .with_context(|| format!("Cannot create WAV file at {}", output_path.display()))?,
        )));

        let running = Arc::new(AtomicBool::new(true));
        let paused = Arc::new(AtomicBool::new(false));

        let stream = build_stream(
            &device,
            &config,
            writer.clone(),
            running.clone(),
            paused.clone(),
        )?;

        stream.play().context("Failed to start audio stream")?;

        Ok(Self {
            _stream: Some(stream),
            writer,
            running,
            paused,
            output_path,
            sample_rate,
            channels,
        })
    }

    /// Temporarily stop writing samples (the stream stays open).
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    /// Resume writing samples after a pause.
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    /// Stop recording and finalise the WAV file.
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        // Drop the stream first to prevent any further callbacks.
        self._stream = None;
        // Finalise the WAV file so the header is correct.
        if let Ok(mut guard) = self.writer.lock() {
            if let Some(w) = guard.take() {
                let _ = w.finalize();
            }
        }
    }
}

impl Drop for AudioRecorder {
    fn drop(&mut self) {
        self.stop();
    }
}

// ── Internals ──────────────────────────────────────────────────────────────

fn build_stream(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    writer: SharedWriter,
    running: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
) -> Result<cpal::Stream> {
    let err_fn = |e| log::error!("Audio stream error: {e}");

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => {
            let w = writer.clone();
            let r = running.clone();
            let p = paused.clone();
            device.build_input_stream(
                &config.clone().into(),
                move |data: &[f32], _| write_samples_f32(data, &w, &r, &p),
                err_fn,
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let w = writer.clone();
            let r = running.clone();
            let p = paused.clone();
            device.build_input_stream(
                &config.clone().into(),
                move |data: &[i16], _| write_samples_i16(data, &w, &r, &p),
                err_fn,
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let w = writer.clone();
            let r = running.clone();
            let p = paused.clone();
            device.build_input_stream(
                &config.clone().into(),
                move |data: &[u16], _| write_samples_u16(data, &w, &r, &p),
                err_fn,
                None,
            )
        }
        fmt => anyhow::bail!("Unsupported audio sample format: {fmt:?}"),
    }
    .context("Failed to build audio input stream")?;

    Ok(stream)
}

fn write_samples_f32(
    data: &[f32],
    writer: &SharedWriter,
    running: &AtomicBool,
    paused: &AtomicBool,
) {
    if !running.load(Ordering::SeqCst) || paused.load(Ordering::SeqCst) {
        return;
    }
    if let Ok(mut guard) = writer.lock() {
        if let Some(ref mut w) = *guard {
            for &s in data {
                let _ = w.write_sample(s);
            }
        }
    }
}

fn write_samples_i16(
    data: &[i16],
    writer: &SharedWriter,
    running: &AtomicBool,
    paused: &AtomicBool,
) {
    if !running.load(Ordering::SeqCst) || paused.load(Ordering::SeqCst) {
        return;
    }
    if let Ok(mut guard) = writer.lock() {
        if let Some(ref mut w) = *guard {
            for &s in data {
                let sample = s as f32 / i16::MAX as f32;
                let _ = w.write_sample(sample);
            }
        }
    }
}

fn write_samples_u16(
    data: &[u16],
    writer: &SharedWriter,
    running: &AtomicBool,
    paused: &AtomicBool,
) {
    if !running.load(Ordering::SeqCst) || paused.load(Ordering::SeqCst) {
        return;
    }
    if let Ok(mut guard) = writer.lock() {
        if let Some(ref mut w) = *guard {
            for &s in data {
                let sample = (s as f32 / u16::MAX as f32) * 2.0 - 1.0;
                let _ = w.write_sample(sample);
            }
        }
    }
}
