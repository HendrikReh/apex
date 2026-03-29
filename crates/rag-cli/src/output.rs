//! Shared output formatting (human-friendly vs JSON).

use std::io::Write;

use serde::Serialize;

/// Print a value as JSON or human-friendly text.
///
/// If `json_mode` is true, serialize `value` as JSON to `writer`.
/// Otherwise, call `human_fn` to produce human-readable output.
pub fn print_or_json<W: Write, T: Serialize>(
    writer: &mut W,
    json_mode: bool,
    value: &T,
    human_fn: impl FnOnce(&T, &mut W) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    if json_mode {
        serde_json::to_writer_pretty(&mut *writer, value)?;
        writeln!(writer)?;
        Ok(())
    } else {
        human_fn(value, writer)
    }
}

/// Render an error for CLI stderr output.
///
/// If the error is a `ClientError`, produce the spec-defined format.
/// Otherwise fall back to anyhow's display chain.
pub fn render_error(err: &anyhow::Error) -> String {
    if let Some(ce) = err.downcast_ref::<rag_client::ClientError>() {
        match ce {
            rag_client::ClientError::InvalidBaseUrl(msg) => format!("Error: {msg}"),
            rag_client::ClientError::InvalidTenant(msg) => format!("Error: {msg}"),
            rag_client::ClientError::Transport(e) => {
                format!("Error: connection failed — {e}")
            }
            rag_client::ClientError::HttpStatus { status, body, .. } => {
                format!("Error: server returned {status} — {body}")
            }
            rag_client::ClientError::Decode(e) => {
                format!("Error: unexpected server response — {e}")
            }
            rag_client::ClientError::Validation(msg) => format!("Error: {msg}"),
        }
    } else {
        format!("Error: {err:#}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::disallowed_methods)]
    #[test]
    fn print_or_json_human() {
        let mut buf = Vec::new();
        let value = serde_json::json!({"key": "val"});
        let called = std::cell::Cell::new(false);

        print_or_json(&mut buf, false, &value, |_v, w| {
            called.set(true);
            writeln!(w, "human output")?;
            Ok(())
        })
        .unwrap();

        assert!(called.get());
        assert_eq!(String::from_utf8(buf).unwrap(), "human output\n");
    }

    #[allow(clippy::disallowed_methods)]
    #[test]
    fn print_or_json_json() {
        let mut buf = Vec::new();
        let value = serde_json::json!({"key": "val"});

        print_or_json(&mut buf, true, &value, |_v, _w| {
            panic!("human_fn should not be called in JSON mode");
        })
        .unwrap();

        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("\"key\""));
        assert!(output.contains("\"val\""));
    }

    #[test]
    fn print_or_json_write_error() {
        struct BrokenWriter;
        impl Write for BrokenWriter {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken"))
            }
        }

        let value = serde_json::json!({"key": "val"});
        let result = print_or_json(&mut BrokenWriter, true, &value, |_v, _w| Ok(()));
        assert!(result.is_err());
    }

    #[test]
    fn render_error_http_status() {
        let ce = rag_client::ClientError::HttpStatus {
            status: 400,
            url: "http://localhost/chat".into(),
            body: "query must not be empty".into(),
        };
        let err: anyhow::Error = ce.into();
        let msg = super::render_error(&err);
        assert_eq!(msg, "Error: server returned 400 — query must not be empty");
    }

    #[allow(clippy::disallowed_methods)]
    #[test]
    fn render_error_transport() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let reqwest_err = rt.block_on(async {
            reqwest::Client::new()
                .get("http://127.0.0.1:1")
                .send()
                .await
                .unwrap_err()
        });
        let ce = rag_client::ClientError::Transport(reqwest_err);
        let err: anyhow::Error = ce.into();
        let msg = super::render_error(&err);
        assert!(msg.starts_with("Error: connection failed — "), "got: {msg}");
    }

    #[test]
    fn render_error_validation() {
        let ce = rag_client::ClientError::Validation("collection must not be empty".into());
        let err: anyhow::Error = ce.into();
        let msg = super::render_error(&err);
        assert_eq!(msg, "Error: collection must not be empty");
    }

    #[test]
    fn render_error_non_client() {
        let err = anyhow::anyhow!("something else went wrong");
        let msg = super::render_error(&err);
        assert_eq!(msg, "Error: something else went wrong");
    }
}
