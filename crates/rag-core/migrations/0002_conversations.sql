-- Users (anonymous or OIDC-authenticated)
CREATE TABLE IF NOT EXISTS users (
    id            UUID PRIMARY KEY,
    tenant        TEXT NOT NULL CHECK (tenant <> ''),
    email         TEXT,
    oidc_issuer   TEXT,
    oidc_subject  TEXT,
    created_at    TIMESTAMPTZ DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_users_oidc_identity
    ON users (tenant, oidc_issuer, oidc_subject);

CREATE UNIQUE INDEX IF NOT EXISTS idx_users_anonymous_identity
    ON users (tenant)
    WHERE email IS NULL AND oidc_issuer IS NULL AND oidc_subject IS NULL;

-- App sessions
CREATE TABLE IF NOT EXISTS app_sessions (
    id            UUID PRIMARY KEY,
    user_id       UUID REFERENCES users(id) ON DELETE CASCADE,
    created_at    TIMESTAMPTZ DEFAULT now(),
    last_seen     TIMESTAMPTZ DEFAULT now(),
    ip_hash       TEXT,
    user_agent    TEXT
);

CREATE INDEX IF NOT EXISTS idx_app_sessions_user_id ON app_sessions(user_id);

-- Conversations
CREATE TABLE IF NOT EXISTS conversations (
    id            UUID PRIMARY KEY,
    user_id       UUID REFERENCES users(id) ON DELETE CASCADE,
    tenant        TEXT NOT NULL CHECK (tenant <> ''),
    title         TEXT,
    collection    TEXT,
    created_at    TIMESTAMPTZ DEFAULT now(),
    archived_at   TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_conversations_user_id ON conversations(user_id);
CREATE INDEX IF NOT EXISTS idx_conversations_tenant ON conversations(tenant);

-- Messages within conversations
CREATE TABLE IF NOT EXISTS messages (
    id              UUID PRIMARY KEY,
    conversation_id UUID REFERENCES conversations(id) ON DELETE CASCADE,
    role            TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system')),
    content         TEXT NOT NULL,
    metadata        JSONB,
    created_at      TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_messages_conversation_id ON messages(conversation_id);

-- Runs (retrieval + generation executions)
CREATE TABLE IF NOT EXISTS runs (
    id              UUID PRIMARY KEY,
    conversation_id UUID REFERENCES conversations(id) ON DELETE CASCADE,
    user_id         UUID REFERENCES users(id) ON DELETE CASCADE,
    app_session_id  UUID REFERENCES app_sessions(id) ON DELETE SET NULL,
    tenant          TEXT NOT NULL CHECK (tenant <> ''),
    trace_id        TEXT,
    status          TEXT,
    meta            JSONB,
    created_at      TIMESTAMPTZ DEFAULT now(),
    completed_at    TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_runs_conversation_id ON runs(conversation_id);
CREATE INDEX IF NOT EXISTS idx_runs_tenant ON runs(tenant);
CREATE INDEX IF NOT EXISTS idx_runs_trace_id ON runs(trace_id);
CREATE INDEX IF NOT EXISTS idx_runs_user_id ON runs(user_id);
