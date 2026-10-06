//! JSON Schema types for OpenAPI 3.1.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// JSON Schema representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Schema {
    /// Boolean schema (true = any, false = none).
    Boolean(bool),
    /// Reference to another schema.
    Ref(RefSchema),
    /// Enum schema (string values).
    Enum(EnumSchema),
    /// OneOf schema (union type).
    OneOf(OneOfSchema),
    /// AnyOf schema (a union whose branches may overlap).
    AnyOf(AnyOfSchema),
    /// Array schema.
    #[serde(serialize_with = "serialize_array_schema")]
    Array(ArraySchema),
    /// Primitive type schema.
    #[serde(serialize_with = "serialize_primitive_schema")]
    Primitive(PrimitiveSchema),
    /// Object schema. Keep the all-optional object variant last when decoding.
    #[serde(serialize_with = "serialize_object_schema")]
    Object(ObjectSchema),
}

#[derive(Serialize)]
struct TypedSchema<'a, T> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(flatten)]
    schema: &'a T,
}

fn serialize_primitive_schema<S: serde::Serializer>(
    schema: &PrimitiveSchema,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if schema.nullable {
        let mut non_null = schema.clone();
        non_null.nullable = false;
        Schema::any_of(vec![Schema::Primitive(non_null), <()>::schema()]).serialize(serializer)
    } else {
        schema.serialize(serializer)
    }
}

fn serialize_object_schema<S: serde::Serializer>(
    schema: &ObjectSchema,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    TypedSchema {
        kind: "object",
        schema,
    }
    .serialize(serializer)
}

fn serialize_array_schema<S: serde::Serializer>(
    schema: &ArraySchema,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    TypedSchema {
        kind: "array",
        schema,
    }
    .serialize(serializer)
}

impl Schema {
    /// Create a string schema.
    pub fn string() -> Self {
        Schema::Primitive(PrimitiveSchema::string())
    }

    /// Create an integer schema with optional format.
    pub fn integer(format: Option<&str>) -> Self {
        Schema::Primitive(PrimitiveSchema::integer(format))
    }

    /// Create a number schema with optional format.
    pub fn number(format: Option<&str>) -> Self {
        Schema::Primitive(PrimitiveSchema::number(format))
    }

    /// Create a boolean schema.
    pub fn boolean() -> Self {
        Schema::Primitive(PrimitiveSchema::boolean())
    }

    /// Create a reference schema.
    pub fn reference(name: &str) -> Self {
        Schema::Ref(RefSchema {
            reference: format!("#/components/schemas/{name}"),
        })
    }

    /// Create an array schema.
    pub fn array(items: Schema) -> Self {
        Schema::Array(ArraySchema {
            items: Box::new(items),
            min_items: None,
            max_items: None,
        })
    }

    /// Create an object schema with the given properties.
    pub fn object(properties: HashMap<String, Schema>, required: Vec<String>) -> Self {
        Schema::Object(ObjectSchema {
            title: None,
            description: None,
            properties,
            required,
            additional_properties: None,
        })
    }

    /// Permit null. Non-primitive schemas use a union with the null schema.
    #[must_use]
    pub fn nullable(mut self) -> Self {
        if let Schema::Primitive(ref mut p) = self {
            p.nullable = true;
            self
        } else {
            Self::any_of(vec![self, <()>::schema()])
        }
    }

    /// Set title on this schema (if object).
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        if let Schema::Object(ref mut o) = self {
            o.title = Some(title.into());
        }
        self
    }

    /// Set description on this schema (if object).
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        if let Schema::Object(ref mut o) = self {
            o.description = Some(description.into());
        }
        self
    }

    /// Create a string enum schema with allowed values.
    pub fn string_enum(values: Vec<String>) -> Self {
        Schema::Enum(EnumSchema {
            schema_type: SchemaType::String,
            enum_values: values,
        })
    }

    /// Create a oneOf schema (union type).
    pub fn one_of(schemas: Vec<Schema>) -> Self {
        Schema::OneOf(OneOfSchema { one_of: schemas })
    }

    /// Create an inclusive union, permitting values accepted by any branch.
    pub fn any_of(schemas: Vec<Schema>) -> Self {
        Schema::AnyOf(AnyOfSchema { any_of: schemas })
    }
}

/// Schema reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefSchema {
    /// Reference path (e.g., "#/components/schemas/Item").
    #[serde(rename = "$ref")]
    pub reference: String,
}

/// Object schema.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ObjectSchema {
    /// Schema title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Schema description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Object properties.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub properties: HashMap<String, Schema>,
    /// Required property names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
    /// Additional properties schema.
    #[serde(
        rename = "additionalProperties",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub additional_properties: Option<Box<Schema>>,
}

/// Array schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArraySchema {
    /// Item schema.
    pub items: Box<Schema>,
    /// Minimum items.
    #[serde(rename = "minItems", default, skip_serializing_if = "Option::is_none")]
    pub min_items: Option<usize>,
    /// Maximum items.
    #[serde(rename = "maxItems", default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<usize>,
}

/// Enum schema with allowed values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumSchema {
    /// JSON Schema type (typically string for enums).
    #[serde(rename = "type")]
    pub schema_type: SchemaType,
    /// Allowed enum values.
    #[serde(rename = "enum")]
    pub enum_values: Vec<String>,
}

/// OneOf schema (union type).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OneOfSchema {
    /// List of possible schemas.
    #[serde(rename = "oneOf")]
    pub one_of: Vec<Schema>,
}

/// Inclusive union, used for nullable types whose inner schema may allow null.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnyOfSchema {
    /// Alternatives; a value must match at least one.
    #[serde(rename = "anyOf")]
    pub any_of: Vec<Schema>,
}

/// Primitive type schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrimitiveSchema {
    /// JSON Schema type.
    #[serde(rename = "type")]
    pub schema_type: SchemaType,
    /// Format hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Nullable flag (OpenAPI 3.1).
    #[serde(default, skip_serializing_if = "is_false")]
    pub nullable: bool,
}

impl PrimitiveSchema {
    /// Create a string schema.
    pub fn string() -> Self {
        Self {
            schema_type: SchemaType::String,
            format: None,
            nullable: false,
        }
    }

    /// Create an integer schema with optional format.
    pub fn integer(format: Option<&str>) -> Self {
        Self {
            schema_type: SchemaType::Integer,
            format: format.map(String::from),
            nullable: false,
        }
    }

    /// Create a number schema with optional format.
    pub fn number(format: Option<&str>) -> Self {
        Self {
            schema_type: SchemaType::Number,
            format: format.map(String::from),
            nullable: false,
        }
    }

    /// Create a boolean schema.
    pub fn boolean() -> Self {
        Self {
            schema_type: SchemaType::Boolean,
            format: None,
            nullable: false,
        }
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

/// JSON Schema primitive types.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SchemaType {
    /// String type.
    String,
    /// Number type (float).
    Number,
    /// Integer type.
    Integer,
    /// Boolean type.
    Boolean,
    /// Null type.
    Null,
}

/// Trait for types that can generate JSON Schema.
pub trait JsonSchema {
    /// Generate the JSON Schema for this type.
    fn schema() -> Schema;

    /// Get the schema name for use in `#/components/schemas/`.
    #[must_use]
    fn schema_name() -> Option<&'static str> {
        None
    }
}

// Implement for primitive types
impl JsonSchema for String {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema {
            schema_type: SchemaType::String,
            format: None,
            nullable: false,
        })
    }
}

impl JsonSchema for i64 {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema {
            schema_type: SchemaType::Integer,
            format: Some("int64".to_string()),
            nullable: false,
        })
    }
}

impl JsonSchema for i32 {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema {
            schema_type: SchemaType::Integer,
            format: Some("int32".to_string()),
            nullable: false,
        })
    }
}

impl JsonSchema for f64 {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema {
            schema_type: SchemaType::Number,
            format: Some("double".to_string()),
            nullable: false,
        })
    }
}

impl JsonSchema for bool {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema {
            schema_type: SchemaType::Boolean,
            format: None,
            nullable: false,
        })
    }
}

macro_rules! integer_schema {
    ($ty:ty, $format:expr) => {
        impl JsonSchema for $ty {
            fn schema() -> Schema {
                Schema::integer($format)
            }
        }
    };
}

integer_schema!(i8, Some("int8"));
integer_schema!(i16, Some("int16"));
integer_schema!(u8, Some("uint8"));
integer_schema!(u16, Some("uint16"));
integer_schema!(u32, Some("uint32"));
integer_schema!(u64, Some("uint64"));
integer_schema!(isize, Some("int64"));
integer_schema!(usize, Some("uint64"));
integer_schema!(i128, None);
integer_schema!(u128, None);

impl JsonSchema for f32 {
    fn schema() -> Schema {
        Schema::number(Some("float"))
    }
}

impl JsonSchema for &str {
    fn schema() -> Schema {
        Schema::string()
    }
}

impl JsonSchema for () {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema {
            schema_type: SchemaType::Null,
            format: None,
            nullable: false,
        })
    }
}

impl JsonSchema for serde_json::Value {
    fn schema() -> Schema {
        Schema::Boolean(true)
    }
}

impl<T: JsonSchema, S: std::hash::BuildHasher> JsonSchema for HashMap<String, T, S> {
    fn schema() -> Schema {
        Schema::Object(ObjectSchema {
            additional_properties: Some(Box::new(T::schema())),
            ..ObjectSchema::default()
        })
    }
}

impl<T: JsonSchema> JsonSchema for std::collections::BTreeMap<String, T> {
    fn schema() -> Schema {
        <HashMap<String, T>>::schema()
    }
}

impl<T: JsonSchema> JsonSchema for Option<T> {
    fn schema() -> Schema {
        T::schema().nullable()
    }
}

impl<T: JsonSchema> JsonSchema for Vec<T> {
    fn schema() -> Schema {
        Schema::Array(ArraySchema {
            items: Box::new(T::schema()),
            min_items: None,
            max_items: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_array_uses_json_schema_keywords_and_round_trips() {
        let schema = Schema::Array(ArraySchema {
            items: Box::new(Schema::integer(None)),
            min_items: Some(2),
            max_items: Some(4),
        });
        let json = serde_json::to_value(schema).unwrap();
        assert_eq!(json["type"], "array");
        assert_eq!(json["minItems"], 2);
        assert_eq!(json["maxItems"], 4);
        assert!(json["min_items"].is_null());
        assert!(json["max_items"].is_null());
        let decoded: Schema = serde_json::from_value(json.clone()).unwrap();
        assert!(
            matches!(&decoded, Schema::Array(array) if array.min_items == Some(2) && array.max_items == Some(4))
        );
        assert_eq!(serde_json::to_value(decoded).unwrap(), json);
    }

    #[test]
    fn optional_array_schema_accepts_real_null_and_retains_item_type() {
        let json = serde_json::to_value(<Option<Vec<String>>>::schema()).unwrap();
        assert_eq!(json["anyOf"][0]["type"], "array");
        assert_eq!(json["anyOf"][0]["items"]["type"], "string");
        assert_eq!(json["anyOf"][1]["type"], "null");
        let decoded: Schema = serde_json::from_value(json.clone()).unwrap();
        assert!(matches!(&decoded, Schema::AnyOf(_)));
        assert_eq!(serde_json::to_value(decoded).unwrap(), json);
    }

    #[test]
    fn nullable_unions_allow_overlapping_null_branches() {
        let any_json = serde_json::to_value(<Option<serde_json::Value>>::schema()).unwrap();
        assert_eq!(any_json["anyOf"][0], true);
        assert_eq!(any_json["anyOf"][1]["type"], "null");
        assert!(any_json["oneOf"].is_null());
        let nested = serde_json::to_value(<Option<Option<Vec<String>>>>::schema()).unwrap();
        assert_eq!(nested["anyOf"][0]["anyOf"][1]["type"], "null");
        assert_eq!(nested["anyOf"][1]["type"], "null");
        let null = serde_json::to_value(<Option<()>>::schema()).unwrap();
        assert_eq!(null["anyOf"][0]["type"], "null");
        assert_eq!(null["anyOf"][1]["type"], "null");
    }
}
