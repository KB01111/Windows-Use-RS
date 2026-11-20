//! STT and TTS services, mirroring `windows_use/speech/service.py`.
//!
//! The Python original wires `pyaudio` streams straight to the provider. Here the
//! microphone is captured with [`cpal`], written to a temporary WAV with
//! [`hound`] and handed to the provider, while generated audio is decoded with
//! [`rodio`] and played through [`cpal`].
//!
//! Providers are async (see [`crate::providers::base::SttProvider`]) while these
//! services are synchronous, exactly like the Python classes: every provider call
//! is driven to completion on a small dedicated Tokio runtime so callers from
//! ordinary `main` code need no runtime of their own.
//!
//! Note on playback: the workspace's `rodio` is built with `default-features =
//! false, features = ["wav"]`, i.e. decoding without the `playback` feature, so
//! `rodio::DeviceSinkBuilder`/`rodio::Player` are not compiled in. Decoding and
//! sample conversion therefore go through `rodio::Decoder` and the samples are
//! submitted to the default output device with `cpal`.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use rodio::Source;

use crate::providers::base::{SttProvider, TtsProvider};

/// Builds an STT provider by name (see [`crate::providers::speech::stt_provider`]).
pub fn create_stt(
    provider: &str,
    model: Option<String>,
    api_key: Option<String>,
) -> Option<Box<dyn SttProvider>> {
    crate::providers::speech::stt_provider(provider, model, api_key)
}

/// Builds a TTS provider by name (see [`crate::providers::speech::tts_provider`]).
pub fn create_tts(
    provider: &str,
    model: Option<String>,
    voice: Option<String>,
    api_key: Option<String>,
) -> Option<Box<dyn TtsProvider>> {
    crate::providers::speech::tts_provider(provider, model, voice, api_key)
}

/// Default audio buffer size in frames, matching the Python `chunk_size`.
pub const DEFAULT_CHUNK_SIZE: usize = 1024;
/// Default capture sample rate in Hz, matching the Python `frame_rate`.
pub const DEFAULT_FRAME_RATE: u32 = 44100;
/// Default channel count (mono), matching the Python `channels`.
pub const DEFAULT_CHANNELS: u16 = 1;

/// Pause after playback so the tail of the audio is not cut off (`sleep(0.5)`).
const PLAYBACK_TAIL_PAUSE: Duration = Duration::from_millis(500);
/// Extra time granted on top of the audio length before playback is abandoned.
const PLAYBACK_SLACK: Duration = Duration::from_secs(10);

/// Removes `path` if it exists, ignoring failures the way the Python code does.
fn remove_file_quietly(path: &Path) {
    if path.as_os_str().is_empty() {
        return;
    }
    let _ = std::fs::remove_file(path);
}

/// Allocates a unique `.wav` path in the system temp directory.
///
/// Python uses `NamedTemporaryFile(delete=False, suffix=".wav")`; the equivalent
/// here is a `uuid`-named file in [`std::env::temp_dir`].
fn temp_wav_path() -> Result<PathBuf> {
    Ok(std::env::temp_dir().join(format!("windows-use-{}.wav", uuid::Uuid::new_v4())))
}

/// Drives a provider future to completion.
///
/// A dedicated current-thread runtime keeps the service synchronous. When the
/// caller is already inside a Tokio runtime the future runs on a scoped thread
/// instead, because a runtime cannot be started from within another runtime.
fn block_on<F, T>(future: F) -> Result<T>
where
    F: std::future::Future<Output = Result<T>> + Send,
    T: Send,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        return thread::scope(|scope| {
            match scope
                .spawn(|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .context("failed to build the speech provider runtime")?;
                    runtime.block_on(future)
                })
                .join()
            {
                Ok(outcome) => outcome,
                Err(_) => Err(anyhow!("speech provider call panicked")),
            }
        });
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build the speech provider runtime")?;
    runtime.block_on(future)
}

/// Writes `samples` as a 16-bit PCM WAV at `path`.
fn write_wav_file(path: &Path, samples: &[i16], channels: u16, sample_rate: u32) -> Result<()> {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("[STT] Failed to create {}", path.display()))?;
    for sample in samples {
        writer
            .write_sample(*sample)
            .context("[STT] Failed to write audio samples to WAV")?;
    }
    writer
        .finalize()
        .context("[STT] Failed to finalize the WAV file")?;
    Ok(())
}

/// Speech-to-Text service: records from the microphone and transcribes it.
///
/// Mirrors the Python `STT` class, including the programmatic
/// `start_recording` / `stop_recording` / `process_audio` sequence used by UIs.
pub struct Stt {
    provider: Option<Box<dyn SttProvider>>,
    verbose: bool,
    chunk_size: usize,
    frame_rate: u32,
    channels: u16,
    /// Channels actually delivered by the device during the last recording.
    capture_channels: u16,
    /// Sample rate actually delivered by the device during the last recording.
    capture_frame_rate: u32,
    /// Recorded samples, filled by the capture callback.
    samples: Arc<Mutex<Vec<i16>>>,
    /// Written by the controlling thread, read by the capture callback.
    is_recording: Arc<AtomicBool>,
    /// The live capture stream; `None` when not recording.
    stream: Option<cpal::Stream>,
    recording_thread: Option<JoinHandle<()>>,
    tempfile_path: Option<PathBuf>,
    audio_samples: Option<Vec<i16>>,
}

impl std::fmt::Debug for Stt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stt")
            .field("provider", &self.model())
            .field("verbose", &self.verbose)
            .field("chunk_size", &self.chunk_size)
            .field("frame_rate", &self.frame_rate)
            .field("channels", &self.channels)
            .field("is_recording", &self.is_recording())
            .finish_non_exhaustive()
    }
}

impl Stt {
    /// Creates a service, matching the Python constructor signature.
    pub fn new(
        provider: Option<Box<dyn SttProvider>>,
        verbose: bool,
        chunk_size: usize,
        frame_rate: u32,
        channels: u16,
    ) -> Self {
        let chunk_size = if chunk_size == 0 { DEFAULT_CHUNK_SIZE } else { chunk_size };
        let frame_rate = if frame_rate == 0 { DEFAULT_FRAME_RATE } else { frame_rate };
        let channels = if channels == 0 { DEFAULT_CHANNELS } else { channels };
        Self {
            provider,
            verbose,
            chunk_size,
            frame_rate,
            channels,
            capture_channels: channels,
            capture_frame_rate: frame_rate,
            samples: Arc::new(Mutex::new(Vec::new())),
            is_recording: Arc::new(AtomicBool::new(false)),
            stream: None,
            recording_thread: None,
            tempfile_path: None,
            audio_samples: None,
        }
    }

    /// Creates a service with the Python defaults (1024 frames, 44100 Hz, mono).
    pub fn with_defaults(provider: Option<Box<dyn SttProvider>>, verbose: bool) -> Self {
        Self::new(provider, verbose, DEFAULT_CHUNK_SIZE, DEFAULT_FRAME_RATE, DEFAULT_CHANNELS)
    }

    /// The configured buffer size in frames.
    pub fn chunk_size(&self) -> usize {
        self.chunk_size
    }

    /// The configured capture sample rate.
    pub fn frame_rate(&self) -> u32 {
        self.frame_rate
    }

    /// The configured channel count.
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// The sample rate the last recording was actually captured at.
    pub fn capture_frame_rate(&self) -> u32 {
        self.capture_frame_rate
    }

    /// The channel count the last recording was actually captured with.
    pub fn capture_channels(&self) -> u16 {
        self.capture_channels
    }

    /// Whether a recording is currently running.
    pub fn is_recording(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst)
    }

    /// The provider's model name, when a provider is configured.
    pub fn model(&self) -> Option<&str> {
        self.provider.as_ref().map(|provider| provider.model())
    }

    /// The last WAV written by [`Stt::process_audio`], if it has not been deleted.
    pub fn tempfile_path(&self) -> Option<&Path> {
        self.tempfile_path.as_deref()
    }

    /// The recorded samples, when a recording has been taken.
    pub fn audio_samples(&self) -> Option<&[i16]> {
        self.audio_samples.as_deref()
    }

    /// Starts capturing on the default input device in a background thread.
    ///
    /// Mirrors Python's `start_recording`: every captured sample is converted to
    /// i16 and appended until [`Stt::stop_recording`] clears the flag.
    pub fn start_recording(&mut self) -> Result<()> {
        if self.is_recording() {
            if self.verbose {
                eprintln!("[STT] Recording is already in progress");
            }
            return Ok(());
        }

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow!("[STT] No default audio input device available"))?;

        let supported = device
            .default_input_config()
            .map_err(|error| anyhow!("[STT] Failed to query the input config: {error}"))?;
        let sample_format = supported.sample_format();
        let config = supported.config();

        // Shared-mode WASAPI only accepts the endpoint's own format, so the WAV
        // header records what the device really produced rather than the request.
        self.capture_channels = config.channels;
        self.capture_frame_rate = config.sample_rate;

        if self.verbose
            && (self.capture_channels != self.channels || self.capture_frame_rate != self.frame_rate)
        {
            eprintln!(
                "[STT] Input device delivers {} channel(s) at {} Hz (requested {} at {} Hz)",
                self.capture_channels, self.capture_frame_rate, self.channels, self.frame_rate
            );
        }

        let samples = Arc::clone(&self.samples);
        samples
            .lock()
            .map_err(|_| anyhow!("[STT] Recording buffer is poisoned"))?
            .clear();

        // A fixed buffer keeps latency predictable; not every host honours it, so
        // fall back to the default buffer when the device rejects the request.
        let stream = match self.build_stream(&device, &config, sample_format, &samples) {
            Ok(stream) => stream,
            Err(first_error) => {
                let mut fallback = config;
                fallback.buffer_size = cpal::BufferSize::Default;
                self.build_stream(&device, &fallback, sample_format, &samples)
                    .map_err(|_| first_error)?
            }
        };

        stream
            .play()
            .map_err(|error| anyhow!("[STT] Failed to start the input stream: {error}"))?;

        self.stream = Some(stream);
        self.is_recording.store(true, Ordering::SeqCst);

        if self.verbose {
            eprintln!("[STT] Recording started...");
            eprintln!("[STT] Press Enter to stop recording...");
        }

        // cpal delivers samples from its own audio thread; this thread only keeps
        // the recording window open and joins cleanly when recording stops.
        let flag = Arc::clone(&self.is_recording);
        self.recording_thread = Some(thread::spawn(move || {
            while flag.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(10));
            }
        }));

        Ok(())
    }

    /// Builds the capture stream for one concrete sample format.
    fn build_stream(
        &self,
        device: &cpal::Device,
        config: &cpal::StreamConfig,
        sample_format: SampleFormat,
        samples: &Arc<Mutex<Vec<i16>>>,
    ) -> Result<cpal::Stream> {
        let mut with_buffer = *config;
        with_buffer.buffer_size = cpal::BufferSize::Fixed(self.chunk_size as u32);

        let recording = Arc::clone(&self.is_recording);

        match sample_format {
            SampleFormat::I8 => build_input_stream::<i8>(device, &with_buffer, samples, &recording),
            SampleFormat::I16 => build_input_stream::<i16>(device, &with_buffer, samples, &recording),
            SampleFormat::I32 => build_input_stream::<i32>(device, &with_buffer, samples, &recording),
            SampleFormat::I64 => build_input_stream::<i64>(device, &with_buffer, samples, &recording),
            SampleFormat::U8 => build_input_stream::<u8>(device, &with_buffer, samples, &recording),
            SampleFormat::U16 => build_input_stream::<u16>(device, &with_buffer, samples, &recording),
            SampleFormat::U32 => build_input_stream::<u32>(device, &with_buffer, samples, &recording),
            SampleFormat::U64 => build_input_stream::<u64>(device, &with_buffer, samples, &recording),
            SampleFormat::F32 => build_input_stream::<f32>(device, &with_buffer, samples, &recording),
            SampleFormat::F64 => build_input_stream::<f64>(device, &with_buffer, samples, &recording),
            other => Err(anyhow!("[STT] Unsupported input sample format: {other}")),
        }
    }

    /// Stops capturing, joins the helper thread and keeps the samples.
    pub fn stop_recording(&mut self) -> Result<()> {
        if !self.is_recording() {
            if self.verbose {
                eprintln!("[STT] No recording in progress");
            }
            return Ok(());
        }

        self.is_recording.store(false, Ordering::SeqCst);

        if let Some(handle) = self.recording_thread.take() {
            let _ = handle.join();
        }

        if let Some(stream) = self.stream.take() {
            let _ = stream.pause();
            drop(stream);
        }

        let captured = self
            .samples
            .lock()
            .map_err(|_| anyhow!("[STT] Recording buffer is poisoned"))?
            .clone();
        self.audio_samples = Some(captured);

        if self.verbose {
            eprintln!("[STT] Recording stopped");
        }

        Ok(())
    }

    /// Transcribes the recorded audio through the provider.
    ///
    /// Writes a temporary WAV, calls `transcribe`, then deletes the file.
    pub fn process_audio(&mut self) -> Result<String> {
        let samples = self
            .audio_samples
            .clone()
            .ok_or_else(|| {
                anyhow!("[STT] No audio recorded. Call start_recording/stop_recording first.")
            })?;

        let (model, provider) = match self.provider.as_ref() {
            Some(provider) => (provider.model().to_string(), provider),
            None => return Err(anyhow!("[STT] No STT provider configured.")),
        };

        if self.verbose {
            eprintln!("[STT] Transcribing audio using {model}...");
        }

        let path = temp_wav_path()?;
        let outcome = write_wav_file(&path, &samples, self.capture_channels, self.capture_frame_rate)
            .and_then(|()| {
                let path_text = path
                    .to_str()
                    .ok_or_else(|| anyhow!("[STT] Temporary path is not valid UTF-8"))?
                    .to_string();
                self.tempfile_path = Some(path.clone());
                block_on(provider.transcribe(&path_text))
            });

        self.tempfile_path = None;
        remove_file_quietly(&path);
        outcome
    }

    /// Records until Enter is pressed, then transcribes and releases resources.
    pub fn invoke(&mut self) -> Result<String> {
        self.start_recording()?;

        // Python calls the blocking `input()`; a thread keeps the wait off the
        // recording setup path while still stopping on Enter or on stdin EOF.
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            let _ = sender.send(());
        });
        let _ = receiver.recv();

        self.stop_recording()?;
        let text = self.process_audio()?;
        self.close();
        Ok(text)
    }

    /// Releases the audio stream, the buffered samples and any temporary file.
    pub fn close(&mut self) {
        self.is_recording.store(false, Ordering::SeqCst);
        if let Some(handle) = self.recording_thread.take() {
            let _ = handle.join();
        }
        if let Some(stream) = self.stream.take() {
            let _ = stream.pause();
            drop(stream);
        }
        self.audio_samples = None;
        if let Some(path) = self.tempfile_path.take() {
            remove_file_quietly(&path);
        }
        if let Ok(mut samples) = self.samples.lock() {
            samples.clear();
        }
    }
}

impl Drop for Stt {
    fn drop(&mut self) {
        self.close();
    }
}

/// Builds an input stream that converts every sample to `i16`.
///
/// The callback only appends while `recording` is set, which is how the Python
/// `_record_loop` stops collecting frames after `stop_recording`.
fn build_input_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    samples: &Arc<Mutex<Vec<i16>>>,
    recording: &Arc<AtomicBool>,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    i16: FromSample<T>,
{
    let buffer = Arc::clone(samples);
    let flag = Arc::clone(recording);
    device
        .build_input_stream(
            *config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                if !flag.load(Ordering::SeqCst) {
                    return;
                }
                if let Ok(mut captured) = buffer.lock() {
                    captured.extend(data.iter().map(|sample| sample.to_sample::<i16>()));
                }
            },
            |error: cpal::Error| eprintln!("[STT] Error reading audio stream: {error}"),
            None,
        )
        .map_err(|error| anyhow!("[STT] Failed to open the input stream: {error}"))
}

/// Text-to-Speech service: synthesizes text and plays the audio back.
///
/// Mirrors the Python `TTS` class, including the separate
/// `generate_audio` / `play_audio` steps used by UIs.
pub struct Tts {
    provider: Option<Box<dyn TtsProvider>>,
    verbose: bool,
    chunk_size: usize,
    tempfile_path: Option<PathBuf>,
}

impl std::fmt::Debug for Tts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tts")
            .field("provider", &self.model())
            .field("verbose", &self.verbose)
            .field("chunk_size", &self.chunk_size)
            .finish_non_exhaustive()
    }
}

impl Tts {
    /// Creates a service, matching the Python constructor signature.
    pub fn new(provider: Option<Box<dyn TtsProvider>>, verbose: bool, chunk_size: usize) -> Self {
        Self {
            provider,
            verbose,
            chunk_size: if chunk_size == 0 { DEFAULT_CHUNK_SIZE } else { chunk_size },
            tempfile_path: None,
        }
    }

    /// Creates a service with the Python default buffer size.
    pub fn with_defaults(provider: Option<Box<dyn TtsProvider>>, verbose: bool) -> Self {
        Self::new(provider, verbose, DEFAULT_CHUNK_SIZE)
    }

    /// The configured playback buffer size in frames.
    pub fn chunk_size(&self) -> usize {
        self.chunk_size
    }

    /// The provider's model name, when a provider is configured.
    pub fn model(&self) -> Option<&str> {
        self.provider.as_ref().map(|provider| provider.model())
    }

    /// The path of the last generated audio file, when it has not been deleted.
    pub fn tempfile_path(&self) -> Option<&Path> {
        self.tempfile_path.as_deref()
    }

    /// Synthesizes `text` into a temporary `.wav` and returns its path.
    pub fn generate_audio(&mut self, text: &str) -> Result<String> {
        let (model, provider) = match self.provider.as_ref() {
            Some(provider) => (provider.model().to_string(), provider),
            None => return Err(anyhow!("[TTS] No TTS provider configured.")),
        };

        if self.verbose {
            eprintln!("[TTS] Generating audio using {model}...");
        }

        // Drop the previous file before taking over the slot, like Python's
        // `_generate_tempfile` reusing `self.tempfile_path`.
        if let Some(previous) = self.tempfile_path.take() {
            remove_file_quietly(&previous);
        }

        let path = temp_wav_path()?;
        let path_text = path
            .to_str()
            .ok_or_else(|| anyhow!("[TTS] Temporary path is not valid UTF-8"))?
            .to_string();

        match block_on(provider.synthesize(text, &path_text)) {
            Ok(()) => {
                self.tempfile_path = Some(path);
                Ok(path_text)
            }
            Err(error) => {
                remove_file_quietly(&path);
                Err(error.context("[TTS] Failed to synthesize audio"))
            }
        }
    }

    /// Plays the audio produced by [`Tts::generate_audio`].
    ///
    /// Playback blocks until the file has been played, then pauses briefly and
    /// deletes the file, exactly like the Python implementation.
    pub fn play_audio(&mut self) -> Result<()> {
        let path = self
            .tempfile_path
            .clone()
            .ok_or_else(|| anyhow!("[TTS] No audio data loaded. Call generate_audio first."))?;

        if self.verbose {
            eprintln!("[TTS] Playing audio...");
        }

        play_wav_file(&path, self.chunk_size)?;

        if self.verbose {
            eprintln!("[TTS] Audio playback complete");
        }

        thread::sleep(PLAYBACK_TAIL_PAUSE);
        remove_file_quietly(&path);
        self.tempfile_path = None;
        Ok(())
    }

    /// Generates and plays `text`, then cleans the temporary file up.
    pub fn invoke(&mut self, text: &str) -> Result<()> {
        self.generate_audio(text)?;
        self.play_audio()?;
        self.close();
        Ok(())
    }

    /// Releases the generated file and any playback resources.
    pub fn close(&mut self) {
        if let Some(path) = self.tempfile_path.take() {
            remove_file_quietly(&path);
        }
    }
}

impl Drop for Tts {
    fn drop(&mut self) {
        self.close();
    }
}

/// Python-compatible alias for [`Stt`], matching the `STT` class name.
pub type STT = Stt;
/// Python-compatible alias for [`Tts`], matching the `TTS` class name.
pub type TTS = Tts;

/// Decoded audio ready for playback.
#[derive(Debug)]
struct DecodedAudio {
    samples: Vec<f32>,
    channels: u16,
    sample_rate: u32,
}

/// Decodes a WAV file with [`rodio`], reading it in `chunk_size`-frame blocks.
fn decode_wav(path: &Path, chunk_size: usize) -> Result<DecodedAudio> {
    let file =
        File::open(path).with_context(|| format!("[TTS] Failed to open {}", path.display()))?;
    let decoder = rodio::Decoder::new(BufReader::new(file))
        .map_err(|error| anyhow!("[TTS] Failed to decode {}: {error}", path.display()))?;

    let channels = decoder.channels().get();
    let sample_rate = decoder.sample_rate().get();
    // Whole frames only, so a block boundary never splits a sample frame.
    let block = chunk_size.max(1).saturating_mul(channels.max(1) as usize);

    let mut samples = Vec::new();
    let mut decoder = decoder;
    loop {
        let mut read = 0;
        while read < block {
            match decoder.next() {
                Some(sample) => {
                    samples.push(sample);
                    read += 1;
                }
                None => break,
            }
        }
        if read < block {
            break;
        }
    }

    if samples.is_empty() {
        return Err(anyhow!("[TTS] {} contains no audio samples", path.display()));
    }

    Ok(DecodedAudio { samples, channels, sample_rate })
}

/// Adapts interleaved samples to the output device's channel count and rate.
fn adapt_audio(audio: &DecodedAudio, channels: u16, sample_rate: u32) -> Vec<f32> {
    if channels == audio.channels && sample_rate == audio.sample_rate {
        return audio.samples.clone();
    }

    let source_channels = audio.channels.max(1) as usize;
    let target_channels = channels.max(1) as usize;
    let source_frames = audio.samples.len() / source_channels;
    if source_frames == 0 {
        return Vec::new();
    }

    // Frame count after resampling with linear interpolation.
    let target_frames = if sample_rate == audio.sample_rate {
        source_frames
    } else {
        ((source_frames as f64) * f64::from(sample_rate) / f64::from(audio.sample_rate.max(1)))
            .ceil() as usize
    };

    let mut out = Vec::with_capacity(target_frames.saturating_mul(target_channels));
    for frame in 0..target_frames {
        let position = if sample_rate == audio.sample_rate || target_frames <= 1 {
            frame.min(source_frames - 1)
        } else {
            ((frame as f64) * f64::from(audio.sample_rate.max(1)) / f64::from(sample_rate.max(1)))
                .round() as usize
        };
        let position = position.min(source_frames - 1);

        for channel in 0..target_channels {
            let source_channel = channel % source_channels;
            out.push(audio.samples[position * source_channels + source_channel]);
        }
    }
    out
}

/// Plays a WAV file through the default output device.
///
/// The file is decoded with [`rodio`] and streamed to [`cpal`] in the requested
/// buffer size, mirroring the Python `readframes`/`write` loop.
fn play_wav_file(path: &Path, chunk_size: usize) -> Result<()> {
    let audio = decode_wav(path, chunk_size)?;

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow!("[TTS] No default audio output device available"))?;
    let supported = device
        .default_output_config()
        .map_err(|error| anyhow!("[TTS] Failed to query the output config: {error}"))?;

    // The endpoint dictates the format in shared mode; resample when it differs.
    let config = supported.config();
    let channels = config.channels;
    let sample_rate = config.sample_rate;
    let samples = adapt_audio(&audio, channels, sample_rate);
    if samples.is_empty() {
        return Err(anyhow!("[TTS] Nothing to play from {}", path.display()));
    }

    let frames = samples.len() / channels.max(1) as usize;
    let duration = Duration::from_secs_f64(frames as f64 / f64::from(sample_rate.max(1)));
    let state = Arc::new(Mutex::new(PlaybackState {
        samples,
        position: 0,
        finished: None,
    }));
    let (done_tx, done_rx) = mpsc::channel();
    if let Ok(mut guard) = state.lock() {
        guard.finished = Some(done_tx);
    }

    let sample_format = supported.sample_format();
    let stream = match sample_format {
        SampleFormat::F32 => build_output_stream::<f32>(&device, &config, &state, chunk_size),
        SampleFormat::I16 => build_output_stream::<i16>(&device, &config, &state, chunk_size),
        SampleFormat::U16 => build_output_stream::<u16>(&device, &config, &state, chunk_size),
        SampleFormat::F64 => build_output_stream::<f64>(&device, &config, &state, chunk_size),
        other => Err(anyhow!("[TTS] Unsupported output sample format: {other}")),
    }?;

    stream
        .play()
        .map_err(|error| anyhow!("[TTS] Failed to start the output stream: {error}"))?;

    // Wait for the callback to drain the buffer; the timeout guards against a
    // stalled device so playback can never hang the agent for ever.
    let _ = done_rx.recv_timeout(duration + PLAYBACK_SLACK);

    let _ = stream.pause();
    drop(stream);
    Ok(())
}

/// Shared playback cursor written by the real-time output callback.
struct PlaybackState {
    samples: Vec<f32>,
    position: usize,
    finished: Option<mpsc::Sender<()>>,
}

/// Builds an output stream that drains [`PlaybackState`] in order.
///
/// The configured `chunk_size` is requested first; devices that refuse a fixed
/// buffer size are retried with the host default so playback still works.
fn build_output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    state: &Arc<Mutex<PlaybackState>>,
    chunk_size: usize,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let mut requested = *config;
    requested.buffer_size = cpal::BufferSize::Fixed(chunk_size.max(1) as u32);

    match open_output_stream::<T>(device, requested, state) {
        Ok(stream) => Ok(stream),
        Err(first_error) => open_output_stream::<T>(device, *config, state)
            .map_err(|_| anyhow!("[TTS] Failed to open the output stream: {first_error}")),
    }
}

/// Opens one output stream attempt for a concrete [`PlaybackState`] buffer.
fn open_output_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    state: &Arc<Mutex<PlaybackState>>,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let buffer = Arc::clone(state);
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                let Ok(mut playback) = buffer.lock() else {
                    data.fill(T::from_sample(0.0f32));
                    return;
                };
                for slot in data.iter_mut() {
                    match playback.samples.get(playback.position) {
                        Some(sample) => {
                            *slot = T::from_sample(*sample);
                            playback.position += 1;
                        }
                        None => *slot = T::from_sample(0.0f32),
                    }
                }
                if playback.position >= playback.samples.len() {
                    if let Some(sender) = playback.finished.take() {
                        let _ = sender.send(());
                    }
                }
            },
            |error: cpal::Error| eprintln!("[TTS] Error writing audio stream: {error}"),
            None,
        )
        .map_err(|error| anyhow!("{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_python_constructor() {
        let stt = Stt::with_defaults(None, false);
        assert_eq!(stt.chunk_size(), 1024);
        assert_eq!(stt.frame_rate(), 44100);
        assert_eq!(stt.channels(), 1);
        assert!(!stt.is_recording());
        assert_eq!(stt.model(), None);

        let tts = Tts::with_defaults(None, false);
        assert_eq!(tts.chunk_size(), 1024);
        assert_eq!(tts.model(), None);
    }

    #[test]
    fn zero_arguments_fall_back_to_defaults() {
        let stt = Stt::new(None, false, 0, 0, 0);
        assert_eq!(stt.chunk_size(), DEFAULT_CHUNK_SIZE);
        assert_eq!(stt.frame_rate(), DEFAULT_FRAME_RATE);
        assert_eq!(stt.channels(), DEFAULT_CHANNELS);

        let tts = Tts::new(None, false, 0);
        assert_eq!(tts.chunk_size(), DEFAULT_CHUNK_SIZE);
    }

    #[test]
    fn process_audio_without_recording_is_an_error() {
        let mut stt = Stt::with_defaults(None, false);
        let error = stt.process_audio().unwrap_err();
        assert!(error.to_string().contains("No audio recorded"));
    }

    #[test]
    fn process_audio_without_provider_is_an_error() {
        let mut stt = Stt::with_defaults(None, false);
        stt.audio_samples = Some(vec![0i16, 1, 2]);
        let error = stt.process_audio().unwrap_err();
        assert!(error.to_string().contains("No STT provider configured"));
    }

    #[test]
    fn generate_audio_without_provider_is_an_error() {
        let mut tts = Tts::with_defaults(None, false);
        let error = tts.generate_audio("hello").unwrap_err();
        assert!(error.to_string().contains("No TTS provider configured"));
    }

    #[test]
    fn play_audio_without_audio_is_an_error() {
        let mut tts = Tts::with_defaults(None, false);
        let error = tts.play_audio().unwrap_err();
        assert!(error.to_string().contains("No audio data loaded"));
    }

    #[test]
    fn stop_recording_is_a_noop_when_idle() {
        let mut stt = Stt::with_defaults(None, false);
        stt.stop_recording().expect("idle stop is fine");
        assert!(!stt.is_recording());
    }

    #[test]
    fn wav_writer_produces_16_bit_pcm() {
        let dir = std::env::temp_dir().join(format!("windows-use-stt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("capture.wav");
        write_wav_file(&path, &[0i16, 100, -100, 32000], 2, 8000).expect("write wav");

        let mut reader = hound::WavReader::open(&path).expect("open wav");
        let spec = reader.spec();
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.sample_rate, 8000);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);
        let samples: Vec<i16> = reader.samples::<i16>().map(|s| s.expect("sample")).collect();
        assert_eq!(samples, vec![0, 100, -100, 32000]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decode_wav_reads_every_sample() {
        let dir = std::env::temp_dir().join(format!("windows-use-tts-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("speech.wav");
        write_wav_file(&path, &[0i16, 1000, -1000, 2000], 1, 8000).expect("write wav");

        let audio = decode_wav(&path, 2).expect("decode wav");
        assert_eq!(audio.channels, 1);
        assert_eq!(audio.sample_rate, 8000);
        assert_eq!(audio.samples.len(), 4);
        assert!((audio.samples[0] - 0.0).abs() < 1e-6);
        assert!(audio.samples[1] > 0.0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decode_wav_rejects_an_empty_file() {
        let dir = std::env::temp_dir().join(format!("windows-use-tts-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("empty.wav");
        write_wav_file(&path, &[], 1, 8000).expect("write wav");

        let error = decode_wav(&path, 2).unwrap_err();
        assert!(error.to_string().contains("no audio samples"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn adapt_audio_keeps_matching_format() {
        let audio = DecodedAudio { samples: vec![0.1, 0.2], channels: 1, sample_rate: 8000 };
        assert_eq!(adapt_audio(&audio, 1, 8000), vec![0.1, 0.2]);
    }

    #[test]
    fn adapt_audio_duplicates_mono_to_stereo() {
        let audio = DecodedAudio { samples: vec![0.5, -0.5], channels: 1, sample_rate: 8000 };
        let adapted = adapt_audio(&audio, 2, 8000);
        assert_eq!(adapted, vec![0.5, 0.5, -0.5, -0.5]);
    }

    #[test]
    fn adapt_audio_resamples_upward() {
        let audio = DecodedAudio { samples: vec![0.0, 1.0], channels: 1, sample_rate: 8000 };
        let adapted = adapt_audio(&audio, 1, 16000);
        assert_eq!(adapted.len(), 4);
        assert!((adapted[0] - 0.0).abs() < 1e-6);
        assert!((adapted[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn temp_wav_path_is_unique_and_wav_suffixed() {
        let first = temp_wav_path().expect("path");
        let second = temp_wav_path().expect("path");
        assert_ne!(first, second);
        assert_eq!(first.extension().and_then(|e| e.to_str()), Some("wav"));
    }

    #[test]
    fn close_clears_recorded_state() {
        let mut stt = Stt::with_defaults(None, false);
        stt.audio_samples = Some(vec![1, 2, 3]);
        stt.close();
        assert!(stt.audio_samples().is_none());
        assert!(stt.tempfile_path().is_none());

        let mut tts = Tts::with_defaults(None, false);
        tts.close();
        assert!(tts.tempfile_path().is_none());
    }

    #[test]
    fn remove_file_quietly_ignores_missing_paths() {
        remove_file_quietly(Path::new(""));
        remove_file_quietly(Path::new("windows-use-does-not-exist.wav"));
    }
}
