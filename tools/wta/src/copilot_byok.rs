//! Copilot BYOK environment isolation and Credential Manager resolution.

use anyhow::{bail, Context, Result};
use tokio::process::Command;

const BASE_URL: &str = "COPILOT_PROVIDER_BASE_URL";
const API_KEY: &str = "COPILOT_PROVIDER_API_KEY";
const PROVIDER_TYPE: &str = "COPILOT_PROVIDER_TYPE";
const MODEL: &str = "COPILOT_MODEL";
const OFFLINE: &str = "COPILOT_OFFLINE";
const CREDENTIAL_ID: &str = "WTA_COPILOT_CREDENTIAL_ID";

const PROVIDER_ENV_KEYS: &[&str] = &[
    BASE_URL,
    API_KEY,
    PROVIDER_TYPE,
    MODEL,
    OFFLINE,
    CREDENTIAL_ID,
];

struct Config {
    base_url: String,
    model: String,
    provider_type: String,
    api_key: Option<String>,
    credential_id: Option<String>,
}

impl Config {
    fn from_env() -> Self {
        Self {
            base_url: trimmed_env(BASE_URL).unwrap_or_default(),
            model: trimmed_env(MODEL).unwrap_or_default(),
            provider_type: trimmed_env(PROVIDER_TYPE).unwrap_or_else(|| "openai".to_string()),
            api_key: trimmed_env(API_KEY),
            credential_id: trimmed_env(CREDENTIAL_ID),
        }
    }

    fn is_complete(&self) -> bool {
        !self.base_url.is_empty() && !self.model.is_empty()
    }
}

/// Remove provider metadata from every child, then restore it only for a
/// Copilot child with a complete configuration.
pub(crate) fn configure_child(cmd: &mut Command, is_copilot: bool) -> Result<()> {
    let config = Config::from_env();
    for key in PROVIDER_ENV_KEYS {
        cmd.env_remove(key);
    }

    if !is_copilot || !config.is_complete() {
        return Ok(());
    }

    let api_key = match config.credential_id.as_deref() {
        Some(id) => Some(read_api_key(id)?),
        None => config.api_key,
    };

    cmd.env(BASE_URL, config.base_url)
        .env(MODEL, config.model)
        .env(PROVIDER_TYPE, config.provider_type)
        .env(OFFLINE, "true");
    if let Some(api_key) = api_key {
        cmd.env(API_KEY, api_key);
    }
    Ok(())
}

pub(crate) fn is_copilot_program(program: &str) -> bool {
    std::path::Path::new(program)
        .file_stem()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("copilot"))
}

fn trimmed_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn read_api_key(credential_id: &str) -> Result<String> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Security::Credentials::{
        CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC,
    };

    let target: Vec<u16> = format!("IntelligentTerminal.CopilotBYOK/{credential_id}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
    if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) } == 0 {
        let error = unsafe { GetLastError() };
        bail!("failed to read Copilot provider credential: Win32 error {error}");
    }
    if credential.is_null() {
        bail!("Credential Manager returned a null Copilot provider credential");
    }

    let blob_size = unsafe { (*credential).CredentialBlobSize as usize };
    let blob = unsafe { (*credential).CredentialBlob };
    if blob_size == 0 || blob.is_null() {
        unsafe { CredFree(credential.cast()) };
        bail!("Copilot provider credential is empty");
    }
    let bytes = unsafe {
        std::slice::from_raw_parts(blob, blob_size).to_vec()
    };
    unsafe { CredFree(credential.cast()) };

    let api_key = String::from_utf8(bytes)
        .context("Copilot provider credential is not valid UTF-8")?
        .trim()
        .to_string();
    if api_key.is_empty() {
        bail!("Copilot provider credential is empty");
    }
    Ok(api_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_copilot_program_names() {
        assert!(is_copilot_program("copilot"));
        assert!(is_copilot_program(r"C:\Tools\copilot.exe"));
        assert!(is_copilot_program(r"C:\Tools\Copilot.cmd"));
        assert!(!is_copilot_program("gemini"));
    }

    #[test]
    fn requires_endpoint_and_model() {
        let complete = Config {
            base_url: "http://localhost:11434".to_string(),
            model: "qwen3.5:9b".to_string(),
            provider_type: "openai".to_string(),
            api_key: None,
            credential_id: None,
        };
        assert!(complete.is_complete());

        assert!(!Config {
            model: String::new(),
            ..complete
        }
        .is_complete());
        assert!(!Config {
            base_url: String::new(),
            model: "qwen3.5:9b".to_string(),
            provider_type: "openai".to_string(),
            api_key: None,
            credential_id: None,
        }
        .is_complete());
    }
}
