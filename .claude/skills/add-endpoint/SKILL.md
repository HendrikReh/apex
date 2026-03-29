---
name: add-endpoint
description: Scaffold a new API endpoint with handler, route registration, OpenAPI entry, and test. Use this skill whenever adding a new route, creating a new API handler, or when the user mentions needing a new endpoint — even if they phrase it as "add a route" or "expose X via the API".
---

# Add API Endpoint

Scaffold a new endpoint following the project's Axum 0.7 conventions. This skill exists because the project has several coordinated touch-points (handler, route, OpenAPI spec, test) that must all stay in sync — missing any one causes documentation drift or broken routing.

## Information Needed

Ask the user for:
- **HTTP method and path** (e.g., `POST /admin/cleanup`)
- **Brief description** of what the endpoint does
- **Request/response types** (or "none" for simple endpoints)
- **Design considerations** — does it need pagination? streaming? idempotency?

## Step 1: Write the Handler

Create the handler in the appropriate module under `crates/rag-server/src/routes/`. Follow this template:

```rust
/// Brief description of what this endpoint does.
#[allow(clippy::disallowed_methods)] // tracing macros use .expect() internally
pub async fn my_handler(
    Extension(ctx): Extension<RequestContext>,
    State(state): State<Arc<AppState>>,
    Path(param): Path<String>,        // if path params needed
    Json(body): Json<MyRequest>,       // if request body needed
) -> Result<Json<MyResponse>, ApiError> {
    let tenant = ctx.tenant.clone();

    let result = state
        .service
        .do_something(&param, &tenant)
        .await
        .map_err(|e| ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("failed to do something: {e}"),
            request_id: ctx.request_id,
            tenant: Some(tenant.clone()),
        })?;

    Ok(Json(MyResponse { /* fields */ }))
}
```

Key conventions:
- Extract `RequestContext` via `Extension(ctx)` for request_id and tenant
- Extract shared state via `State(state): State<Arc<AppState>>`
- Return `Result<Json<T>, ApiError>` — never `.unwrap()` or `.expect()` (ADR-001)
- Add `#[allow(clippy::disallowed_methods)]` with comment if handler uses `tracing` macros or `serde_json::json!`

## Step 2: Register the Route

In the server's route configuration, add the route:

```rust
.route("/my-endpoint/:param", post(routes::module::my_handler))
```

Axum 0.7 uses `:param` syntax (NOT `{param}`). Multiple methods on the same path chain: `.route("/path", get(list).post(create))`.

If the route needs ordering relative to others (e.g., `/prompts/resolve` before `/prompts/:name`), place it above catch-all patterns.

## Step 3: Update OpenAPI Spec

Add the endpoint to `docs/openapi.yaml` (if it exists). Follow existing patterns:

```yaml
  /my-endpoint/{param}:
    post:
      summary: Brief description
      description: Detailed description. Requires tenant header.
      parameters:
      - $ref: '#/components/parameters/TenantHeader'
      - $ref: '#/components/parameters/RequestIdHeader'
      - name: param
        in: path
        required: true
        schema:
          type: string
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/MyRequest'
      responses:
        '200':
          description: Success
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/MyResponse'
```

Note: OpenAPI uses `{param}` syntax, unlike Axum's `:param`.

## Step 4: Write a Test

Use the `spawn_app` or `oneshot` pattern (see `/write-test` skill for full details):

```rust
#[tokio::test]
async fn test_my_endpoint_returns_expected() {
    let app = Router::new()
        .route("/my-endpoint/:param", post(my_handler))
        .with_state(state)
        .layer(Extension(test_ctx()));

    let res = app
        .oneshot(
            Request::builder()
                .uri("/my-endpoint/test-value")
                .method("POST")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
}
```

## Step 5: Verify

Run `cargo check -p rag-server` to confirm everything compiles.

## Cross-references

- `/write-test` — detailed test patterns (state capture)
- `/add-crate` — if the endpoint requires a new crate
