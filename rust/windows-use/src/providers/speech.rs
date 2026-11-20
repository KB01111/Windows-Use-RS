//! Speech-to-text and text-to-speech providers, mirroring the Python `stt.py` and
//! `tts.py` modules for OpenAI, Google, Groq, ElevenLabs and Deepgram.
//!
//! Every provider is a thin JSON/REST client built on `reqwest`, so the crate needs
//! no vendor SDKs.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use reqwest::multipart::{Form, Part};
use serde_json::{json, Value};

use crate::providers::base::{env_var, BoxFuture, SttProvider, TtsProvider};

fn read_audio(path: &str) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("reading audio file {path}"))
}

fn audio_part(bytes: Vec<u8>, path: &str) -> Result<Part> {
    let filename = Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "audio.wav".to_string());
    let mime = match Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("wav")
        .to_lowercase()
        .as_str()
    {
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "webm" => "audio/webm",
        _ => "audio/wav",
    };
    Part::bytes(bytes)
        .file_name(filename)
        .mime_str(mime)
        .context("building the audio part")
}

/// OpenAI transcription through the Whisper endpoint.
pub struct SttOpenAI {
    model: String,
    api_key: Option<String>,
    base_url: Option<String>,
    language: Option<String>,
    temperature: f64,
    timeout: f64,
}

impl SttOpenAI {
    pub fn new(
        model: Option<String>,
        api_key: Option<String>,
        base_url: Option<String>,
        language: Option<String>,
    ) -> Self {
        Self {
            model: model.unwrap_or_else(|| "whisper-1".to_string()),
            api_key: api_key.or_else(|| env_var("OPENAI_API_KEY")),
            base_url: base_url.or_else(|| env_var("OPENAI_BASE_URL")),
            language,
            temperature: 0.0,
            timeout: 120.0,
        }
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/audio/transcriptions",
            self.base_url
                .clone()
                .unwrap_or_else(|| "https://api.openai.com/v1".to_string())
                .trim_end_matches('/')
        )
    }
}

impl SttProvider for SttOpenAI {
    fn model(&self) -> &str {
        &self.model
    }

    fn transcribe<'a>(&'a self, file_path: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let api_key = self
                .api_key
                .clone()
                .ok_or_else(|| anyhow!("OPENAI_API_KEY is not set"))?;
            let bytes = read_audio(file_path)?;
            let mut form = Form::new()
                .part("file", audio_part(bytes, file_path)?)
                .text("model", self.model.clone())
                .text("temperature", self.temperature.to_string());
            if let Some(language) = &self.language {
                form = form.text("language", language.clone());
            }

            let response = reqwest::Client::new()
                .post(self.endpoint())
                .bearer_auth(api_key)
                .multipart(form)
                .timeout(std::time::Duration::from_secs_f64(self.timeout))
                .send()
                .await
                .context("calling the transcription API")?;

            let status = response.status();
            let body: Value = response.json().await.unwrap_or(Value::Null);
            if !status.is_success() {
                return Err(anyhow!("transcription failed ({status}): {body}"));
            }
            Ok(body
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string())
        })
    }
}

/// Groq's Whisper-compatible transcription endpoint.
pub struct SttGroq {
    inner: SttOpenAI,
}

impl SttGroq {
    pub fn new(model: Option<String>, api_key: Option<String>, base_url: Option<String>) -> Self {
        let inner = SttOpenAI {
            model: model.unwrap_or_else(|| "whisper-large-v3-turbo".to_string()),
            api_key: api_key.or_else(|| env_var("GROQ_API_KEY")),
            base_url: base_url.or_else(|| Some("https://api.groq.com/openai/v1".to_string())),
            language: None,
            temperature: 0.0,
            timeout: 120.0,
        };
        Self { inner }
    }
}

impl SttProvider for SttGroq {
    fn model(&self) -> &str {
        self.inner.model()
    }

    fn transcribe<'a>(&'a self, file_path: &'a str) -> BoxFuture<'a, Result<String>> {
        self.inner.transcribe(file_path)
    }
}

/// Google Gemini speech transcription through `generateContent`.
pub struct SttGoogle {
    model: String,
    api_key: Option<String>,
    base_url: Option<String>,
    timeout: f64,
}

impl SttGoogle {
    pub fn new(model: Option<String>, api_key: Option<String>, base_url: Option<String>) -> Self {
        Self {
            model: model.unwrap_or_else(|| "gemini-2.5-flash".to_string()),
            api_key: api_key
                .or_else(|| env_var("GOOGLE_API_KEY"))
                .or_else(|| env_var("GEMINI_API_KEY")),
            base_url: base_url
                .or_else(|| Some("https://generativelanguage.googleapis.com/v1beta".to_string())),
            timeout: 120.0,
        }
    }
}

impl SttProvider for SttGoogle {
    fn model(&self) -> &str {
        &self.model
    }

    fn transcribe<'a>(&'a self, file_path: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            use base64::Engine;
            let api_key = self
                .api_key
                .clone()
                .ok_or_else(|| anyhow!("GOOGLE_API_KEY is not set"))?;
            let bytes = read_audio(file_path)?;
            let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
            let base = self
                .base_url
                .clone()
                .unwrap_or_else(|| "https://generativelanguage.googleapis.com/v1beta".to_string());
            let url = format!(
                "{}/models/{}:generateContent?key={api_key}",
                base.trim_end_matches('/'),
                self.model
            );

            let body = json!({
                "contents": [{
                    "parts": [
                        { "text": "Transcribe this audio exactly. Reply with the transcription only." },
                        { "inline_data": { "mime_type": "audio/wav", "data": encoded } }
                    ]
                }]
            });

            let response = reqwest::Client::new()
                .post(url)
                .json(&body)
                .timeout(std::time::Duration::from_secs_f64(self.timeout))
                .send()
                .await
                .context("calling the Gemini transcription API")?;
            let status = response.status();
            let value: Value = response.json().await.unwrap_or(Value::Null);
            if !status.is_success() {
                return Err(anyhow!("transcription failed ({status}): {value}"));
            }
            let text = value
                .pointer("/candidates/0/content/parts/0/text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string();
            Ok(text)
        })
    }
}

/// ElevenLabs Scribe transcription.
pub struct SttElevenLabs {
    model: String,
    api_key: Option<String>,
    base_url: Option<String>,
    timeout: f64,
}

impl SttElevenLabs {
    pub fn new(model: Option<String>, api_key: Option<String>, base_url: Option<String>) -> Self {
        Self {
            model: model.unwrap_or_else(|| "scribe_v2".to_string()),
            api_key: api_key.or_else(|| env_var("ELEVENLABS_API_KEY")),
            base_url: base_url.or_else(|| Some("https://api.elevenlabs.io/v1".to_string())),
            timeout: 120.0,
        }
    }
}

impl SttProvider for SttElevenLabs {
    fn model(&self) -> &str {
        &self.model
    }

    fn transcribe<'a>(&'a self, file_path: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let api_key = self
                .api_key
                .clone()
                .ok_or_else(|| anyhow!("ELEVENLABS_API_KEY is not set"))?;
            let bytes = read_audio(file_path)?;
            let form = Form::new()
                .part("file", audio_part(bytes, file_path)?)
                .text("model_id", self.model.clone());
            let url = format!(
                "{}/speech-to-text",
                self.base_url
                    .clone()
                    .unwrap_or_else(|| "https://api.elevenlabs.io/v1".to_string())
                    .trim_end_matches('/')
            );

            let response = reqwest::Client::new()
                .post(url)
                .header("xi-api-key", api_key)
                .multipart(form)
                .timeout(std::time::Duration::from_secs_f64(self.timeout))
                .send()
                .await
                .context("calling the ElevenLabs transcription API")?;
            let status = response.status();
            let value: Value = response.json().await.unwrap_or(Value::Null);
            if !status.is_success() {
                return Err(anyhow!("transcription failed ({status}): {value}"));
            }
            Ok(value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string())
        })
    }
}

/// Deepgram Nova transcription.
pub struct SttDeepgram {
    model: String,
    api_key: Option<String>,
    base_url: Option<String>,
    timeout: f64,
}

impl SttDeepgram {
    pub fn new(model: Option<String>, api_key: Option<String>, base_url: Option<String>) -> Self {
        Self {
            model: model.unwrap_or_else(|| "nova-2".to_string()),
            api_key: api_key.or_else(|| env_var("DEEPGRAM_API_KEY")),
            base_url: base_url.or_else(|| Some("https://api.deepgram.com/v1".to_string())),
            timeout: 120.0,
        }
    }
}

impl SttProvider for SttDeepgram {
    fn model(&self) -> &str {
        &self.model
    }

    fn transcribe<'a>(&'a self, file_path: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let api_key = self
                .api_key
                .clone()
                .ok_or_else(|| anyhow!("DEEPGRAM_API_KEY is not set"))?;
            let bytes = read_audio(file_path)?;
            let url = format!(
                "{}/listen?model={}&smart_format=true",
                self.base_url
                    .clone()
                    .unwrap_or_else(|| "https://api.deepgram.com/v1".to_string())
                    .trim_end_matches('/'),
                self.model
            );

            let response = reqwest::Client::new()
                .post(url)
                .header("Authorization", format!("Token {api_key}"))
                .header("Content-Type", "audio/wav")
                .body(bytes)
                .timeout(std::time::Duration::from_secs_f64(self.timeout))
                .send()
                .await
                .context("calling the Deepgram transcription API")?;
            let status = response.status();
            let value: Value = response.json().await.unwrap_or(Value::Null);
            if !status.is_success() {
                return Err(anyhow!("transcription failed ({status}): {value}"));
            }
            Ok(value
                .pointer("/results/channels/0/alternatives/0/transcript")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string())
        })
    }
}

/// OpenAI speech synthesis.
pub struct TtsOpenAI {
    model: String,
    voice: String,
    api_key: Option<String>,
    base_url: Option<String>,
    speed: f64,
    response_format: String,
    timeout: f64,
}

impl TtsOpenAI {
    pub fn new(
        model: Option<String>,
        voice: Option<String>,
        api_key: Option<String>,
        base_url: Option<String>,
    ) -> Self {
        Self {
            model: model.unwrap_or_else(|| "tts-1".to_string()),
            voice: voice.unwrap_or_else(|| "alloy".to_string()),
            api_key: api_key.or_else(|| env_var("OPENAI_API_KEY")),
            base_url: base_url.or_else(|| env_var("OPENAI_BASE_URL")),
            speed: 1.0,
            response_format: "wav".to_string(),
            timeout: 120.0,
        }
    }
}

impl TtsProvider for TtsOpenAI {
    fn model(&self) -> &str {
        &self.model
    }

    fn synthesize<'a>(
        &'a self,
        text: &'a str,
        output_path: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let api_key = self
                .api_key
                .clone()
                .ok_or_else(|| anyhow!("OPENAI_API_KEY is not set"))?;
            let url = format!(
                "{}/audio/speech",
                self.base_url
                    .clone()
                    .unwrap_or_else(|| "https://api.openai.com/v1".to_string())
                    .trim_end_matches('/')
            );
            let body = json!({
                "model": self.model,
                "voice": self.voice,
                "input": text,
                "speed": self.speed,
                "response_format": self.response_format,
            });

            let response = reqwest::Client::new()
                .post(url)
                .bearer_auth(api_key)
                .json(&body)
                .timeout(std::time::Duration::from_secs_f64(self.timeout))
                .send()
                .await
                .context("calling the speech API")?;
            let status = response.status();
            if !status.is_success() {
                let detail = response.text().await.unwrap_or_default();
                return Err(anyhow!("speech synthesis failed ({status}): {detail}"));
            }
            let bytes = response.bytes().await.context("reading synthesized audio")?;
            std::fs::write(output_path, bytes)
                .with_context(|| format!("writing {output_path}"))?;
            Ok(())
        })
    }
}

/// Groq's Orpheus speech synthesis.
pub struct TtsGroq {
    inner: TtsOpenAI,
}

impl TtsGroq {
    pub fn new(model: Option<String>, voice: Option<String>, api_key: Option<String>) -> Self {
        Self {
            inner: TtsOpenAI {
                model: model.unwrap_or_else(|| "canopylabs/orpheus-v1-english".to_string()),
                voice: voice.unwrap_or_else(|| "autumn".to_string()),
                api_key: api_key.or_else(|| env_var("GROQ_API_KEY")),
                base_url: Some("https://api.groq.com/openai/v1".to_string()),
                speed: 1.0,
                response_format: "wav".to_string(),
                timeout: 120.0,
            },
        }
    }
}

impl TtsProvider for TtsGroq {
    fn model(&self) -> &str {
        self.inner.model()
    }

    fn synthesize<'a>(
        &'a self,
        text: &'a str,
        output_path: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        self.inner.synthesize(text, output_path)
    }
}

/// Google Gemini speech synthesis.
pub struct TtsGoogle {
    model: String,
    voice: String,
    api_key: Option<String>,
    base_url: Option<String>,
    timeout: f64,
}

impl TtsGoogle {
    pub fn new(model: Option<String>, voice: Option<String>, api_key: Option<String>) -> Self {
        Self {
            model: model.unwrap_or_else(|| "gemini-2.5-flash-preview-tts".to_string()),
            voice: voice.unwrap_or_else(|| "Kore".to_string()),
            api_key: api_key
                .or_else(|| env_var("GOOGLE_API_KEY"))
                .or_else(|| env_var("GEMINI_API_KEY")),
            base_url: Some("https://generativelanguage.googleapis.com/v1beta".to_string()),
            timeout: 120.0,
        }
    }
}

impl TtsProvider for TtsGoogle {
    fn model(&self) -> &str {
        &self.model
    }

    fn synthesize<'a>(
        &'a self,
        text: &'a str,
        output_path: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            use base64::Engine;
            let api_key = self
                .api_key
                .clone()
                .ok_or_else(|| anyhow!("GOOGLE_API_KEY is not set"))?;
            let url = format!(
                "{}/models/{}:generateContent?key={api_key}",
                self.base_url
                    .clone()
                    .unwrap_or_else(|| "https://generativelanguage.googleapis.com/v1beta".to_string())
                    .trim_end_matches('/'),
                self.model
            );
            let body = json!({
                "contents": [{ "parts": [{ "text": text }] }],
                "generationConfig": {
                    "responseModalities": ["AUDIO"],
                    "speechConfig": {
                        "voiceConfig": { "prebuiltVoiceConfig": { "voiceName": self.voice } }
                    }
                }
            });

            let response = reqwest::Client::new()
                .post(url)
                .json(&body)
                .timeout(std::time::Duration::from_secs_f64(self.timeout))
                .send()
                .await
                .context("calling the Gemini speech API")?;
            let status = response.status();
            let value: Value = response.json().await.unwrap_or(Value::Null);
            if !status.is_success() {
                return Err(anyhow!("speech synthesis failed ({status}): {value}"));
            }
            let encoded = value
                .pointer("/candidates/0/content/parts/0/inlineData/data")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("Gemini returned no audio data"))?;
            let pcm = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .context("decoding the audio payload")?;
            // Gemini returns raw 24 kHz mono PCM; wrap it in a WAV container.
            write_pcm_wav(output_path, &pcm, 24_000, 1)?;
            Ok(())
        })
    }
}

/// ElevenLabs speech synthesis.
pub struct TtsElevenLabs {
    model: String,
    voice: String,
    api_key: Option<String>,
    base_url: Option<String>,
    timeout: f64,
}

impl TtsElevenLabs {
    pub fn new(model: Option<String>, voice: Option<String>, api_key: Option<String>) -> Self {
        Self {
            model: model.unwrap_or_else(|| "eleven_multilingual_v2".to_string()),
            voice: voice.unwrap_or_else(|| "21m00Tcm4TlvDq8ikWAM".to_string()),
            api_key: api_key.or_else(|| env_var("ELEVENLABS_API_KEY")),
            base_url: Some("https://api.elevenlabs.io/v1".to_string()),
            timeout: 120.0,
        }
    }
}

impl TtsProvider for TtsElevenLabs {
    fn model(&self) -> &str {
        &self.model
    }

    fn synthesize<'a>(
        &'a self,
        text: &'a str,
        output_path: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let api_key = self
                .api_key
                .clone()
                .ok_or_else(|| anyhow!("ELEVENLABS_API_KEY is not set"))?;
            let url = format!(
                "{}/text-to-speech/{}?output_format=wav_44100",
                self.base_url
                    .clone()
                    .unwrap_or_else(|| "https://api.elevenlabs.io/v1".to_string())
                    .trim_end_matches('/'),
                self.voice
            );
            let body = json!({ "text": text, "model_id": self.model });

            let response = reqwest::Client::new()
                .post(url)
                .header("xi-api-key", api_key)
                .json(&body)
                .timeout(std::time::Duration::from_secs_f64(self.timeout))
                .send()
                .await
                .context("calling the ElevenLabs speech API")?;
            let status = response.status();
            if !status.is_success() {
                let detail = response.text().await.unwrap_or_default();
                return Err(anyhow!("speech synthesis failed ({status}): {detail}"));
            }
            let bytes = response.bytes().await.context("reading synthesized audio")?;
            std::fs::write(output_path, bytes).with_context(|| format!("writing {output_path}"))?;
            Ok(())
        })
    }
}

/// Deepgram Aura speech synthesis.
pub struct TtsDeepgram {
    model: String,
    api_key: Option<String>,
    base_url: Option<String>,
    timeout: f64,
}

impl TtsDeepgram {
    pub fn new(model: Option<String>, api_key: Option<String>) -> Self {
        Self {
            model: model.unwrap_or_else(|| "aura-2-thalia-en".to_string()),
            api_key: api_key.or_else(|| env_var("DEEPGRAM_API_KEY")),
            base_url: Some("https://api.deepgram.com/v1".to_string()),
            timeout: 120.0,
        }
    }
}

impl TtsProvider for TtsDeepgram {
    fn model(&self) -> &str {
        &self.model
    }

    fn synthesize<'a>(
        &'a self,
        text: &'a str,
        output_path: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let api_key = self
                .api_key
                .clone()
                .ok_or_else(|| anyhow!("DEEPGRAM_API_KEY is not set"))?;
            let url = format!(
                "{}/speak?model={}",
                self.base_url
                    .clone()
                    .unwrap_or_else(|| "https://api.deepgram.com/v1".to_string())
                    .trim_end_matches('/'),
                self.model
            );
            let response = reqwest::Client::new()
                .post(url)
                .header("Authorization", format!("Token {api_key}"))
                .header("Content-Type", "application/json")
                .json(&json!({ "text": text }))
                .timeout(std::time::Duration::from_secs_f64(self.timeout))
                .send()
                .await
                .context("calling the Deepgram speech API")?;
            let status = response.status();
            if !status.is_success() {
                let detail = response.text().await.unwrap_or_default();
                return Err(anyhow!("speech synthesis failed ({status}): {detail}"));
            }
            let bytes = response.bytes().await.context("reading synthesized audio")?;
            std::fs::write(output_path, bytes).with_context(|| format!("writing {output_path}"))?;
            Ok(())
        })
    }
}

/// Writes raw PCM samples into a WAV container.
pub fn write_pcm_wav(path: &str, pcm: &[u8], sample_rate: u32, channels: u16) -> Result<()> {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer =
        hound::WavWriter::create(path, spec).with_context(|| format!("creating {path}"))?;
    for chunk in pcm.chunks_exact(2) {
        let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
        writer
            .write_sample(sample)
            .context("writing a PCM sample")?;
    }
    writer.finalize().context("finalizing the WAV file")?;
    Ok(())
}

/// Builds an STT provider by name.
pub fn stt_provider(
    provider: &str,
    model: Option<String>,
    api_key: Option<String>,
) -> Option<Box<dyn SttProvider>> {
    Some(match provider {
        "openai" => Box::new(SttOpenAI::new(model, api_key, None, None)),
        "groq" => Box::new(SttGroq::new(model, api_key, None)),
        "google" => Box::new(SttGoogle::new(model, api_key, None)),
        "elevenlabs" => Box::new(SttElevenLabs::new(model, api_key, None)),
        "deepgram" => Box::new(SttDeepgram::new(model, api_key, None)),
        _ => return None,
    })
}

/// Builds a TTS provider by name.
pub fn tts_provider(
    provider: &str,
    model: Option<String>,
    voice: Option<String>,
    api_key: Option<String>,
) -> Option<Box<dyn TtsProvider>> {
    Some(match provider {
        "openai" => Box::new(TtsOpenAI::new(model, voice, api_key, None)),
        "groq" => Box::new(TtsGroq::new(model, voice, api_key)),
        "google" => Box::new(TtsGoogle::new(model, voice, api_key)),
        "elevenlabs" => Box::new(TtsElevenLabs::new(model, voice, api_key)),
        "deepgram" => Box::new(TtsDeepgram::new(model, api_key)),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn providers_construct_without_keys() {
        for name in ["openai", "groq", "google", "elevenlabs", "deepgram"] {
            assert!(stt_provider(name, None, Some("key".to_string())).is_some());
            assert!(tts_provider(name, None, None, Some("key".to_string())).is_some());
        }
        assert!(stt_provider("nope", None, None).is_none());
        assert!(tts_provider("nope", None, None, None).is_none());
    }

    #[test]
    fn default_models_match_python() {
        let stt = SttOpenAI::new(None, Some("k".to_string()), None, None);
        assert_eq!(stt.model(), "whisper-1");
        let tts = TtsOpenAI::new(None, None, Some("k".to_string()), None);
        assert_eq!(tts.model(), "tts-1");
        let deepgram = SttDeepgram::new(None, Some("k".to_string()), None);
        assert_eq!(deepgram.model(), "nova-2");
    }
}
