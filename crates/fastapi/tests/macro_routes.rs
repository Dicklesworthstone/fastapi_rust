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

use fastapi_rust::prelude::*;
use fastapi_rust::testing::TestClient;

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
struct Item {
    id: i64,
    name: String,
    price: f64,
}

#[derive(Deserialize, JsonSchema)]
struct SearchParams {
    q: String,
    limit: Option<usize>,
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
