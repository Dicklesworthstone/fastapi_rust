//! Integration tests for enum-like schema generation (manual construction).
//!
//! Note: `#[derive(JsonSchema)]` lives in `fastapi-macros`, which depends on this crate.
//! Keeping macro-derive tests here creates a dependency cycle. These tests validate
//! the underlying schema types and serialization behavior instead.

use fastapi_openapi::{EnumSchema, OneOfSchema, Schema, SchemaType};

#[test]
fn enum_schema_serializes_as_string_enum() {
    let schema = Schema::Enum(EnumSchema {
        schema_type: SchemaType::String,
        enum_values: vec!["Red".to_string(), "Green".to_string(), "Blue".to_string()],
    });

    let json = serde_json::to_string(&schema).unwrap();
    assert!(json.contains(r#""type":"string""#), "{json}");
    assert!(json.contains(r#""enum""#), "{json}");
    assert!(json.contains(r#""Red""#), "{json}");
    assert!(json.contains(r#""Green""#), "{json}");
    assert!(json.contains(r#""Blue""#), "{json}");
}

#[test]
fn oneof_schema_serializes() {
    let schema = Schema::OneOf(OneOfSchema {
        one_of: vec![Schema::reference("Int"), Schema::reference("Text")],
    });

    let json = serde_json::to_string(&schema).unwrap();
    assert!(json.contains(r#""oneOf""#), "{json}");
    assert!(json.contains(r"#/components/schemas/Int"), "{json}");
    assert!(json.contains(r"#/components/schemas/Text"), "{json}");
}
