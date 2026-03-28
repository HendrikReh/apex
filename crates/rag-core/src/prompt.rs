//! Prompt template loading and rendering via Handlebars.

use anyhow::{Context, Result};
use handlebars::Handlebars;
use serde::Serialize;

use crate::context::ContextChunk;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Context variables for the system prompt template.
#[derive(Debug, Serialize)]
pub struct PromptContext<'a> {
    pub context: &'a str,
    pub language_instruction: &'a str,
}

/// Loads and renders a Handlebars template for system prompts.
pub struct PromptRenderer {
    handlebars: Handlebars<'static>,
}

impl PromptRenderer {
    /// Load and compile a template from the given file path.
    ///
    /// Fails fast if the file is missing or the template is invalid.
    pub fn from_file(path: &str) -> Result<Self> {
        let template = std::fs::read_to_string(path)
            .with_context(|| format!("reading prompt template from {path}"))?;
        let mut handlebars = Handlebars::new();
        handlebars.set_strict_mode(true);
        handlebars
            .register_template_string("system", &template)
            .with_context(|| format!("compiling prompt template from {path}"))?;
        Ok(Self { handlebars })
    }

    /// Render the system prompt with the given context.
    pub fn render_system_prompt(&self, context: &PromptContext<'_>) -> Result<String> {
        self.handlebars.render("system", context).context("rendering system prompt template")
    }
}

// ---------------------------------------------------------------------------
// Context chunk rendering
// ---------------------------------------------------------------------------

/// Render context chunks as a numbered list for the LLM prompt.
///
/// Produces format: `[1] <text>\n---\n[2] <text>\n---\n...`
/// Returns empty string for empty input.
pub fn render_context_chunks(chunks: &[ContextChunk]) -> String {
    if chunks.is_empty() {
        return String::new();
    }
    chunks
        .iter()
        .enumerate()
        .map(|(i, chunk)| format!("[{}] {}", i + 1, chunk.text))
        .collect::<Vec<_>>()
        .join("\n---\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Citation;

    fn make_chunk(text: &str) -> ContextChunk {
        ContextChunk {
            chunk_id: "id".into(),
            text: text.into(),
            document_id: "doc".into(),
            chunk_index: 0,
            fused_score: 1.0,
            token_count: 5,
            citation: Some(Citation {
                chunk_id: "id".into(),
                document_id: "doc".into(),
                chunk_index: 0,
                sources: vec!["dense".into()],
            }),
        }
    }

    #[test]
    fn render_context_chunks_numbered() {
        let chunks = vec![
            make_chunk("First chunk text"),
            make_chunk("Second chunk text"),
            make_chunk("Third chunk text"),
        ];
        let rendered = render_context_chunks(&chunks);
        assert_eq!(
            rendered,
            "[1] First chunk text\n---\n[2] Second chunk text\n---\n[3] Third chunk text"
        );
    }

    #[test]
    fn render_context_chunks_empty() {
        assert_eq!(render_context_chunks(&[]), "");
    }

    #[test]
    fn render_context_chunks_single() {
        let chunks = vec![make_chunk("Only chunk")];
        assert_eq!(render_context_chunks(&chunks), "[1] Only chunk");
    }

    #[test]
    fn prompt_renderer_from_file_missing() {
        let result = PromptRenderer::from_file("/tmp/nonexistent-prompt-template.hbs");
        assert!(result.is_err());
    }

    #[test]
    #[allow(clippy::disallowed_methods)] // test assertions
    fn prompt_renderer_renders_template() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("test.hbs");
        std::fs::write(
            &path,
            "You are a helpful assistant.\n\n{{context}}\n\n{{language_instruction}}",
        )
        .expect("write");

        let renderer = PromptRenderer::from_file(path.to_str().expect("path")).expect("from_file");
        let result = renderer
            .render_system_prompt(&PromptContext {
                context: "[1] Some context",
                language_instruction: "Respond in English",
            })
            .expect("render");

        assert!(result.contains("[1] Some context"));
        assert!(result.contains("Respond in English"));
    }

    #[test]
    #[allow(clippy::disallowed_methods)] // test assertions
    fn prompt_renderer_empty_language_instruction() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("test.hbs");
        std::fs::write(&path, "Assistant.\n\n{{context}}\n\n{{language_instruction}}")
            .expect("write");

        let renderer = PromptRenderer::from_file(path.to_str().expect("path")).expect("from_file");
        let result = renderer
            .render_system_prompt(&PromptContext { context: "[1] Chunk", language_instruction: "" })
            .expect("render");

        assert!(result.contains("[1] Chunk"));
        // Empty language_instruction should not leave artifacts.
        assert!(!result.contains("{{"));
    }
}
