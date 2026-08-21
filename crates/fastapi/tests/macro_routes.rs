//! End-to-end coverage of the proc-macro route path as a *consumer* sees it:
//! `use fastapi_rust::prelude::*`, `#[get]`/`#[post]` handlers taking `&Cx`
//! and extractors, returning `Json<T>`, registered via the generated
//! `<name>_route()` entries, and exercised through `TestClient`.
//!
//! This is the README's headline example. It guards three things that were
//! broken together before 0.4.3: macro expansions resolving `fastapi_core::`
//! paths through the facade, `Json<T>` as a response type, and handler
//! futures being allowed to borrow the request for the call's duration.

// The handlers below intentionally mirror the README signatures (`async fn` that may
// not await, `Result<_, HttpError>`), so the pedantic lints about them are noise here.
#![allow(clippy::unused_async, clippy::result_large_err)]

use fastapi_rust::prelude::*;
use fastapi_rust::testing::TestClient;

#[derive(Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
struct Item {
    id: i64,
    name: String,
}

#[get("/items/{id}")]
async fn get_item(cx: &Cx, id: Path<i64>) -> Json<Item> {
    // `checkpoint` returns `Result<(), asupersync::Error>`; there is no `From`
    // into `HttpError`, so handlers map it explicitly.
    if cx.checkpoint().is_err() {
        return Json(Item {
            id: id.0,
            name: "cancelled".into(),
        });
    }
    Json(Item {
        id: id.0,
        name: "Widget".into(),
    })
}

#[get("/checked/{id}")]
async fn get_checked(ctx: &RequestContext, id: Path<i64>) -> Result<Json<Item>, HttpError> {
    ctx.checkpoint()?; // CancelledError -> HttpError (499)
    Ok(Json(Item {
        id: id.0,
        name: "Checked".into(),
    }))
}

#[post("/items")]
async fn create_item(_cx: &Cx, item: Json<Item>) -> Result<Json<Item>, HttpError> {
    if item.0.name.is_empty() {
        return Err(HttpError::bad_request());
    }
    Ok(item)
}

fn app() -> App {
    App::builder()
        .title("Macro routes")
        .version("0.0.1")
        .route_entry(get_item_route())
        .route_entry(get_checked_route())
        .route_entry(create_item_route())
        .middleware(RequestIdMiddleware::new())
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
            name: "Widget".into()
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
