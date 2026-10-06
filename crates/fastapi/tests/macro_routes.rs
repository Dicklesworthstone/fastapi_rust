//! End-to-end coverage of the proc-macro route path as a *consumer* sees it:
//! `use fastapi_rust::prelude::*`, `#[get]`/`#[post]` handlers taking `&Cx`
//! and extractors, returning `Json<T>`, registered via the generated
//! `<name>_route()` entries, and exercised through `TestClient`.
//!
//! This is the README's headline example. It guards three things that were
//! broken together before 0.4.3: macro expansions resolving `fastapi_core::`
//! paths through the facade, `Json<T>` as a response type, and handler
//! futures being allowed to borrow the request for the call's duration.

// Optional extractors have Infallible errors; their generated wrappers must
// also compile cleanly for consumers that deny unreachable code.
#![deny(unreachable_code)]
// The handlers below intentionally mirror the README signatures (`async fn` that may
// not await, `Result<_, HttpError>`), so the pedantic lints about them are noise here.
#![allow(clippy::unused_async, clippy::result_large_err)]

use fastapi_rust::ResponseBody;
use fastapi_rust::prelude::*;
use fastapi_rust::testing::TestClient;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct CleanupEvents(Arc<Mutex<Vec<&'static str>>>);

impl CleanupEvents {
    fn record(&self, event: &'static str) {
        self.0.lock().expect("cleanup events mutex").push(event);
    }

    fn snapshot(&self) -> Vec<&'static str> {
        self.0.lock().expect("cleanup events mutex").clone()
    }
}

#[derive(Clone)]
struct InnerResource(CleanupEvents);

impl FromDependencyWithCleanup for InnerResource {
    type Value = Self;
    type Error = HttpError;

    async fn setup(
        ctx: &RequestContext,
        req: &mut Request,
    ) -> Result<(Self, Option<CleanupFn>), Self::Error> {
        let events = State::<CleanupEvents>::from_request(ctx, req)
            .await
            .map_err(|err| HttpError::internal().with_detail(err.to_string()))?
            .0;
        events.record("setup inner");
        let cleanup_events = events.clone();
        let cleanup: CleanupFn = Box::new(move || {
            Box::pin(async move {
                cleanup_events.record("cleanup inner");
            })
        });
        Ok((Self(events), Some(cleanup)))
    }
}

#[derive(Clone)]
struct OuterResource(CleanupEvents);

impl FromDependencyWithCleanup for OuterResource {
    type Value = Self;
    type Error = HttpError;

    async fn setup(
        ctx: &RequestContext,
        req: &mut Request,
    ) -> Result<(Self, Option<CleanupFn>), Self::Error> {
        let inner = DependsCleanup::<InnerResource>::from_request(ctx, req).await?;
        let events = inner.0.0;
        events.record("setup outer");
        let cleanup_events = events.clone();
        let cleanup: CleanupFn = Box::new(move || {
            Box::pin(async move {
                cleanup_events.record("cleanup outer");
            })
        });
        Ok((Self(events), Some(cleanup)))
    }
}

#[get("/cleanup")]
async fn cleanup_route(
    _cx: &Cx,
    outer: DependsCleanup<OuterResource>,
    inner: DependsCleanup<InnerResource>,
) -> &'static str {
    assert!(Arc::ptr_eq(&outer.0.0.0, &inner.0.0.0));
    outer.0.0.record("handler");
    "resource used"
}

#[test]
fn test_client_runs_cached_dependency_cleanup_in_lifo_order() {
    let events = CleanupEvents(Arc::new(Mutex::new(Vec::new())));
    let app = App::builder()
        .state(events.clone())
        .route_entry(cleanup_route_route())
        .build();
    let client = TestClient::new(app);
    let response = client.get("/cleanup").send();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.text(), "resource used");
    assert_eq!(
        events.snapshot(),
        [
            "setup inner",
            "setup outer",
            "handler",
            "cleanup outer",
            "cleanup inner"
        ]
    );
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
struct Item {
    id: i64,
    name: String,
    price: f64,
}

#[get("/cleanup-error")]
async fn cleanup_error(
    _cx: &Cx,
    inner: DependsCleanup<InnerResource>,
) -> Result<&'static str, HttpError> {
    inner.0.0.record("handler error");
    Err(HttpError::bad_request().with_detail("resource operation failed"))
}

#[get("/cleanup-query")]
async fn cleanup_query(
    _cx: &Cx,
    inner: DependsCleanup<InnerResource>,
    _query: Query<SearchParams>,
) -> &'static str {
    inner.0.0.record("handler");
    "valid query"
}

#[derive(Clone)]
struct FailedOuter;

impl FromDependencyWithCleanup for FailedOuter {
    type Value = Self;
    type Error = HttpError;

    async fn setup(
        ctx: &RequestContext,
        req: &mut Request,
    ) -> Result<(Self, Option<CleanupFn>), Self::Error> {
        let inner = DependsCleanup::<InnerResource>::from_request(ctx, req).await?;
        inner.0.0.record("outer setup error");
        Err(HttpError::bad_request().with_detail("outer acquisition failed"))
    }
}

#[get("/cleanup-setup-error")]
async fn cleanup_setup_error(_cx: &Cx, _outer: DependsCleanup<FailedOuter>) -> &'static str {
    "handler must not run"
}

#[get("/cleanup-uncached")]
async fn cleanup_uncached(
    _cx: &Cx,
    first: DependsCleanup<InnerResource, NoCache>,
    second: DependsCleanup<InnerResource, NoCache>,
) -> &'static str {
    assert!(Arc::ptr_eq(&first.0.0.0, &second.0.0.0));
    first.0.0.record("handler");
    "two acquisitions"
}

fn cleanup_client(events: &CleanupEvents) -> TestClient<App> {
    TestClient::new(
        App::builder()
            .state(events.clone())
            .route_entry(cleanup_error_route())
            .route_entry(cleanup_query_route())
            .route_entry(cleanup_setup_error_route())
            .route_entry(cleanup_uncached_route())
            .build(),
    )
}

#[test]
fn test_client_cleanup_preserves_handler_error_response() {
    let events = CleanupEvents(Arc::new(Mutex::new(Vec::new())));
    let client = cleanup_client(&events);
    let response = client.get("/cleanup-error").send();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: serde_json::Value = response.json().expect("error JSON");
    assert_eq!(error["detail"], "resource operation failed");
    assert_eq!(
        events.snapshot(),
        ["setup inner", "handler error", "cleanup inner"]
    );
}

#[test]
fn test_client_cleanup_runs_after_later_extractor_rejection() {
    let events = CleanupEvents(Arc::new(Mutex::new(Vec::new())));
    let client = cleanup_client(&events);
    let response = client.get("/cleanup-query?q=valid&limit=invalid").send();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let error: serde_json::Value = response.json().expect("validation JSON");
    assert!(
        !error["detail"]
            .as_array()
            .expect("validation errors")
            .is_empty()
    );
    assert_eq!(events.snapshot(), ["setup inner", "cleanup inner"]);
}

#[test]
fn test_client_cleanup_releases_inner_when_outer_setup_fails() {
    let events = CleanupEvents(Arc::new(Mutex::new(Vec::new())));
    let client = cleanup_client(&events);
    let response = client.get("/cleanup-setup-error").send();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: serde_json::Value = response.json().expect("error JSON");
    assert_eq!(error["detail"], "outer acquisition failed");
    assert_eq!(
        events.snapshot(),
        ["setup inner", "outer setup error", "cleanup inner"]
    );
}

#[test]
fn test_client_cleanup_releases_each_uncached_acquisition() {
    let events = CleanupEvents(Arc::new(Mutex::new(Vec::new())));
    let client = cleanup_client(&events);
    let response = client.get("/cleanup-uncached").send();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.text(), "two acquisitions");
    assert_eq!(
        events.snapshot(),
        [
            "setup inner",
            "setup inner",
            "handler",
            "cleanup inner",
            "cleanup inner"
        ]
    );
}

struct CleanupMiddleware(CleanupEvents);

struct AcquireAndStopMiddleware;

impl fastapi_rust::core::Middleware for AcquireAndStopMiddleware {
    fn before<'a>(
        &'a self,
        ctx: &'a RequestContext,
        req: &'a mut Request,
    ) -> fastapi_rust::core::BoxFuture<'a, fastapi_rust::core::ControlFlow> {
        Box::pin(async move {
            let inner = DependsCleanup::<InnerResource>::from_request(ctx, req)
                .await
                .expect("middleware resource");
            inner.0.0.record("middleware stop");
            fastapi_rust::core::ControlFlow::Break(
                Response::with_status(StatusCode::FORBIDDEN)
                    .body(ResponseBody::Bytes(b"stopped by middleware".to_vec())),
            )
        })
    }
}

#[test]
fn test_client_cleanup_releases_middleware_resource_after_short_circuit() {
    let events = CleanupEvents(Arc::new(Mutex::new(Vec::new())));
    let client = TestClient::new(
        App::builder()
            .state(events.clone())
            .middleware(AcquireAndStopMiddleware)
            .route_entry(cleanup_route_route())
            .build(),
    );
    let response = client.get("/cleanup").send();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(response.text(), "stopped by middleware");
    assert_eq!(
        events.snapshot(),
        ["setup inner", "middleware stop", "cleanup inner"]
    );
}

impl fastapi_rust::core::Middleware for CleanupMiddleware {
    fn after<'a>(
        &'a self,
        _ctx: &'a RequestContext,
        _req: &'a Request,
        response: Response,
    ) -> fastapi_rust::core::middleware::BoxFuture<'a, Response> {
        Box::pin(async move {
            self.0.record("middleware after");
            response.header("x-resource-lifecycle", b"completed middleware".to_vec())
        })
    }
}

#[test]
fn test_client_cleanup_waits_for_response_middleware() {
    let events = CleanupEvents(Arc::new(Mutex::new(Vec::new())));
    let client = TestClient::new(
        App::builder()
            .state(events.clone())
            .middleware(CleanupMiddleware(events.clone()))
            .route_entry(cleanup_route_route())
            .build(),
    );
    let response = client.get("/cleanup").send();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.text(), "resource used");
    assert_eq!(
        response.header("x-resource-lifecycle"),
        Some(b"completed middleware".as_slice())
    );
    assert_eq!(
        events.snapshot(),
        [
            "setup inner",
            "setup outer",
            "handler",
            "middleware after",
            "cleanup outer",
            "cleanup inner"
        ]
    );
}

#[derive(Deserialize, JsonSchema)]
struct SearchParams {
    q: String,
    limit: Option<usize>,
}

#[derive(Clone)]
struct ConfiguredName(&'static str);

#[get("/configured-state")]
async fn configured_state(_cx: &Cx, name: State<ConfiguredName>) -> String {
    name.0.0.to_string()
}

struct StateObserver(Arc<Mutex<Vec<String>>>);

impl fastapi_rust::core::Middleware for StateObserver {
    fn before<'a>(
        &'a self,
        ctx: &'a RequestContext,
        req: &'a mut Request,
    ) -> fastapi_rust::core::middleware::BoxFuture<'a, fastapi_rust::core::ControlFlow> {
        Box::pin(async move {
            let name = State::<ConfiguredName>::from_request(ctx, req)
                .await
                .expect("configured middleware state");
            self.0
                .lock()
                .expect("observations")
                .push(name.0.0.to_string());
            fastapi_rust::core::ControlFlow::Continue
        })
    }
}

#[test]
fn configured_state_reaches_middleware_and_handler_without_cross_app_leaks() {
    let observations = Arc::new(Mutex::new(Vec::new()));
    let first = TestClient::new(
        App::builder()
            .state(ConfiguredName("first"))
            .middleware(StateObserver(Arc::clone(&observations)))
            .route_entry(configured_state_route())
            .build(),
    );
    let second = TestClient::new(
        App::builder()
            .state(ConfiguredName("second"))
            .middleware(StateObserver(Arc::clone(&observations)))
            .route_entry(configured_state_route())
            .build(),
    );
    for (client, expected) in [(&first, "first"), (&second, "second"), (&first, "first")] {
        let response = client.get("/configured-state").send();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.text(), expected);
    }
    assert_eq!(
        *observations.lock().expect("observations"),
        ["first", "second", "first"]
    );
}

#[test]
fn missing_configured_state_still_returns_configuration_error() {
    let client = TestClient::new(App::builder().route_entry(configured_state_route()).build());
    let response = client.get("/configured-state").send();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let error: serde_json::Value = response.json().expect("configuration error");
    assert!(
        error["detail"]
            .as_str()
            .expect("error detail")
            .contains("State type not found")
    );
}

#[get("/items/{id}")]
async fn get_item(cx: &Cx, id: Path<i64>) -> Json<Item> {
    // `checkpoint` returns `Result<(), asupersync::Error>`; there is no `From`
    // into `HttpError`, so handlers map it explicitly.
    if cx.checkpoint().is_err() {
        return Json(Item {
            id: id.0,
            name: "cancelled".into(),
            price: 29.99,
        });
    }
    Json(Item {
        id: id.0,
        name: "Widget".into(),
        price: 29.99,
    })
}

#[get("/checked/{id}")]
async fn get_checked(ctx: &RequestContext, id: Path<i64>) -> Result<Json<Item>, HttpError> {
    ctx.checkpoint()?; // CancelledError -> HttpError (499)
    Ok(Json(Item {
        id: id.0,
        name: "Checked".into(),
        price: 29.99,
    }))
}

#[post("/items")]
async fn create_item(_cx: &Cx, item: Json<Item>) -> Result<Json<Item>, HttpError> {
    if item.0.price < 0.0 {
        return Err(HttpError::bad_request().with_detail("price must be non-negative"));
    }
    if item.0.name.is_empty() {
        return Err(HttpError::bad_request());
    }
    Ok(item)
}

#[get("/search")]
async fn search(_cx: &Cx, q: Query<SearchParams>, _auth: Option<BearerToken>) -> Json<Vec<Item>> {
    let items = vec![Item {
        id: 1,
        name: q.0.q,
        price: 29.99,
    }];
    Json(items.into_iter().take(q.0.limit.unwrap_or(10)).collect())
}

fn app() -> App {
    App::builder()
        .title("Macro routes")
        .version("0.0.1")
        .openapi(fastapi_rust::OpenApiConfig::new())
        .route_entry(get_item_route())
        .route_entry(get_checked_route())
        .route_entry(create_item_route())
        .route_entry(search_route())
        .middleware(RequestIdMiddleware::new())
        .middleware(Cors::new().allow_any_origin())
        .build()
}

type ItemAlias = Item;

#[derive(JsonSchema)]
struct MissingItem {
    detail: String,
}

#[post("/batch", response(200, Vec<Item>, "Stored batch"))]
async fn create_batch(_cx: &Cx, items: Json<Vec<Item>>) -> Json<Vec<Item>> {
    items
}

#[post("/optional")]
async fn optional_count(_cx: &Cx, count: Option<Json<u32>>) -> Json<u32> {
    Json(count.map_or(0, |count| count.0))
}

#[post("/nullable-item")]
async fn nullable_item(_cx: &Cx, item: Json<Option<Item>>) -> Json<Option<Item>> {
    item
}

#[post("/counts")]
async fn counts(
    _cx: &Cx,
    counts: Json<std::collections::BTreeMap<String, Option<u32>>>,
) -> Json<std::collections::BTreeMap<String, Option<u32>>> {
    counts
}

#[get(
    "/aliases/{id:int}",
    response(200, ItemAlias, "An aliased item"),
    response(404, self::MissingItem, "Item missing")
)]
async fn aliased_item(_cx: &Cx, id: Path<i64>) -> Result<Json<ItemAlias>, HttpError> {
    if id.0 < 0 {
        let missing = MissingItem {
            detail: "Item missing".into(),
        };
        return Err(HttpError::not_found().with_detail(missing.detail));
    }
    Ok(Json(Item {
        id: id.0,
        name: "Aliased".into(),
        price: 1.0,
    }))
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct FilterParams {
    search_term: String,
    #[serde(rename = "max", default)]
    page_size: usize,
    cursor: Option<String>,
    r#type: Option<String>,
}

struct PageHeader;
impl fastapi_rust::extract::HeaderName for PageHeader {
    const NAME: &'static str = "X-Page";
}

#[get("/protected/bearer")]
async fn protected_bearer(_cx: &Cx, token: BearerToken) -> Json<String> {
    Json(token.token().to_owned())
}

#[test]
fn required_bearer_extraction_has_served_security_metadata() {
    let client = TestClient::new(
        App::builder()
            .openapi(fastapi_rust::OpenApiConfig::new())
            .route_entry(protected_bearer_route())
            .build(),
    );
    let missing = client.get("/protected/bearer").send();
    assert_eq!(missing.status_code(), 401);
    assert_eq!(missing.header_str("www-authenticate"), Some("Bearer"));
    let wrong_scheme = client
        .get("/protected/bearer")
        .header("authorization", "Basic dXNlcjpwYXNz")
        .send();
    assert_eq!(wrong_scheme.status_code(), 401);
    assert_eq!(wrong_scheme.header_str("www-authenticate"), Some("Bearer"));
    let valid = client
        .get("/protected/bearer")
        .header("authorization", "Bearer opaque-token")
        .send();
    assert_eq!(valid.status_code(), 200);
    assert_eq!(
        valid.json::<String>().expect("extracted token"),
        "opaque-token"
    );
    let document = client
        .get("/openapi.json")
        .send()
        .json::<serde_json::Value>()
        .expect("served OpenAPI");
    assert_eq!(
        document["paths"]["/protected/bearer"]["get"]["security"],
        serde_json::json!([{ "BearerToken": [] }]),
    );
    assert_eq!(
        document["components"]["securitySchemes"]["BearerToken"],
        serde_json::json!({ "type": "http", "scheme": "bearer" }),
    );
}

type AuthAlias = fastapi_rust::BearerToken;

#[get("/protected/basic")]
async fn protected_basic(_cx: &Cx, credentials: fastapi_rust::BasicAuth) -> Json<String> {
    Json(credentials.username)
}

#[get("/protected/oauth")]
async fn protected_oauth(_cx: &Cx, token: OAuth2PasswordBearer) -> Json<String> {
    Json(token.token().to_owned())
}

#[get("/protected/alias")]
async fn protected_alias(_cx: &Cx, token: AuthAlias) -> Json<String> {
    Json(token.token().to_owned())
}

#[get("/optional/auth")]
async fn optional_auth(_cx: &Cx, token: Option<AuthAlias>) -> Json<Option<String>> {
    Json(token.map(|token| token.token().to_owned()))
}

#[get("/optional/basic")]
async fn optional_basic(_cx: &Cx, credentials: Option<BasicAuth>) -> Json<Option<String>> {
    Json(credentials.map(|credentials| credentials.username))
}

#[get("/protected/mixed")]
async fn mixed_auth(_cx: &Cx, token: AuthAlias, credentials: Option<BasicAuth>) -> Json<String> {
    Json(format!(
        "{}:{}",
        token.token(),
        credentials.map_or_else(String::new, |credentials| credentials.username),
    ))
}

#[get("/protected/conjunction")]
async fn conjoined_auth(_cx: &Cx, token: AuthAlias, oauth: OAuth2PasswordBearer) -> Json<String> {
    Json(format!("{}:{}", token.token(), oauth.token()))
}

fn auth_app() -> App {
    App::builder()
        .openapi(fastapi_rust::OpenApiConfig::new())
        .route_entry(protected_bearer_route())
        .route_entry(protected_basic_route())
        .route_entry(protected_oauth_route())
        .route_entry(protected_alias_route())
        .route_entry(optional_auth_route())
        .route_entry(optional_basic_route())
        .route_entry(mixed_auth_route())
        .route_entry(conjoined_auth_route())
        .route_entry(get_item_route())
        .build()
}

#[test]
fn builtin_authentication_schemes_match_required_runtime_extraction() {
    let client = TestClient::new(auth_app());
    for (path, header, value, scheme, challenge) in [
        (
            "/protected/basic",
            "Basic dXNlcjpwYXNz",
            "user",
            "BasicAuth",
            "Basic realm=\"api\"",
        ),
        (
            "/protected/oauth",
            "Bearer oauth-token",
            "oauth-token",
            "OAuth2PasswordBearer",
            "Bearer",
        ),
        (
            "/protected/alias",
            "Bearer aliased-token",
            "aliased-token",
            "BearerToken",
            "Bearer",
        ),
    ] {
        let missing = client.get(path).send();
        assert_eq!(missing.status_code(), 401);
        assert_eq!(missing.header_str("www-authenticate"), Some(challenge));
        let valid = client.get(path).header("authorization", header).send();
        assert_eq!(valid.status_code(), 200);
        assert_eq!(
            valid.json::<String>().expect("extracted credentials"),
            value
        );
        let document = client
            .get("/openapi.json")
            .send()
            .json::<serde_json::Value>()
            .expect("served auth metadata");
        assert_eq!(
            document["paths"][path]["get"]["security"],
            serde_json::json!([{ (scheme): [] }]),
        );
    }
    for header in ["Bearer wrong-scheme", "Basic !!!!", "Basic dXNlcg=="] {
        let invalid = client
            .get("/protected/basic")
            .header("authorization", header)
            .send();
        assert_eq!(invalid.status_code(), 401);
        assert_eq!(
            invalid.header_str("www-authenticate"),
            Some("Basic realm=\"api\"")
        );
    }
    for header in ["Basic dXNlcjpwYXNz", "Bearer"] {
        let invalid = client
            .get("/protected/oauth")
            .header("authorization", header)
            .send();
        assert_eq!(invalid.status_code(), 401);
        assert_eq!(invalid.header_str("www-authenticate"), Some("Bearer"));
    }
    let document = client
        .get("/openapi.json")
        .send()
        .json::<serde_json::Value>()
        .expect("served schemes");
    let schemes = document["components"]["securitySchemes"]
        .as_object()
        .expect("scheme map");
    assert_eq!(schemes.len(), 3);
    assert_eq!(
        schemes["BasicAuth"],
        serde_json::json!({"type": "http", "scheme": "basic"})
    );
    assert_eq!(
        schemes["BearerToken"],
        serde_json::json!({"type": "http", "scheme": "bearer"})
    );
    assert_eq!(
        schemes["OAuth2PasswordBearer"],
        serde_json::json!({"type": "oauth2", "flows": {"password": {"tokenUrl": "/token", "scopes": {}}}}),
    );
    assert!(
        document["paths"]["/items/{id}"]["get"]
            .get("security")
            .is_none()
    );
}

#[test]
fn optional_authentication_documents_anonymous_access_without_weakening_required_auth() {
    let client = TestClient::new(auth_app());
    for (path, scheme, valid_header, expected, malformed) in [
        (
            "/optional/auth",
            "BearerToken",
            "Bearer optional-token",
            "optional-token",
            "Basic invalid",
        ),
        (
            "/optional/basic",
            "BasicAuth",
            "Basic dXNlcjpwYXNz",
            "user",
            "Basic !!!!",
        ),
    ] {
        for header in [None, Some(malformed)] {
            let response = match header {
                Some(header) => client.get(path).header("authorization", header).send(),
                None => client.get(path).send(),
            };
            assert_eq!(response.status_code(), 200);
            assert_eq!(
                response
                    .json::<Option<String>>()
                    .expect("optional credentials"),
                None
            );
        }
        let valid = client
            .get(path)
            .header("authorization", valid_header)
            .send();
        assert_eq!(valid.status_code(), 200);
        assert_eq!(
            valid
                .json::<Option<String>>()
                .expect("optional valid credentials"),
            Some(expected.to_owned())
        );
        let document = client
            .get("/openapi.json")
            .send()
            .json::<serde_json::Value>()
            .expect("optional metadata");
        assert_eq!(
            document["paths"][path]["get"]["security"],
            serde_json::json!([{}, {(scheme): []}])
        );
    }
    assert_eq!(client.get("/protected/mixed").send().status_code(), 401);
    assert_eq!(
        client
            .get("/protected/mixed")
            .header("authorization", "Basic dXNlcjpwYXNz")
            .send()
            .status_code(),
        401,
    );
    let valid = client
        .get("/protected/mixed")
        .header("authorization", "Bearer required-token")
        .send();
    assert_eq!(valid.status_code(), 200);
    assert_eq!(
        valid
            .json::<String>()
            .expect("required token with absent optional Basic"),
        "required-token:"
    );
    let document = client
        .get("/openapi.json")
        .send()
        .json::<serde_json::Value>()
        .expect("mixed metadata");
    assert_eq!(
        document["paths"]["/protected/mixed"]["get"]["security"],
        serde_json::json!([{"BearerToken": []}])
    );
}

fn explicit_security_app() -> App {
    use fastapi_rust::openapi::{ApiKeyLocation, OAuthFlows, OAuthPasswordFlow, SecurityScheme};

    let manual = fastapi_rust::fastapi_core::RouteEntry::from_route(
        fastapi_rust::fastapi_router::Route::new(Method::Get, "/manual/auth")
            .security_scheme("ApiKey"),
        |_ctx, request| {
            let response = if request
                .headers()
                .get("x-api-key")
                .is_some_and(|key| !key.is_empty())
            {
                Response::ok()
            } else {
                Response::with_status(StatusCode::UNAUTHORIZED)
            };
            Box::pin(std::future::ready(response))
        },
    )
    .security_scheme(
        "ApiKey",
        SecurityScheme::ApiKey {
            name: "x-api-key".to_owned(),
            location: ApiKeyLocation::Header,
            description: Some("Application-provided API key".to_owned()),
        },
    );
    let oauth = protected_oauth_route().security_scheme(
        "OAuth2PasswordBearer",
        SecurityScheme::OAuth2 {
            flows: OAuthFlows {
                password: OAuthPasswordFlow {
                    token_url: "/sessions/token".to_owned(),
                    refresh_url: Some("/sessions/refresh".to_owned()),
                    scopes: std::collections::HashMap::from([(
                        "read:reports".to_owned(),
                        "Read reports".to_owned(),
                    )]),
                },
            },
            description: Some("Explicit application metadata".to_owned()),
        },
    );
    App::builder()
        .openapi(fastapi_rust::OpenApiConfig::new())
        .route_entry(manual)
        .route_entry(oauth)
        .build()
}

#[test]
fn explicit_manual_security_definitions_are_served() {
    let client = TestClient::new(explicit_security_app());
    assert_eq!(client.get("/manual/auth").send().status_code(), 401);
    assert_eq!(
        client
            .get("/manual/auth")
            .header("x-api-key", "")
            .send()
            .status_code(),
        401
    );
    assert_eq!(
        client
            .get("/manual/auth")
            .header("x-api-key", "application-key")
            .send()
            .status_code(),
        200
    );
    let document = client
        .get("/openapi.json")
        .send()
        .json::<serde_json::Value>()
        .expect("explicit metadata");
    assert_eq!(
        document["paths"]["/manual/auth"]["get"]["security"],
        serde_json::json!([{"ApiKey": []}])
    );
    let schemes = &document["components"]["securitySchemes"];
    assert_eq!(schemes["ApiKey"]["type"], "apiKey");
    assert_eq!(schemes["ApiKey"]["name"], "x-api-key");
    assert_eq!(schemes["ApiKey"]["in"], "header");
}

#[test]
fn explicit_oauth_metadata_preserves_runtime_extraction() {
    let client = TestClient::new(explicit_security_app());
    let valid = client
        .get("/protected/oauth")
        .header("authorization", "Bearer opaque-token")
        .send();
    assert_eq!(valid.status_code(), 200);
    assert_eq!(
        valid.json::<String>().expect("unchanged OAuth extraction"),
        "opaque-token"
    );
    let document = client
        .get("/openapi.json")
        .send()
        .json::<serde_json::Value>()
        .expect("explicit OAuth metadata");
    let schemes = &document["components"]["securitySchemes"];
    assert_eq!(
        schemes["OAuth2PasswordBearer"]["flows"]["password"]["tokenUrl"],
        "/sessions/token"
    );
    assert_eq!(
        schemes["OAuth2PasswordBearer"]["flows"]["password"]["refreshUrl"],
        "/sessions/refresh"
    );
    assert_eq!(
        schemes["OAuth2PasswordBearer"]["flows"]["password"]["scopes"]["read:reports"],
        "Read reports"
    );
    assert_eq!(
        document["paths"]["/protected/oauth"]["get"]["security"],
        serde_json::json!([{"OAuth2PasswordBearer": []}])
    );
}

#[test]
fn required_authentication_extractors_form_a_conjunction() {
    let client = TestClient::new(auth_app());
    assert_eq!(
        client.get("/protected/conjunction").send().status_code(),
        401
    );
    let valid = client
        .get("/protected/conjunction")
        .header("authorization", "Bearer shared-token")
        .send();
    assert_eq!(valid.status_code(), 200);
    assert_eq!(
        valid.json::<String>().expect("both extractors ran"),
        "shared-token:shared-token"
    );
    let document = client
        .get("/openapi.json")
        .send()
        .json::<serde_json::Value>()
        .expect("conjoined metadata");
    assert_eq!(
        document["paths"]["/protected/conjunction"]["get"]["security"],
        serde_json::json!([{"BearerToken": [], "OAuth2PasswordBearer": []}]),
    );
}

struct TraceHeader;
impl fastapi_rust::extract::HeaderName for TraceHeader {
    const NAME: &'static str = "X-Trace";
}

#[get("/filters")]
async fn filters(
    _cx: &Cx,
    query: Query<FilterParams>,
    page: fastapi_rust::extract::NamedHeader<u32, PageHeader>,
    trace: Option<fastapi_rust::extract::NamedHeader<String, TraceHeader>>,
) -> Json<String> {
    Json(format!(
        "{}:{}:{}:{}:{}:{}",
        query.0.search_term,
        query.0.page_size,
        query.0.cursor.unwrap_or_default(),
        page.value,
        trace.map_or_else(String::new, |trace| trace.value),
        query.0.r#type.unwrap_or_default()
    ))
}

#[get("/optional-filters")]
async fn optional_filters(_cx: &Cx, query: Option<Query<FilterParams>>) -> Json<bool> {
    Json(query.is_some())
}

fn schema_app() -> App {
    App::builder()
        .openapi(fastapi_rust::OpenApiConfig::new())
        .route_entry(create_batch_route())
        .route_entry(optional_count_route())
        .route_entry(nullable_item_route())
        .route_entry(counts_route())
        .route_entry(aliased_item_route())
        .route_entry(filters_route())
        .route_entry(optional_filters_route())
        .build()
}

type SmallPathId = std::primitive::u32;

#[get("/small/{id}")]
async fn small_path(_cx: &Cx, id: Path<SmallPathId>) -> Json<u32> {
    Json(id.0)
}

#[post("/small/{id}")]
async fn string_path(_cx: &Cx, id: Path<String>) -> Json<String> {
    Json(id.0)
}

#[get("/pairs/{label}/{id}")]
async fn tuple_path(_cx: &Cx, values: Path<(String, i64)>) -> Json<String> {
    Json(format!("{}:{}", values.0.0, values.0.1))
}

#[derive(Deserialize, JsonSchema)]
struct NamedPath {
    #[serde(rename = "userId", default)]
    user_id: Option<u32>,
    r#type: String,
}

type NamedPathAlias = NamedPath;

#[get("/named/{type}/{userId}")]
async fn named_path(_cx: &Cx, values: Path<NamedPathAlias>) -> Json<String> {
    Json(format!(
        "{}:{}",
        values.0.r#type,
        values.0.user_id.unwrap_or_default()
    ))
}

#[get("/nullable/{type}/{userId}")]
async fn nullable_path(_cx: &Cx, values: Path<Option<NamedPath>>) -> Json<String> {
    Json(values.0.map_or_else(String::new, |values| {
        format!("{}:{}", values.r#type, values.user_id.unwrap_or_default())
    }))
}

#[get("/maybe/{id}")]
async fn optional_path(_cx: &Cx, value: Option<Path<i64>>) -> Json<Option<i64>> {
    Json(value.map(|value| value.0))
}

#[get("/wild/{*id}")]
async fn wildcard_path(_cx: &Cx, value: Path<i64>) -> Json<i64> {
    Json(value.0)
}

#[get("/uuid/{id:uuid}")]
async fn uuid_path(_cx: &Cx, value: Path<String>) -> Json<String> {
    Json(value.0)
}

fn path_schema_app() -> App {
    App::builder()
        .openapi(fastapi_rust::OpenApiConfig::new())
        .route_entry(small_path_route())
        .route_entry(string_path_route())
        .route_entry(tuple_path_route())
        .route_entry(named_path_route())
        .route_entry(nullable_path_route())
        .route_entry(optional_path_route())
        .route_entry(wildcard_path_route())
        .route_entry(uuid_path_route())
        .build()
}

fn assert_path_parameters(spec: &serde_json::Value, path: &str, expected: &[(&str, &str, &str)]) {
    let parameters = spec["paths"][path]["get"]["parameters"]
        .as_array()
        .expect("path parameters");
    assert_eq!(parameters.len(), expected.len(), "{path}");
    for (parameter, (name, kind, format)) in parameters.iter().zip(expected) {
        assert_eq!(parameter["name"], *name, "{path}");
        assert_eq!(parameter["in"], "path");
        assert_eq!(parameter["required"], true);
        assert_eq!(parameter["schema"]["type"], *kind, "{path}");
        assert_eq!(
            parameter["schema"]["format"].as_str().unwrap_or_default(),
            *format,
            "{path}"
        );
        assert!(parameter["schema"]["anyOf"].is_null());
    }
}

#[test]
fn served_path_schemas_follow_scalar_alias_tuple_order_and_named_serde_fields() {
    let client = TestClient::new(path_schema_app());
    let spec: serde_json::Value = client
        .get("/openapi.json")
        .send()
        .json()
        .expect("served OpenAPI");
    assert_path_parameters(&spec, "/small/{id}", &[("id", "integer", "uint32")]);
    assert_path_parameters(
        &spec,
        "/pairs/{label}/{id}",
        &[("label", "string", ""), ("id", "integer", "int64")],
    );
    assert_path_parameters(
        &spec,
        "/named/{type}/{userId}",
        &[("type", "string", ""), ("userId", "integer", "uint32")],
    );
    assert_eq!(
        spec["paths"]["/small/{id}"]["post"]["parameters"][0]["schema"]["type"],
        "string"
    );
    for id in [0, u32::MAX] {
        let response = client.get(&format!("/small/{id}")).send();
        assert_eq!(response.status().as_u16(), 200);
        assert_eq!(response.json::<u32>().expect("scalar alias"), id);
    }
    assert_eq!(client.get("/small/-1").send().status().as_u16(), 422);
    assert_eq!(
        client.get("/small/4294967296").send().status().as_u16(),
        422
    );
    assert_eq!(
        client
            .post("/small/raw-text")
            .send()
            .json::<String>()
            .expect("string path"),
        "raw-text"
    );
    assert_eq!(
        client
            .get("/pairs/chapter/-7")
            .send()
            .json::<String>()
            .expect("tuple path"),
        "chapter:-7"
    );
    let invalid_tuple = client.get("/pairs/chapter/not-an-integer").send();
    assert_eq!(invalid_tuple.status().as_u16(), 422);
    assert_eq!(
        invalid_tuple
            .json::<serde_json::Value>()
            .expect("tuple error")["detail"][0]["loc"],
        serde_json::json!(["path", "id"])
    );
    assert_eq!(
        client
            .get("/named/book/19")
            .send()
            .json::<String>()
            .expect("named path"),
        "book:19"
    );
    let invalid_named = client.get("/named/book/not-an-integer").send();
    assert_eq!(invalid_named.status().as_u16(), 422);
    assert_eq!(
        invalid_named
            .json::<serde_json::Value>()
            .expect("named error")["detail"][0]["loc"],
        serde_json::json!(["path", "userId"])
    );
}

#[test]
fn path_schemas_keep_optional_values_required_and_normalize_wildcard_names() {
    let client = TestClient::new(path_schema_app());
    let spec: serde_json::Value = client
        .get("/openapi.json")
        .send()
        .json()
        .expect("served OpenAPI");
    for path in ["/maybe/{id}", "/wild/{id}"] {
        let parameters = spec["paths"][path]["get"]["parameters"]
            .as_array()
            .expect("one typed path parameter");
        assert_eq!(parameters.len(), 1);
        assert_eq!(parameters[0]["name"], "id");
        assert_eq!(parameters[0]["in"], "path");
        assert_eq!(parameters[0]["required"], true);
        assert_eq!(parameters[0]["schema"]["type"], "integer");
        assert_eq!(parameters[0]["schema"]["format"], "int64");
    }
    assert!(spec["paths"]["/wild/{*id}"].is_null());
    let named = &spec["paths"]["/nullable/{type}/{userId}"]["get"]["parameters"];
    assert_eq!(
        named.as_array().expect("nullable named parameters").len(),
        2
    );
    assert_eq!(named[0]["schema"]["type"], "string");
    assert_eq!(named[1]["schema"]["type"], "integer");
    assert_eq!(named[1]["schema"]["format"], "uint32");
    assert_eq!(named[0]["required"], true);
    assert_eq!(named[1]["required"], true);
    assert!(named[0]["schema"]["anyOf"].is_null());
    assert!(named[1]["schema"]["anyOf"].is_null());
    assert_eq!(
        client
            .get("/nullable/book/23")
            .send()
            .json::<String>()
            .expect("nullable named path"),
        "book:23"
    );
    assert_eq!(
        client.get("/nullable/book/bad").send().status().as_u16(),
        422
    );
    assert_eq!(
        client
            .get("/wild/7")
            .send()
            .json::<i64>()
            .expect("wildcard id"),
        7
    );
    assert_eq!(client.get("/wild/a/b").send().status().as_u16(), 422);
    assert_eq!(
        client
            .get("/maybe/42")
            .send()
            .json::<Option<i64>>()
            .expect("optional id"),
        Some(42)
    );
    let malformed_optional = client.get("/maybe/bad").send();
    assert_eq!(malformed_optional.status().as_u16(), 200);
    assert_eq!(
        malformed_optional
            .json::<Option<i64>>()
            .expect("optional extraction policy"),
        None
    );
}

#[test]
fn typed_path_metadata_preserves_manual_examples_and_explicit_converter_guards() {
    let mut route = fastapi_rust::router::Route::new(Method::Get, "/manual/{id}");
    route.path_params[0] = route.path_params[0]
        .clone()
        .with_title("Record identifier")
        .with_description("Identifier supplied by the caller")
        .deprecated()
        .with_named_example("small", serde_json::json!(1));
    let manual = fastapi_rust::fastapi_core::RouteEntry::from_route(route, |_ctx, _req| {
        Box::pin(std::future::ready(Response::ok()))
    })
    .path_schema::<i32>(&["id"]);
    let client = TestClient::new(
        App::builder()
            .openapi(fastapi_rust::OpenApiConfig::new())
            .route_entry(manual)
            .route_entry(uuid_path_route())
            .route_entry(aliased_item_route())
            .get("/untyped/{id:int}", |_ctx, _req| async { Response::ok() })
            .build(),
    );
    let spec: serde_json::Value = client
        .get("/openapi.json")
        .send()
        .json()
        .expect("served OpenAPI");
    let parameters = spec["paths"]["/manual/{id}"]["get"]["parameters"]
        .as_array()
        .expect("one manually described path parameter");
    assert_eq!(parameters.len(), 1);
    let parameter = &parameters[0];
    assert_eq!(parameter["name"], "id");
    assert_eq!(parameter["required"], true);
    assert_eq!(parameter["schema"]["type"], "integer");
    assert_eq!(parameter["schema"]["format"], "int32");
    assert_eq!(parameter["title"], "Record identifier");
    assert_eq!(
        parameter["description"],
        "Identifier supplied by the caller"
    );
    assert_eq!(parameter["deprecated"], true);
    assert!(parameter["example"].is_null());
    assert_eq!(parameter["examples"]["small"]["value"], 1);
    for path in ["/aliases/{id}", "/untyped/{id}"] {
        let schema = &spec["paths"][path]["get"]["parameters"][0]["schema"];
        assert_eq!(schema["type"], "integer");
        assert_eq!(schema["format"], "int64");
    }
    let uuid = "123e4567-e89b-12d3-a456-426614174000";
    let uuid_schema = &spec["paths"]["/uuid/{id}"]["get"]["parameters"][0]["schema"];
    assert_eq!(uuid_schema["type"], "string");
    assert_eq!(uuid_schema["format"], "uuid");
    assert_eq!(
        client
            .get(&format!("/uuid/{uuid}"))
            .send()
            .json::<String>()
            .expect("validated UUID"),
        uuid
    );
    assert_eq!(client.get("/uuid/bad").send().status().as_u16(), 404);
    assert_eq!(
        client
            .get("/aliases/not-an-integer")
            .send()
            .status()
            .as_u16(),
        404
    );
    assert_eq!(client.get("/untyped/bad").send().status().as_u16(), 404);
}

#[test]
fn served_openapi_nullable_models_and_map_values_match_real_json_requests() {
    let client = TestClient::new(schema_app());
    let spec: serde_json::Value = client
        .get("/openapi.json")
        .send()
        .json()
        .expect("served OpenAPI");
    let operation = &spec["paths"]["/nullable-item"]["post"];
    assert_eq!(operation["requestBody"]["required"], true);
    let body_schema = &operation["requestBody"]["content"]["application/json"]["schema"];
    assert_eq!(body_schema["anyOf"][0]["type"], "object");
    assert_eq!(body_schema["anyOf"][1]["type"], "null");
    assert_eq!(
        operation["responses"]["200"]["content"]["application/json"]["schema"],
        *body_schema
    );
    let absent: Option<Item> = None;
    let null_response = client.post("/nullable-item").json(&absent).send();
    assert_eq!(null_response.status().as_u16(), 200);
    assert_eq!(
        null_response
            .json::<Option<Item>>()
            .expect("null JSON response"),
        None
    );
    let present = Some(Item {
        id: 8,
        name: "Nullable".into(),
        price: 2.0,
    });
    assert_eq!(
        client
            .post("/nullable-item")
            .json(&present)
            .send()
            .json::<Option<Item>>()
            .expect("model JSON response"),
        present
    );
    let map_schema =
        &spec["paths"]["/counts"]["post"]["requestBody"]["content"]["application/json"]["schema"];
    assert_eq!(map_schema["type"], "object");
    assert_eq!(
        map_schema["additionalProperties"]["anyOf"][0]["type"],
        "integer"
    );
    assert_eq!(
        map_schema["additionalProperties"]["anyOf"][1]["type"],
        "null"
    );
    assert!(map_schema["additional_properties"].is_null());
    let counts = std::collections::BTreeMap::from([
        ("present".to_string(), Some(9_u32)),
        ("absent".to_string(), None),
    ]);
    assert_eq!(
        client
            .post("/counts")
            .json(&counts)
            .send()
            .json::<std::collections::BTreeMap<String, Option<u32>>>()
            .expect("nullable map values"),
        counts
    );
    let decoded: fastapi_rust::OpenApi =
        serde_json::from_value(spec.clone()).expect("nullable document decode");
    assert_eq!(
        serde_json::to_value(decoded).expect("nullable document encode"),
        spec
    );
}

fn assert_schema_references_resolve(spec: &serde_json::Value, value: &serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            if let Some(reference) = object.get("$ref") {
                let reference = reference.as_str().expect("reference string");
                let pointer = reference
                    .strip_prefix('#')
                    .expect("local component reference");
                assert!(
                    spec.pointer(pointer).is_some(),
                    "unresolved schema: {reference}"
                );
            }
            for child in object.values() {
                assert_schema_references_resolve(spec, child);
            }
        }
        serde_json::Value::Array(array) => {
            for child in array {
                assert_schema_references_resolve(spec, child);
            }
        }
        _ => {}
    }
}

#[test]
fn served_openapi_path_type_matches_integer_handler_without_converter() {
    let client = TestClient::new(app());
    let valid = client.get("/items/42").send();
    assert_eq!(valid.status().as_u16(), 200);
    assert_eq!(valid.json::<Item>().expect("typed item").id, 42);
    assert_eq!(
        client.get("/items/not-an-integer").send().status().as_u16(),
        422
    );

    let spec: serde_json::Value = client
        .get("/openapi.json")
        .send()
        .json()
        .expect("served OpenAPI");
    let parameters = spec["paths"]["/items/{id}"]["get"]["parameters"]
        .as_array()
        .expect("path parameters");
    assert_eq!(parameters.len(), 1);
    assert_eq!(parameters[0]["name"], "id");
    assert_eq!(parameters[0]["in"], "path");
    assert_eq!(parameters[0]["required"], true);
    assert_eq!(parameters[0]["schema"]["type"], "integer");
    assert_eq!(parameters[0]["schema"]["format"], "int64");
}

#[test]
fn served_openapi_registers_models_and_infers_json_success_responses() {
    let client = TestClient::new(app());
    let response = client.get("/openapi.json").send();
    assert_eq!(response.status().as_u16(), 200);
    let spec: serde_json::Value = response.json().expect("served OpenAPI");
    assert_schema_references_resolve(&spec, &spec);
    assert_eq!(
        spec["components"]["schemas"]["Item"]["properties"]["price"]["type"],
        "number"
    );
    assert_eq!(spec["components"]["schemas"]["Item"]["type"], "object");
    let decoded: fastapi_rust::openapi::OpenApi =
        serde_json::from_value(spec.clone()).expect("document round trip");
    let round_trip = serde_json::to_value(decoded).expect("document serialization");
    assert_eq!(round_trip, spec);
    assert_eq!(
        spec["paths"]["/items"]["post"]["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/Item"
    );
    assert_eq!(
        spec["paths"]["/items"]["post"]["requestBody"]["required"],
        true
    );
    for path in ["/items/{id}", "/checked/{id}"] {
        assert_eq!(
            spec["paths"][path]["get"]["responses"]["200"]["content"]["application/json"]["schema"]
                ["$ref"],
            "#/components/schemas/Item"
        );
    }
    assert_eq!(
        spec["paths"]["/search"]["get"]["responses"]["200"]["content"]["application/json"]["schema"]
            ["items"]["properties"]["name"]["type"],
        "string"
    );
}

#[test]
fn served_openapi_uses_inline_containers_primitives_and_canonical_alias_names() {
    let client = TestClient::new(schema_app());
    let spec: serde_json::Value = client
        .get("/openapi.json")
        .send()
        .json()
        .expect("served OpenAPI");
    assert_schema_references_resolve(&spec, &spec);
    let batch = &spec["paths"]["/batch"]["post"];
    assert_eq!(
        batch["requestBody"]["content"]["application/json"]["schema"]["type"],
        "array"
    );
    assert!(batch["requestBody"]["content"]["application/json"]["schema"]["$ref"].is_null());
    assert_eq!(
        batch["requestBody"]["content"]["application/json"]["schema"]["items"]["properties"]["id"]
            ["type"],
        "integer"
    );
    assert_eq!(batch["responses"]["200"]["description"], "Stored batch");
    assert!(
        batch["responses"]["200"]["content"]["application/json"]["schema"]["items"].is_object()
    );
    let optional = &spec["paths"]["/optional"]["post"];
    assert_eq!(optional["requestBody"]["required"], false);
    assert_eq!(
        optional["requestBody"]["content"]["application/json"]["schema"]["type"],
        "integer"
    );
    assert_eq!(
        optional["responses"]["200"]["content"]["application/json"]["schema"]["type"],
        "integer"
    );
    assert!(spec["paths"]["/aliases/{id:int}"].is_null());
    let alias = &spec["paths"]["/aliases/{id}"]["get"];
    assert_eq!(alias["parameters"][0]["name"], "id");
    assert_eq!(alias["parameters"][0]["schema"]["type"], "integer");
    assert_eq!(alias["responses"]["200"]["description"], "An aliased item");
    assert_eq!(
        alias["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/Item"
    );
    assert!(spec["components"]["schemas"]["Item"].is_object());
    assert!(spec["components"]["schemas"]["ItemAlias"].is_null());
    assert_eq!(alias["responses"]["404"]["description"], "Item missing");
    assert_eq!(
        alias["responses"]["404"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/MissingItem"
    );
    assert!(spec["components"]["schemas"]["MissingItem"].is_object());
    assert_eq!(
        client
            .get("/aliases/23")
            .send()
            .json::<Item>()
            .expect("aliased runtime")
            .id,
        23
    );
    assert_eq!(client.get("/aliases/-1").send().status().as_u16(), 404);
    assert_eq!(
        client
            .post("/optional")
            .send()
            .json::<u32>()
            .expect("absent body"),
        0
    );
    assert_eq!(
        client
            .post("/optional")
            .json(&17_u32)
            .send()
            .json::<u32>()
            .expect("primitive body"),
        17
    );
}

#[test]
fn served_openapi_accepts_typed_metadata_on_manual_route_entries() {
    let entry =
        fastapi_rust::fastapi_core::RouteEntry::new(Method::Post, "/manual/{id:int}", |_, _| {
            std::future::ready(Response::json(&7_u32).expect("manual JSON response"))
        })
        .request_schema::<u32>(true)
        .response_schema::<u32>(200, "Manual JSON response");
    let client = TestClient::new(
        App::builder()
            .openapi(fastapi_rust::OpenApiConfig::new())
            .route_entry(entry)
            .get("/wild/{*rest}", |_, _| {
                std::future::ready(Response::json(&"wildcard").expect("wildcard JSON response"))
            })
            .build(),
    );
    let spec: serde_json::Value = client
        .get("/openapi.json")
        .send()
        .json()
        .expect("served OpenAPI");
    assert_schema_references_resolve(&spec, &spec);
    assert!(spec["paths"]["/wild/{*rest}"].is_null());
    assert_eq!(
        spec["paths"]["/wild/{rest}"]["get"]["parameters"][0]["name"],
        "rest"
    );
    assert_eq!(
        client
            .get("/wild/a/b")
            .send()
            .json::<String>()
            .expect("wildcard response"),
        "wildcard"
    );
    let operation = &spec["paths"]["/manual/{id}"]["post"];
    assert!(spec["paths"]["/manual/{id:int}"].is_null());
    assert_eq!(operation["parameters"][0]["name"], "id");
    assert_eq!(operation["parameters"][0]["schema"]["type"], "integer");
    assert_eq!(operation["requestBody"]["required"], true);
    assert_eq!(
        operation["requestBody"]["content"]["application/json"]["schema"]["type"],
        "integer"
    );
    assert_eq!(
        operation["responses"]["200"]["description"],
        "Manual JSON response"
    );
    assert_eq!(
        operation["responses"]["200"]["content"]["application/json"]["schema"]["type"],
        "integer"
    );
    assert_eq!(
        client
            .post("/manual/3")
            .json(&3_u32)
            .send()
            .json::<u32>()
            .expect("manual response"),
        7
    );
    assert_eq!(
        client.post("/manual/not-a-number").send().status().as_u16(),
        404
    );
}

#[test]
fn served_openapi_query_and_header_parameters_match_runtime_extraction() {
    let client = TestClient::new(schema_app());
    let spec: serde_json::Value = client
        .get("/openapi.json")
        .send()
        .json()
        .expect("served OpenAPI");
    let parameters = spec["paths"]["/filters"]["get"]["parameters"]
        .as_array()
        .expect("parameter list");
    assert_eq!(parameters.len(), 6);
    assert!(
        parameters
            .iter()
            .all(|parameter| parameter["name"] != "r#type")
    );
    for (name, location, required, kind) in [
        ("searchTerm", "query", true, "string"),
        ("max", "query", false, "integer"),
        ("cursor", "query", false, "string"),
        ("type", "query", false, "string"),
        ("X-Page", "header", true, "integer"),
        ("X-Trace", "header", false, "string"),
    ] {
        let parameter = parameters
            .iter()
            .find(|parameter| parameter["name"] == name)
            .expect("named parameter");
        assert_eq!(parameter["in"], location);
        assert_eq!(parameter["required"], required);
        assert_eq!(parameter["schema"]["type"], kind);
    }
    let optional = spec["paths"]["/optional-filters"]["get"]["parameters"]
        .as_array()
        .expect("optional parameters");
    assert!(
        optional
            .iter()
            .all(|parameter| parameter["required"] == false)
    );
    let valid = client
        .get("/filters?searchTerm=rust&max=4&cursor=next&type=book")
        .header("X-Page", "2")
        .header("X-Trace", "trace")
        .send();
    assert_eq!(valid.status().as_u16(), 200);
    assert_eq!(
        valid.json::<String>().expect("extracted values"),
        "rust:4:next:2:trace:book"
    );
    let defaults = client
        .get("/filters?searchTerm=rust")
        .header("X-Page", "2")
        .send();
    assert_eq!(
        defaults.json::<String>().expect("optional values"),
        "rust:0::2::"
    );
    assert_eq!(
        client
            .get("/filters?searchTerm=rust")
            .send()
            .status()
            .as_u16(),
        422
    );
    assert_eq!(
        client
            .get("/filters?searchTerm=rust")
            .header("X-Page", "invalid")
            .send()
            .status()
            .as_u16(),
        422
    );
    assert_eq!(
        client
            .get("/filters")
            .header("X-Page", "2")
            .send()
            .status()
            .as_u16(),
        422
    );
    assert!(
        !client
            .get("/optional-filters")
            .send()
            .json::<bool>()
            .expect("optional query")
    );
}

#[test]
fn get_route_with_cx_and_path_returns_json() {
    let client = TestClient::new(app());
    let response = client.get("/items/42").send();
    assert_eq!(response.status().as_u16(), 200);
    let item: Item = response.json().expect("JSON body");
    assert_eq!(
        item,
        Item {
            id: 42,
            name: "Widget".into(),
            price: 29.99,
        }
    );
}

#[test]
fn request_context_checkpoint_question_mark_compiles_and_passes() {
    let client = TestClient::new(app());
    let response = client.get("/checked/7").send();
    assert_eq!(response.status().as_u16(), 200);
    let item: Item = response.json().expect("JSON body");
    assert_eq!(item.name, "Checked");
}

#[test]
fn path_extractor_type_mismatch_is_422() {
    let client = TestClient::new(app());
    let response = client.get("/items/not-a-number").send();
    assert_eq!(response.status().as_u16(), 422);
}

#[test]
fn post_route_round_trips_json_body() {
    let client = TestClient::new(app());
    let response = client
        .post("/items")
        .json(&Item {
            id: 1,
            name: "Gadget".into(),
            price: 5.0,
        })
        .send();
    assert_eq!(response.status().as_u16(), 200);
    let item: Item = response.json().expect("JSON body");
    assert_eq!(item.name, "Gadget");
}

#[test]
fn post_route_propagates_http_error() {
    let client = TestClient::new(app());
    let response = client
        .post("/items")
        .json(&Item {
            id: 1,
            name: String::new(),
            price: 0.0,
        })
        .send();
    assert_eq!(response.status().as_u16(), 400);
}

#[test]
fn builder_metadata_lands_in_app_config() {
    let app = app();
    assert_eq!(app.config().name, "Macro routes");
    assert_eq!(app.config().version, "0.0.1");
}

#[test]
fn search_extracts_query_and_accepts_missing_optional_auth() {
    let client = TestClient::new(app());
    let response = client.get("/search?q=Blue%20widget").send();
    assert_eq!(response.status().as_u16(), 200);
    let items: Vec<Item> = response.json().expect("JSON items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "Blue widget");

    let response = client
        .get("/search?q=Widget&limit=0")
        .header("authorization", "Bearer test-token")
        .send();
    assert_eq!(response.status().as_u16(), 200);
    assert!(response.json::<Vec<Item>>().expect("JSON items").is_empty());
}

#[test]
fn search_rejects_missing_or_malformed_required_query_values() {
    let client = TestClient::new(app());
    for path in ["/search", "/search?q=Widget&limit=invalid"] {
        let response = client.get(path).send();
        assert_eq!(response.status().as_u16(), 422);
    }
}

#[test]
fn post_route_rejects_negative_price_with_handler_detail() {
    let client = TestClient::new(app());
    let response = client
        .post("/items")
        .json(&Item {
            id: 1,
            name: "Widget".into(),
            price: -1.0,
        })
        .send();
    assert_eq!(response.status().as_u16(), 400);
    let error: serde_json::Value = response.json().expect("JSON error");
    assert_eq!(error["detail"], "price must be non-negative");
}
