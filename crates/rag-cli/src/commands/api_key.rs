//! `api-key generate` — generate a new API key in the expected server format.

use std::io::Write;

use anyhow::Result;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use sha2::{Digest, Sha256};

use crate::output::print_or_json;

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

    let output = serde_json::json!({
        "key": full_key,
        "prefix": prefix,
        "hash": hash,
    });
    print_or_json(stdout, json, &output, |value, writer| {
        let key = value["key"].as_str().ok_or_else(|| anyhow::anyhow!("missing key field"))?;
        let prefix =
            value["prefix"].as_str().ok_or_else(|| anyhow::anyhow!("missing prefix field"))?;
        let hash = value["hash"].as_str().ok_or_else(|| anyhow::anyhow!("missing hash field"))?;

        writeln!(writer, "API Key:  {key}")?;
        writeln!(writer, "Prefix:   {prefix}")?;
        writeln!(writer, "Hash:     {hash}")?;
        writeln!(writer)?;
        writeln!(writer, "Save this key now — it cannot be retrieved later.")?;
        writeln!(writer, "Use the hash value when manually seeding the database.")?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn generate_human_output_has_expected_shape() {
        let mut buf = Vec::new();
        run(&mut buf, false).expect("generate human output");

        let output = String::from_utf8(buf).expect("utf8");
        let lines: Vec<&str> = output.lines().collect();
        assert!(lines[0].starts_with("API Key:  apex_"));
        assert!(lines[1].starts_with("Prefix:   "));
        assert!(lines[2].starts_with("Hash:     "));
        assert_eq!(lines[1]["Prefix:   ".len()..].len(), PREFIX_LEN);
        assert_eq!(lines[2]["Hash:     ".len()..].len(), 64);
        assert!(output.contains("Save this key now"));
        assert!(output.contains("manually seeding the database"));
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn generate_json_output_has_expected_fields() {
        let mut buf = Vec::new();
        run(&mut buf, true).expect("generate json output");

        let value: serde_json::Value = serde_json::from_slice(&buf).expect("json");
        let key = value["key"].as_str().expect("key");
        let prefix = value["prefix"].as_str().expect("prefix");
        let hash = value["hash"].as_str().expect("hash");

        assert!(key.starts_with("apex_"));
        assert_eq!(prefix.len(), PREFIX_LEN);
        assert_eq!(hash.len(), 64);
        assert!(key.contains(prefix));
    }
}
