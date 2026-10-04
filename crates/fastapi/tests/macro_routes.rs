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

#[derive(Deserialize)]
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
        .route_entry(get_item_route())
        .route_entry(get_checked_route())
        .route_entry(create_item_route())
        .route_entry(search_route())
        .middleware(RequestIdMiddleware::new())
        .middleware(Cors::new().allow_any_origin())
        .build()
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
