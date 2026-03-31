-- Phase 10: Auth tables for service accounts, API keys, and OIDC principals.

CREATE TABLE service_accounts (
    id            UUID PRIMARY KEY,
    tenant        TEXT NOT NULL,
    name          TEXT NOT NULL,
    role          TEXT NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    disabled_at   TIMESTAMPTZ,
    UNIQUE (tenant, name)
);

CREATE TABLE api_keys (
    id                  UUID PRIMARY KEY,
    service_account_id  UUID NOT NULL REFERENCES service_accounts(id),
    key_prefix          TEXT NOT NULL UNIQUE,
    key_hash            TEXT NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at          TIMESTAMPTZ,
    revoked_at          TIMESTAMPTZ,
    last_used_at        TIMESTAMPTZ
);

CREATE INDEX idx_api_keys_service_account ON api_keys(service_account_id);

CREATE TABLE oidc_principals (
    id          UUID PRIMARY KEY,
    tenant      TEXT NOT NULL,
    issuer      TEXT NOT NULL,
    subject     TEXT NOT NULL,
    role        TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    disabled_at TIMESTAMPTZ,
    UNIQUE (tenant, issuer, subject)
);
