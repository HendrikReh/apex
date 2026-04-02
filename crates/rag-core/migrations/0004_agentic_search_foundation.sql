-- Add a GIN index for chunk-level full-text search used by agentic retrieval.
CREATE INDEX IF NOT EXISTS idx_chunks_text_fts
    ON chunks
    USING GIN (to_tsvector('simple', text));
