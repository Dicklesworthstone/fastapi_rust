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
    /// Object schema.
    Object(ObjectSchema),
    /// Array schema.
    Array(ArraySchema),
    /// Primitive type schema.
    Primitive(PrimitiveSchema),
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

    /// Set nullable on this schema (if primitive).
    #[must_use]
    pub fn nullable(mut self) -> Self {
        if let Schema::Primitive(ref mut p) = self {
            p.nullable = true;
        }
        self
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_properties: Option<Box<Schema>>,
}

/// Array schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArraySchema {
    /// Item schema.
    pub items: Box<Schema>,
    /// Minimum items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_items: Option<usize>,
    /// Maximum items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<usize>,
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

impl<T: JsonSchema> JsonSchema for Option<T> {
    fn schema() -> Schema {
        match T::schema() {
            Schema::Primitive(mut p) => {
                p.nullable = true;
                Schema::Primitive(p)
            }
            other => other,
        }
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
