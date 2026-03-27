-- Documents table: tenant-scoped document metadata (ADR-002)
CREATE TABLE IF NOT EXISTS documents (
    tenant        TEXT NOT NULL CHECK (tenant <> ''),
    id            TEXT NOT NULL,
    title         TEXT NOT NULL,
    language      TEXT,
    metadata      JSONB,
    source_path   TEXT,
    version       TEXT,
    checksum      TEXT,
    ingest_run_id UUID,
    token_count   BIGINT,
    collection    TEXT,
    stats_collection  TEXT,
    stats_token_count BIGINT,
    created_at    TIMESTAMPTZ DEFAULT now(),
    updated_at    TIMESTAMPTZ DEFAULT now(),
    PRIMARY KEY (tenant, id)
);

CREATE INDEX IF NOT EXISTS idx_documents_ingest_run_id ON documents(ingest_run_id);
CREATE INDEX IF NOT EXISTS idx_documents_tenant ON documents(tenant);

-- Chunks table: denormalized chunk text with FK to documents
CREATE TABLE IF NOT EXISTS chunks (
    id            BIGSERIAL PRIMARY KEY,
    tenant        TEXT NOT NULL CHECK (tenant <> ''),
    document_id   TEXT NOT NULL,
    chunk_index   INT NOT NULL,
    text          TEXT NOT NULL,
    created_at    TIMESTAMPTZ DEFAULT now(),
    FOREIGN KEY (tenant, document_id) REFERENCES documents(tenant, id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_chunks_tenant_document_chunk
    ON chunks(tenant, document_id, chunk_index);

-- Corpus stats for BM25 avgdl tracking
CREATE TABLE IF NOT EXISTS corpus_stats (
    tenant        TEXT NOT NULL,
    collection    TEXT NOT NULL,
    total_docs    BIGINT DEFAULT 0,
    total_tokens  BIGINT DEFAULT 0,
    avgdl         DOUBLE PRECISION GENERATED ALWAYS AS (
                      CASE WHEN total_docs > 0 THEN total_tokens::float / total_docs ELSE 0.0 END
                  ) STORED,
    updated_at    TIMESTAMPTZ DEFAULT now(),
    PRIMARY KEY (tenant, collection)
);
