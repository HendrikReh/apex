//! `api-key generate` — generate a new API key in the expected server format.

use std::io::Write;

use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use sha2::{Digest, Sha256};

const KEY_MARKER: &str = "apex";
const PREFIX_LEN: usize = 8;
const SECRET_BYTES: usize = 32;
const ALPHANUMERIC: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

#[allow(clippy::disallowed_methods)]
pub fn run(stdout: &mut impl Write, json: bool) -> Result<()> {
    let mut rng = rand::rng();

    let prefix: String = (0..PREFIX_LEN)
        .map(|_| {
            let idx = rng.random_range(0..ALPHANUMERIC.len());
            ALPHANUMERIC[idx] as char
        })
        .collect();

    let mut secret_bytes = [0u8; SECRET_BYTES];
    rng.fill(&mut secret_bytes);
    let secret_encoded = URL_SAFE_NO_PAD.encode(secret_bytes);

    let full_key = format!("{KEY_MARKER}_{prefix}_{secret_encoded}");

    let mut hasher = Sha256::new();
    hasher.update(secret_encoded.as_bytes());
    let hash = hex::encode(hasher.finalize());

    if json {
        let output = serde_json::json!({
            "key": full_key,
            "prefix": prefix,
            "hash": hash,
        });
        writeln!(stdout, "{}", serde_json::to_string_pretty(&output)?)?;
    } else {
        writeln!(stdout, "API Key:  {full_key}")?;
        writeln!(stdout, "Prefix:   {prefix}")?;
        writeln!(stdout, "Hash:     {hash}")?;
        writeln!(stdout)?;
        writeln!(stdout, "Save this key now — it cannot be retrieved later.")?;
        writeln!(stdout, "Use the hash value when manually seeding the database.")?;
    }

    Ok(())
}
