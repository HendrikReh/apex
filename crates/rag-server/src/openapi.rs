//! OpenAPI spec generation via utoipa.

use utoipa::OpenApi;

use crate::routes::{api_keys, chat, collections, health, ingest, search};
use crate::state::ErrorBody;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Apex RAG API",
        version = env!("CARGO_PKG_VERSION"),
        description = "API-first Retrieval-Augmented Generation platform",
        license(name = "AGPL-3.0", url = "https://www.gnu.org/licenses/agpl-3.0.html"),
    ),
    paths(
        health::health,
        health::readiness,
        search::search_dense,
        search::search_sparse,
        search::search_hybrid,
        ingest::ingest_paths,
        ingest::ingest_upload,
        chat::chat,
        collections::collection_stats,
        api_keys::create_service_account,
        api_keys::create_api_key,
        api_keys::revoke_api_key,
        api_keys::list_api_keys,
    ),
    components(schemas(
        search::SearchRequest,
        search::SearchResponse,
        search::SearchResult,
        search::HybridSearchRequest,
        search::HybridSearchResponse,
        search::HybridSearchResult,
        ingest::IngestPathsRequest,
        ingest::IngestResponse,
        ingest::FailureEntry,
        chat::ChatHttpRequest,
        chat::ChatHttpResponse,
        chat::CitationJson,
        chat::UsageJson,
        collections::CollectionStatsResponse,
        health::ReadinessResponse,
        health::ReadinessChecks,
        api_keys::CreateServiceAccountRequest,
        api_keys::CreateServiceAccountResponse,
        api_keys::CreateApiKeyRequest,
        api_keys::CreateApiKeyResponse,
        api_keys::ApiKeyListItem,
        ErrorBody,
    )),
    modifiers(&SecurityAddon),
    tags(
        (name = "Health", description = "Liveness and readiness probes"),
        (name = "Search", description = "Dense, sparse, and hybrid retrieval"),
        (name = "Ingest", description = "Document ingestion"),
        (name = "Chat", description = "RAG chat with citations"),
        (name = "Collections", description = "Collection management"),
        (name = "Auth", description = "API key and service account management"),
    )
)]
pub struct ApiDoc;

struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "api_key",
            utoipa::openapi::security::SecurityScheme::Http(
                utoipa::openapi::security::HttpBuilder::new()
                    .scheme(utoipa::openapi::security::HttpAuthScheme::Bearer)
                    .bearer_format("API Key or JWT")
                    .build(),
            ),
        );
    }
}
