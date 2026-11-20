//! CLI provider config storage, mirroring `windows_use/cli/config.py`.
//!
//! The JSON file holds an array of provider entries. API keys are encrypted with
//! Fernet, using a key derived from the user's home directory, so the file is only
//! readable on the machine that wrote it and stays byte-compatible with the Python
//! implementation:
//!
//! * key = `urlsafe_b64encode(PBKDF2-HMAC-SHA256(home.as_posix(), b"windows-use-cli-v1", 480_000, 32))`
//! * token = `urlsafe_b64(0x80 || u64_be(timestamp) || iv[16] || AES-128-CBC(PKCS7(plaintext)) || HMAC-SHA256(...))`
//!
//! Timestamps are deliberately not validated on decrypt, matching Fernet's own
//! TTL-free `decrypt` call used by Python.

use std::fs;
use std::path::{Path, PathBuf};

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use base64::engine::general_purpose::URL_SAFE;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{Map, Value};
use sha2::Sha256;

type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;
type HmacSha256 = Hmac<Sha256>;

/// Salt for the PBKDF2 key derivation, shared with the Python implementation.
pub const ENC_SALT: &[u8] = b"windows-use-cli-v1";
/// PBKDF2 iteration count, shared with the Python implementation.
pub const PBKDF2_ITERATIONS: u32 = 480_000;
/// Derived key length in bytes (AES-128 key + HMAC-SHA256 key).
pub const KEY_LENGTH: usize = 32;
/// Fernet token version byte.
pub const FERNET_VERSION: u8 = 0x80;
/// Environment variable overriding the config directory (used by the tests).
pub const CONFIG_DIR_ENV: &str = "WINDOWS_USE_CONFIG_DIR";

/// The config directory: `$WINDOWS_USE_CONFIG_DIR` when set, else `~/.windows-use`.
///
/// The environment override exists so tests never touch the real home directory;
/// production code uses the same `~/.windows-use` path as the Python CLI.
pub fn config_dir() -> PathBuf {
    if let Some(custom) = std::env::var_os(CONFIG_DIR_ENV) {
        let path = PathBuf::from(custom);
        if !path.as_os_str().is_empty() {
            return path;
        }
    }
    home_dir().join(".windows-use")
}

/// `~/.windows-use/config.json`.
pub fn config_file() -> PathBuf {
    config_dir().join("config.json")
}

/// `~/.windows-use/speech_config.json`.
pub fn speech_config_file() -> PathBuf {
    config_dir().join("speech_config.json")
}

/// The user's home directory, the PBKDF2 password in the Python implementation.
///
/// `Path.home()` is `USERPROFILE` on Windows and `HOME` elsewhere; both are checked
/// so a Unix-hosted test sees the same value as `pathlib`.
pub fn home_dir() -> PathBuf {
    for name in ["USERPROFILE", "HOME"] {
        if let Some(value) = std::env::var_os(name) {
            let path = PathBuf::from(value);
            if !path.as_os_str().is_empty() {
                return path;
            }
        }
    }
    PathBuf::from(".")
}

/// Renders a path the way Python's `Path.as_posix()` does.
///
/// Windows separators become `/`, so `C:\Users\kevin` derives the same key as the
/// Python CLI running on the same machine.
pub fn path_as_posix(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Derives the 32-byte Fernet key from the home directory.
pub fn derive_key() -> [u8; KEY_LENGTH] {
    derive_key_for(&home_dir())
}

/// Derives the 32-byte Fernet key from an explicit path.
///
/// PBKDF2 with 480,000 iterations costs tens of milliseconds, and the key depends
/// only on the path, so the last derivation is memoised. The result is identical to
/// deriving every time; only the latency differs.
pub fn derive_key_for(home: &Path) -> [u8; KEY_LENGTH] {
    let password = path_as_posix(home);

    if let Some(cached) = cached_key(&password) {
        return cached;
    }

    let mut key = [0u8; KEY_LENGTH];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), ENC_SALT, PBKDF2_ITERATIONS, &mut key);
    store_cached_key(&password, &key);
    key
}

/// Memoised derived keys, keyed by the PBKDF2 password.
static KEY_CACHE: std::sync::Mutex<Vec<(String, [u8; KEY_LENGTH])>> =
    std::sync::Mutex::new(Vec::new());

/// Returns a previously derived key for `password`, when one was cached.
fn cached_key(password: &str) -> Option<[u8; KEY_LENGTH]> {
    let cache = KEY_CACHE.lock().ok()?;
    cache
        .iter()
        .find(|(cached, _)| cached == password)
        .map(|(_, key)| *key)
}

/// Records a derived key so later calls skip PBKDF2.
fn store_cached_key(password: &str, key: &[u8; KEY_LENGTH]) {
    if let Ok(mut cache) = KEY_CACHE.lock() {
        // Only the home directory is ever used in practice, so the list stays tiny.
        cache.push((password.to_string(), *key));
    }
}

/// Splits the derived key the way `cryptography.fernet.Fernet` does.
///
/// Fernet reads the 32 decoded bytes as `signing_key = key[:16]` and
/// `encryption_key = key[16:]` – the HMAC half comes *first*, which is the reverse
/// of the naive reading of the spec.
fn split_key(key: &[u8; KEY_LENGTH]) -> (&[u8], &[u8]) {
    let (signing_key, encryption_key) = key.split_at(16);
    (encryption_key, signing_key)
}

/// Encrypts a secret, returning a urlsafe-base64 Fernet token.
///
/// An empty plaintext returns an empty string, matching the Python guard
/// (`if not plain: return ""`).
pub fn encrypt_secret(plain: &str) -> String {
    if plain.is_empty() {
        return String::new();
    }
    encrypt_secret_with_key(plain, &derive_key())
}

/// Decrypts a Fernet token, returning `None` on any failure.
pub fn decrypt_secret(encrypted: &str) -> Option<String> {
    if encrypted.is_empty() {
        return None;
    }
    decrypt_secret_with_key(encrypted, &derive_key())
}

/// [`encrypt_secret`] with an explicit derived key.
pub fn encrypt_secret_with_key(plain: &str, key: &[u8; KEY_LENGTH]) -> String {
    if plain.is_empty() {
        return String::new();
    }

    // Fernet uses the current UNIX time in whole seconds, big endian.
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    let mut iv = [0u8; 16];
    let random = uuid::Uuid::new_v4().into_bytes();
    iv.copy_from_slice(&random[..16]);

    let (encryption_key, signing_key) = split_key(key);

    let ciphertext = Aes128CbcEnc::new(encryption_key.into(), (&iv).into())
        .encrypt_padded_vec_mut::<Pkcs7>(plain.as_bytes());

    let mut token = Vec::with_capacity(1 + 8 + 16 + ciphertext.len() + 32);
    token.push(FERNET_VERSION);
    token.extend_from_slice(&timestamp.to_be_bytes());
    token.extend_from_slice(&iv);
    token.extend_from_slice(&ciphertext);

    let mut mac = HmacSha256::new_from_slice(signing_key).expect("HMAC accepts any key length");
    mac.update(&token);
    token.extend_from_slice(&mac.finalize().into_bytes());

    URL_SAFE.encode(token)
}

/// [`decrypt_secret`] with an explicit derived key.
pub fn decrypt_secret_with_key(encrypted: &str, key: &[u8; KEY_LENGTH]) -> Option<String> {
    let raw = URL_SAFE.decode(encrypted.trim()).ok()?;

    // version + timestamp + iv + at least one cipher block + hmac
    if raw.len() < 1 + 8 + 16 + 16 + 32 {
        return None;
    }
    if raw[0] != FERNET_VERSION {
        return None;
    }

    let (encryption_key, signing_key) = split_key(key);
    let (signed, signature) = raw.split_at(raw.len() - 32);

    let mut mac = HmacSha256::new_from_slice(signing_key).ok()?;
    mac.update(signed);
    // `verify_slice` is constant time and rejects any tampering.
    mac.verify_slice(signature).ok()?;

    let ciphertext = &signed[9 + 16..];
    let plaintext = Aes128CbcDec::new(encryption_key.into(), (&signed[9..25]).into())
        .decrypt_padded_vec_mut::<Pkcs7>(ciphertext)
        .ok()?;

    String::from_utf8(plaintext).ok()
}

/// Reads a JSON file, returning `None` when it is missing or unparseable.
fn read_json(path: &Path) -> Option<Value> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Writes pretty JSON with two-space indentation, like `json.dumps(indent=2)`.
///
/// No trailing newline is appended, so the bytes match what Python's
/// `Path.write_text(json.dumps(..., indent=2))` produces for the same content.
fn write_json(path: &Path, value: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    fs::write(path, text)
}

/// Loads the providers config array, returning an empty list when missing.
pub fn get_providers_config() -> Vec<Value> {
    match read_json(&config_file()) {
        Some(Value::Array(entries)) => entries,
        _ => Vec::new(),
    }
}

/// Returns the active provider config with the API key decrypted.
///
/// Mirrors Python: `api_key_encrypted` is removed and replaced by `api_key`, and
/// `base_url` is always present (as `null` when unset).
pub fn get_active_config() -> Option<Value> {
    for entry in get_providers_config() {
        let Value::Object(mut object) = entry else {
            continue;
        };
        if object.get("active") != Some(&Value::Bool(true)) {
            continue;
        }

        let encrypted = object
            .remove("api_key_encrypted")
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default();

        let api_key = if encrypted.is_empty() {
            Value::Null
        } else {
            match decrypt_secret(&encrypted) {
                Some(plain) => Value::String(plain),
                None => Value::Null,
            }
        };
        object.insert("api_key".to_string(), api_key);
        object.entry("base_url").or_insert(Value::Null);
        return Some(Value::Object(object));
    }
    None
}

/// Returns the decrypted API key for `provider`, when one is stored.
pub fn get_api_key(provider: &str) -> Option<String> {
    for entry in get_providers_config() {
        if entry.get("provider").and_then(Value::as_str) != Some(provider) {
            continue;
        }
        let encrypted = entry
            .get("api_key_encrypted")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if encrypted.is_empty() {
            return None;
        }
        return decrypt_secret(encrypted);
    }
    None
}

/// Encrypts the `api_key` of one entry, mirroring the Python save helper.
///
/// The plaintext key never reaches disk: it is dropped and replaced by
/// `api_key_encrypted`, which is always present afterwards.
fn seal_entry(entry: &Map<String, Value>) -> Map<String, Value> {
    let mut sealed = Map::new();
    for (key, value) in entry {
        if key == "api_key" || key == "api_key_encrypted" {
            continue;
        }
        sealed.insert(key.clone(), value.clone());
    }

    let plain = entry.get("api_key").and_then(Value::as_str).unwrap_or_default();
    let existing = entry
        .get("api_key_encrypted")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let encrypted = if !plain.is_empty() {
        encrypt_secret(plain)
    } else if !existing.is_empty() {
        existing.to_string()
    } else {
        String::new()
    };
    sealed.insert("api_key_encrypted".to_string(), Value::String(encrypted));
    sealed
}

/// Saves the providers config array, encrypting any plaintext `api_key`.
pub fn save_providers_config(configs: &[Value]) -> std::io::Result<()> {
    let mut out = Vec::with_capacity(configs.len());
    for config in configs {
        match config {
            Value::Object(object) => out.push(Value::Object(seal_entry(object))),
            other => out.push(other.clone()),
        }
    }
    write_json(&config_file(), &Value::Array(out))
}

/// Adds or updates a provider entry.
///
/// `api_key` and `base_url` are `Option` because Python distinguishes "not
/// provided" (`None`, leave untouched) from "cleared" (`""`, store empty).
pub fn upsert_provider(
    provider: &str,
    llm: &str,
    api_key: Option<&str>,
    base_url: Option<&str>,
    set_active: bool,
) -> std::io::Result<()> {
    let mut configs = get_providers_config();
    let mut updated = false;

    for config in configs.iter_mut() {
        let Value::Object(object) = config else {
            continue;
        };
        if object.get("provider").and_then(Value::as_str) != Some(provider) {
            continue;
        }

        object.insert("llm".to_string(), Value::String(llm.to_string()));
        if let Some(key) = api_key {
            let encrypted = if key.is_empty() {
                String::new()
            } else {
                encrypt_secret(key)
            };
            object.insert("api_key_encrypted".to_string(), Value::String(encrypted));
        }
        if let Some(url) = base_url {
            object.insert("base_url".to_string(), Value::String(url.to_string()));
        }
        if set_active {
            object.insert("active".to_string(), Value::Bool(true));
        }
        updated = true;
        break;
    }

    if !updated {
        let mut entry = Map::new();
        entry.insert("provider".to_string(), Value::String(provider.to_string()));
        entry.insert("llm".to_string(), Value::String(llm.to_string()));
        let encrypted = match api_key {
            Some(key) if !key.is_empty() => encrypt_secret(key),
            _ => String::new(),
        };
        entry.insert(
            "api_key_encrypted".to_string(),
            Value::String(encrypted),
        );
        entry.insert("active".to_string(), Value::Bool(set_active));
        if let Some(url) = base_url {
            entry.insert("base_url".to_string(), Value::String(url.to_string()));
        }
        configs.push(Value::Object(entry));
    }

    if set_active {
        for config in configs.iter_mut() {
            let Value::Object(object) = config else {
                continue;
            };
            if object.get("provider").and_then(Value::as_str) != Some(provider) {
                object.insert("active".to_string(), Value::Bool(false));
            }
        }
    }

    save_providers_config(&configs)
}

/// Updates the API key of an existing provider entry.
///
/// Returns the Python `ValueError` equivalent: an error when the provider is not
/// configured yet.
pub fn update_provider_api_key(provider: &str, api_key: &str) -> std::io::Result<()> {
    let mut configs = get_providers_config();
    for config in configs.iter_mut() {
        let Value::Object(object) = config else {
            continue;
        };
        if object.get("provider").and_then(Value::as_str) != Some(provider) {
            continue;
        }
        let encrypted = if api_key.is_empty() {
            String::new()
        } else {
            encrypt_secret(api_key)
        };
        object.insert("api_key_encrypted".to_string(), Value::String(encrypted));
        return save_providers_config(&configs);
    }
    Err(not_found(provider))
}

/// Updates the base URL of an existing provider entry.
pub fn update_provider_base_url(provider: &str, base_url: &str) -> std::io::Result<()> {
    let mut configs = get_providers_config();
    for config in configs.iter_mut() {
        let Value::Object(object) = config else {
            continue;
        };
        if object.get("provider").and_then(Value::as_str) != Some(provider) {
            continue;
        }
        let value = if base_url.is_empty() {
            Value::Null
        } else {
            Value::String(base_url.to_string())
        };
        object.insert("base_url".to_string(), value);
        return save_providers_config(&configs);
    }
    Err(not_found(provider))
}

/// The error Python raises for an unknown provider (`ValueError`).
fn not_found(provider: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("Provider '{provider}' not found in config"),
    )
}

/// Whether at least one provider is configured.
pub fn is_configured() -> bool {
    !get_providers_config().is_empty()
}

/// Loads the speech config object, returning an empty map when missing.
pub fn get_speech_config() -> Value {
    match read_json(&speech_config_file()) {
        Some(value @ Value::Object(_)) => value,
        _ => Value::Object(Map::new()),
    }
}

/// Unseals a speech section: decrypt `api_key_encrypted` into `api_key`.
fn unseal_section(section: &Value) -> Option<Value> {
    let Value::Object(object) = section else {
        return None;
    };
    if object.get("enabled") != Some(&Value::Bool(true)) {
        return None;
    }

    let mut out = object.clone();
    let encrypted = out
        .remove("api_key_encrypted")
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default();
    let api_key = if encrypted.is_empty() {
        Value::Null
    } else {
        match decrypt_secret(&encrypted) {
            Some(plain) => Value::String(plain),
            None => Value::Null,
        }
    };
    out.insert("api_key".to_string(), api_key);
    Some(Value::Object(out))
}

/// Returns the STT config with the API key decrypted, when it is enabled.
pub fn get_stt_config() -> Option<Value> {
    unseal_section(get_speech_config().get("stt")?)
}

/// Returns the TTS config with the API key decrypted, when it is enabled.
pub fn get_tts_config() -> Option<Value> {
    unseal_section(get_speech_config().get("tts")?)
}

/// Saves the speech config, merging with the existing file.
///
/// `None` leaves a section untouched, exactly like the Python default arguments.
pub fn save_speech_config(stt: Option<&Value>, tts: Option<&Value>) -> std::io::Result<()> {
    let mut config = get_speech_config();
    let Value::Object(ref mut object) = config else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "speech config is not a JSON object",
        ));
    };

    for (name, section) in [("stt", stt), ("tts", tts)] {
        let Some(section) = section else {
            continue;
        };
        let Value::Object(section) = section else {
            continue;
        };
        object.insert(name.to_string(), Value::Object(seal_entry(section)));
    }

    write_json(&speech_config_file(), &Value::Object(object.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Point the config functions at a scratch directory for one test.
    ///
    /// The variable is process-global, so every test that writes config also takes
    /// [`ENV_LOCK`] to stay deterministic under the parallel test harness.
    struct TempConfigDir {
        dir: PathBuf,
        previous: Option<std::ffi::OsString>,
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    impl TempConfigDir {
        fn new() -> (Self, std::sync::MutexGuard<'static, ()>) {
            let guard = ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let dir = std::env::temp_dir()
                .join(format!("windows-use-config-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&dir).expect("create scratch dir");
            let previous = std::env::var_os(CONFIG_DIR_ENV);
            std::env::set_var(CONFIG_DIR_ENV, &dir);
            (Self { dir, previous }, guard)
        }
    }

    impl Drop for TempConfigDir {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var(CONFIG_DIR_ENV, value),
                None => std::env::remove_var(CONFIG_DIR_ENV),
            }
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let secret = "sk-test-12345";
        let token = encrypt_secret(secret);
        assert!(!token.is_empty());
        assert_ne!(token, secret);
        assert_eq!(decrypt_secret(&token).as_deref(), Some(secret));
    }

    #[test]
    fn encrypt_empty_secret_returns_empty_string() {
        assert_eq!(encrypt_secret(""), "");
        assert_eq!(decrypt_secret(""), None);
    }

    #[test]
    fn decrypt_invalid_input_returns_none() {
        assert_eq!(decrypt_secret("not-a-token"), None);
        // Valid base64, wrong contents.
        assert_eq!(decrypt_secret("aGVsbG8gd29ybGQ="), None);
        // Truncated Fernet token.
        assert_eq!(decrypt_secret("gAAAAAB"), None);
        // Uppercase base64 alphabet is not urlsafe Fernet input.
        assert_eq!(decrypt_secret("!!!not base64!!!"), None);
    }

    #[test]
    fn decrypt_rejects_a_tampered_hmac() {
        let key = derive_key();
        let mut raw = URL_SAFE
            .decode(encrypt_secret_with_key("secret", &key))
            .expect("decode token");
        let last = raw.len() - 1;
        raw[last] ^= 0xff;
        let tampered = URL_SAFE.encode(raw);
        assert_eq!(decrypt_secret_with_key(&tampered, &key), None);
    }

    #[test]
    fn decrypt_rejects_the_wrong_key() {
        let token = encrypt_secret_with_key("secret", &[7u8; KEY_LENGTH]);
        assert_eq!(decrypt_secret_with_key(&token, &[8u8; KEY_LENGTH]), None);
    }

    #[test]
    fn derived_key_matches_the_python_reference_vector() {
        // `PBKDF2HMAC(SHA256, 32, b"windows-use-cli-v1", 480_000).derive(b"C:/Users/kevin")`
        let key = derive_key_for(Path::new(r"C:\Users\kevin"));
        let expected = "73f84aa2461495f723edf1b50cbcf4980f08f4d667a62fa26cda97c98b46fed3";
        let actual: String = key.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(actual, expected);

        // The base64 form is the actual Fernet key on the Python side.
        assert_eq!(
            URL_SAFE.encode(key),
            "c_hKokYUlfcj7fG1DLz0mA8I9NZnpi-ibNqXyYtG_tM="
        );
    }

    #[test]
    fn decrypts_a_token_produced_by_python_cryptography() {
        // Produced by `cryptography.fernet.Fernet` with the 32 raw key bytes below,
        // a fixed IV of 0x00..0x0f and timestamp 1_700_000_000:
        //   Fernet(b64(key))._encrypt_from_parts(b"sk-fixed-secret", ts, iv)
        // Decrypting it proves the token layout, the PKCS7 padding, the CBC
        // chaining and – critically – the signing/encryption key split all match.
        let key = *b"0123456789abcdef0123456789abcdef";
        let token = "gAAAAABlU_EAAAECAwQFBgcICQoLDA0OD-IX0S61SYQLlnCPjmZ3-Tbuva-9jTvHLxS7rnclhPDju7ixybbSrnzqtqPd8vZXYA==";
        assert_eq!(
            decrypt_secret_with_key(token, &key).as_deref(),
            Some("sk-fixed-secret")
        );
    }

    #[test]
    fn key_split_puts_the_signing_half_first() {
        // `cryptography.fernet.Fernet.__init__` assigns
        // `signing_key = key[:16]` and `encryption_key = key[16:]`.
        let mut key = [0u8; KEY_LENGTH];
        for (index, byte) in key.iter_mut().enumerate() {
            *byte = index as u8;
        }
        let (encryption_key, signing_key) = split_key(&key);
        assert_eq!(signing_key, &key[..16]);
        assert_eq!(encryption_key, &key[16..]);
    }

    #[test]
    fn token_layout_matches_the_fernet_spec() {
        let key = derive_key();
        let token = encrypt_secret_with_key("secret", &key);
        let raw = URL_SAFE.decode(&token).expect("decode token");

        assert_eq!(raw[0], FERNET_VERSION);
        // 1 version + 8 timestamp + 16 iv + 16 ciphertext (one padded block) + 32 hmac
        assert_eq!(raw.len(), 1 + 8 + 16 + 16 + 32);

        let timestamp = u64::from_be_bytes(raw[1..9].try_into().expect("timestamp bytes"));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs();
        assert!(timestamp.abs_diff(now) < 60, "timestamp {timestamp} vs now {now}");
    }

    #[test]
    fn path_as_posix_uses_forward_slashes() {
        assert_eq!(path_as_posix(Path::new(r"C:\Users\kevin")), "C:/Users/kevin");
        assert_eq!(path_as_posix(Path::new("/home/kevin")), "/home/kevin");
    }

    #[test]
    fn config_dir_honours_the_env_override() {
        let (_dir, _guard) = TempConfigDir::new();
        let expected = std::env::var_os(CONFIG_DIR_ENV).expect("override set");
        assert_eq!(config_dir(), PathBuf::from(expected));
        assert!(config_file().ends_with("config.json"));
        assert!(speech_config_file().ends_with("speech_config.json"));
    }

    #[test]
    fn missing_config_reads_as_empty() {
        let (_dir, _guard) = TempConfigDir::new();
        assert!(get_providers_config().is_empty());
        assert_eq!(get_active_config(), None);
        assert_eq!(get_api_key("openai"), None);
        assert!(!is_configured());
        assert_eq!(get_speech_config(), Value::Object(Map::new()));
        assert_eq!(get_stt_config(), None);
        assert_eq!(get_tts_config(), None);
    }

    #[test]
    fn save_upsert_and_read_roundtrip() {
        let (_dir, _guard) = TempConfigDir::new();

        upsert_provider("groq", "llama-3.3-70b-versatile", Some("gsk-secret"), None, true)
            .expect("upsert groq");
        assert!(is_configured());

        // The key must be encrypted on disk, never stored in the clear.
        let raw = fs::read_to_string(config_file()).expect("read config");
        assert!(!raw.contains("gsk-secret"));
        assert!(raw.contains("api_key_encrypted"));
        assert!(raw.contains("\n  {"));

        let active = get_active_config().expect("active config");
        assert_eq!(active["provider"], json!("groq"));
        assert_eq!(active["llm"], json!("llama-3.3-70b-versatile"));
        assert_eq!(active["api_key"], json!("gsk-secret"));
        assert_eq!(active["active"], json!(true));
        assert_eq!(active["base_url"], Value::Null);
        assert!(active.get("api_key_encrypted").is_none());

        assert_eq!(get_api_key("groq").as_deref(), Some("gsk-secret"));

        // A second provider becomes the only active one.
        upsert_provider(
            "openai",
            "gpt-5.2",
            Some("sk-openai"),
            Some("https://api.openai.com/v1"),
            true,
        )
        .expect("upsert openai");

        let active = get_active_config().expect("active config");
        assert_eq!(active["provider"], json!("openai"));
        assert_eq!(active["base_url"], json!("https://api.openai.com/v1"));

        let configs = get_providers_config();
        assert_eq!(configs.len(), 2);
        let groq = configs
            .iter()
            .find(|entry| entry["provider"] == json!("groq"))
            .expect("groq entry");
        assert_eq!(groq["active"], json!(false));
        assert_eq!(get_api_key("groq").as_deref(), Some("gsk-secret"));
    }

    #[test]
    fn upsert_without_an_api_key_keeps_the_existing_one() {
        let (_dir, _guard) = TempConfigDir::new();
        upsert_provider("openai", "gpt-4o", Some("sk-first"), None, true).expect("upsert");
        upsert_provider("openai", "gpt-5.2", None, None, true).expect("update model only");

        let active = get_active_config().expect("active config");
        assert_eq!(active["llm"], json!("gpt-5.2"));
        assert_eq!(active["api_key"], json!("sk-first"));
    }

    #[test]
    fn upsert_with_empty_base_url_clears_it() {
        let (_dir, _guard) = TempConfigDir::new();
        upsert_provider("openai", "gpt-4o", Some("k"), Some("https://x/v1"), true).expect("upsert");
        assert_eq!(
            get_active_config().expect("active")["base_url"],
            json!("https://x/v1")
        );

        upsert_provider("openai", "gpt-4o", None, Some(""), true).expect("clear base url");
        assert_eq!(get_active_config().expect("active")["base_url"], json!(""));
    }

    #[test]
    fn update_api_key_and_base_url() {
        let (_dir, _guard) = TempConfigDir::new();
        upsert_provider("groq", "llama-3.1-8b-instant", None, None, true).expect("upsert");
        assert_eq!(get_api_key("groq"), None);

        update_provider_api_key("groq", "gsk-new").expect("update key");
        assert_eq!(get_api_key("groq").as_deref(), Some("gsk-new"));

        update_provider_base_url("groq", "https://groq.example/v1").expect("update url");
        assert_eq!(
            get_active_config().expect("active")["base_url"],
            json!("https://groq.example/v1")
        );

        update_provider_base_url("groq", "").expect("clear url");
        assert_eq!(get_active_config().expect("active")["base_url"], Value::Null);

        let error = update_provider_api_key("nope", "k").unwrap_err();
        assert!(error.to_string().contains("not found in config"));
        let error = update_provider_base_url("nope", "u").unwrap_err();
        assert!(error.to_string().contains("not found in config"));
    }

    #[test]
    fn invalid_config_json_reads_as_empty() {
        let (dir, _guard) = TempConfigDir::new();
        fs::write(dir.dir.join("config.json"), "{not json").expect("write bad json");
        assert!(get_providers_config().is_empty());
        assert!(!is_configured());

        // A JSON object (not an array) is also rejected, like the Python isinstance check.
        fs::write(dir.dir.join("config.json"), "{}").expect("write object json");
        assert!(get_providers_config().is_empty());
    }

    #[test]
    fn save_preserves_unrecognised_keys() {
        let (_dir, _guard) = TempConfigDir::new();
        let configs = vec![json!({
            "provider": "openai",
            "llm": "gpt-4o",
            "api_key": "sk-plain",
            "active": true,
            "custom": "kept",
        })];
        save_providers_config(&configs).expect("save");

        let stored = get_providers_config();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0]["custom"], json!("kept"));
        assert!(stored[0].get("api_key").is_none());
        let encrypted = stored[0]["api_key_encrypted"]
            .as_str()
            .expect("encrypted key");
        assert_eq!(decrypt_secret(encrypted).as_deref(), Some("sk-plain"));
    }

    #[test]
    fn save_keeps_an_already_encrypted_key() {
        let (_dir, _guard) = TempConfigDir::new();
        let encrypted = encrypt_secret("sk-keep");
        let configs = vec![json!({
            "provider": "openai",
            "llm": "gpt-4o",
            "api_key_encrypted": encrypted,
            "active": true,
        })];
        save_providers_config(&configs).expect("save");
        assert_eq!(get_api_key("openai").as_deref(), Some("sk-keep"));
    }

    #[test]
    fn save_without_a_key_writes_an_empty_encrypted_field() {
        let (_dir, _guard) = TempConfigDir::new();
        let configs = vec![json!({
            "provider": "ollama",
            "llm": "llama3.3",
            "active": true,
        })];
        save_providers_config(&configs).expect("save");
        assert_eq!(get_providers_config()[0]["api_key_encrypted"], json!(""));
        assert_eq!(get_api_key("ollama"), None);
    }

    #[test]
    fn speech_config_roundtrip() {
        let (_dir, _guard) = TempConfigDir::new();

        let stt = json!({
            "enabled": true,
            "provider": "openai",
            "model": "whisper-1",
            "api_key": "sk-stt",
        });
        save_speech_config(Some(&stt), None).expect("save stt");

        let stored = get_speech_config();
        assert!(!stored.to_string().contains("sk-stt"));
        assert!(stored.get("tts").is_none(), "tts left untouched");

        let stt_config = get_stt_config().expect("stt config");
        assert_eq!(stt_config["provider"], json!("openai"));
        assert_eq!(stt_config["api_key"], json!("sk-stt"));
        assert_eq!(get_tts_config(), None);

        // Passing None leaves the section unchanged.
        let tts = json!({
            "enabled": true,
            "provider": "openai",
            "model": "tts-1",
            "voice": "alloy",
            "api_key": "sk-tts",
        });
        save_speech_config(None, Some(&tts)).expect("save tts");

        let tts_config = get_tts_config().expect("tts config");
        assert_eq!(tts_config["voice"], json!("alloy"));
        assert_eq!(tts_config["api_key"], json!("sk-tts"));
        assert_eq!(
            get_stt_config().expect("stt still there")["api_key"],
            json!("sk-stt")
        );
    }

    #[test]
    fn disabled_speech_sections_are_absent() {
        let (_dir, _guard) = TempConfigDir::new();
        let stt = json!({"enabled": false, "provider": "openai", "api_key": "sk"});
        save_speech_config(Some(&stt), None).expect("save");
        assert_eq!(get_stt_config(), None);
    }

    #[test]
    fn speech_config_supports_providers_without_keys() {
        let (_dir, _guard) = TempConfigDir::new();
        let tts = json!({"enabled": true, "provider": "ollama", "model": "tts-1"});
        save_speech_config(None, Some(&tts)).expect("save");
        let stored = get_tts_config().expect("tts config");
        assert_eq!(stored["api_key"], Value::Null);
        assert_eq!(stored["provider"], json!("ollama"));
    }
}
