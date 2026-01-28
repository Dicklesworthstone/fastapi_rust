//! Integration tests for enum schema generation with the JsonSchema derive macro.

// Enum variants are used by the JsonSchema derive macro, not directly in Rust code
#![allow(dead_code)]

use fastapi_macros::JsonSchema;
use fastapi_openapi::JsonSchema as JsonSchemaTrait;

// Test 1: Unit-only enum (should generate string enum)
#[derive(JsonSchema)]
enum Color {
    Red,
    Green,
    Blue,
}

#[test]
fn test_unit_enum_generates_string_enum() {
    let schema = Color::schema();
    let json = serde_json::to_string(&schema).unwrap();
    // Unit-only enums should generate: {"type":"string","enum":["Red","Green","Blue"]}
    assert!(
        json.contains(r#""type":"string""#),
        "Should be string type: {json}"
    );
    assert!(
        json.contains(r#""enum""#),
        "Should have enum values: {json}"
    );
    assert!(json.contains(r#""Red""#), "Should have Red: {json}");
    assert!(json.contains(r#""Green""#), "Should have Green: {json}");
    assert!(json.contains(r#""Blue""#), "Should have Blue: {json}");
}

#[test]
fn test_unit_enum_schema_name() {
    assert_eq!(Color::schema_name(), Some("Color"));
}

// Test 2: Tuple variant enum (should generate oneOf)
#[derive(JsonSchema)]
enum Value {
    Int(i32),
    Text(String),
}

#[test]
fn test_tuple_enum_generates_one_of() {
    let schema = Value::schema();
    let json = serde_json::to_string(&schema).unwrap();
    // Tuple variants should generate oneOf with {"Int": <schema>}, {"Text": <schema>}
    assert!(json.contains(r#""oneOf""#), "Should have oneOf: {json}");
    assert!(json.contains(r#""Int""#), "Should have Int variant: {json}");
    assert!(
        json.contains(r#""Text""#),
        "Should have Text variant: {json}"
    );
}

// Test 3: Struct variant enum (should generate oneOf)
#[derive(JsonSchema)]
enum Message {
    Text { body: String },
    Image { url: String, width: u32 },
}

#[test]
fn test_struct_enum_generates_one_of() {
    let schema = Message::schema();
    let json = serde_json::to_string(&schema).unwrap();
    // Struct variants should generate oneOf with nested objects
    assert!(json.contains(r#""oneOf""#), "Should have oneOf: {json}");
    assert!(
        json.contains(r#""Text""#),
        "Should have Text variant: {json}"
    );
    assert!(
        json.contains(r#""Image""#),
        "Should have Image variant: {json}"
    );
    assert!(json.contains(r#""body""#), "Should have body field: {json}");
    assert!(json.contains(r#""url""#), "Should have url field: {json}");
    assert!(
        json.contains(r#""width""#),
        "Should have width field: {json}"
    );
}

// Test 4: Mixed variant enum (unit + tuple + struct)
#[derive(JsonSchema)]
enum Event {
    Empty,
    Count(u32),
    Data { value: String },
}

#[test]
fn test_mixed_enum_generates_one_of() {
    let schema = Event::schema();
    let json = serde_json::to_string(&schema).unwrap();
    // Mixed variants should generate oneOf
    assert!(json.contains(r#""oneOf""#), "Should have oneOf: {json}");
    // Should have all three variants
    assert!(
        json.contains(r#""Empty""#),
        "Should have Empty variant: {json}"
    );
    assert!(
        json.contains(r#""Count""#),
        "Should have Count variant: {json}"
    );
    assert!(
        json.contains(r#""Data""#),
        "Should have Data variant: {json}"
    );
}

// Test 5: Enum with optional fields in struct variants
#[derive(JsonSchema)]
enum Request {
    Get {
        path: String,
        headers: Option<String>,
    },
    Post {
        path: String,
        body: String,
    },
}

#[test]
fn test_enum_with_optional_fields() {
    let schema = Request::schema();
    let json = serde_json::to_string(&schema).unwrap();
    assert!(json.contains(r#""oneOf""#), "Should have oneOf: {json}");
    assert!(json.contains(r#""Get""#), "Should have Get variant: {json}");
    assert!(
        json.contains(r#""Post""#),
        "Should have Post variant: {json}"
    );
    // Both should have path field
    assert!(json.contains(r#""path""#), "Should have path field: {json}");
}

// Test 6: Single-variant enum
#[derive(JsonSchema)]
enum SingleUnit {
    Only,
}

#[derive(JsonSchema)]
enum SingleTuple {
    Only(String),
}

#[test]
fn test_single_variant_enums() {
    // Single unit variant
    let schema = SingleUnit::schema();
    let json = serde_json::to_string(&schema).unwrap();
    assert!(
        json.contains(r#""Only""#),
        "Should have Only variant: {json}"
    );

    // Single tuple variant
    let schema = SingleTuple::schema();
    let json = serde_json::to_string(&schema).unwrap();
    assert!(
        json.contains(r#""Only""#),
        "Should have Only variant: {json}"
    );
}

// Test 7: Enum with nested types
#[derive(JsonSchema)]
struct Inner {
    value: i32,
}

#[derive(JsonSchema)]
enum Wrapper {
    Simple(String),
    Complex(Inner),
}

#[test]
fn test_enum_with_nested_types() {
    let schema = Wrapper::schema();
    let json = serde_json::to_string(&schema).unwrap();
    assert!(json.contains(r#""oneOf""#), "Should have oneOf: {json}");
    assert!(
        json.contains(r#""Simple""#),
        "Should have Simple variant: {json}"
    );
    assert!(
        json.contains(r#""Complex""#),
        "Should have Complex variant: {json}"
    );
}
