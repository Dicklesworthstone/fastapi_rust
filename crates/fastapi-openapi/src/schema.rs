//! JSON Schema types for OpenAPI 3.1.

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
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
    /// Enum/union schema (oneOf, anyOf, allOf).
    Enum(EnumSchema),
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

    /// Create a oneOf schema (discriminated union - exactly one must match).
    pub fn one_of(schemas: Vec<Schema>) -> Self {
        Schema::Enum(EnumSchema {
            one_of: schemas,
            ..Default::default()
        })
    }

    /// Create an anyOf schema (untagged union - at least one must match).
    pub fn any_of(schemas: Vec<Schema>) -> Self {
        Schema::Enum(EnumSchema {
            any_of: schemas,
            ..Default::default()
        })
    }

    /// Create an allOf schema (intersection - all must match).
    pub fn all_of(schemas: Vec<Schema>) -> Self {
        Schema::Enum(EnumSchema {
            all_of: schemas,
            ..Default::default()
        })
    }

    /// Create a string enum schema (for unit variants only).
    pub fn string_enum(values: Vec<String>) -> Self {
        Schema::Primitive(PrimitiveSchema {
            schema_type: SchemaType::String,
            format: None,
            nullable: false,
            minimum: None,
            maximum: None,
            exclusive_minimum: None,
            exclusive_maximum: None,
            min_length: None,
            max_length: None,
            pattern: None,
            enum_values: Some(values),
        })
    }

    /// Create a oneOf schema with discriminator.
    pub fn one_of_with_discriminator(
        schemas: Vec<Schema>,
        property_name: impl Into<String>,
        mapping: HashMap<String, String>,
    ) -> Self {
        Schema::Enum(EnumSchema {
            one_of: schemas,
            discriminator: Some(Discriminator {
                property_name: property_name.into(),
                mapping,
            }),
            ..Default::default()
        })
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

/// Enum/union schema supporting oneOf, anyOf, allOf, and string enums.
///
/// This is used for Rust enums which map to various OpenAPI constructs:
/// - Unit variants only → string enum with `enum` keyword
/// - Mixed variants → `oneOf` with discriminated union
/// - Untagged enums → `anyOf`
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnumSchema {
    /// Schema title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Schema description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// oneOf schemas (discriminated union - exactly one must match).
    #[serde(default, skip_serializing_if = "Vec::is_empty", rename = "oneOf")]
    pub one_of: Vec<Schema>,
    /// anyOf schemas (untagged union - at least one must match).
    #[serde(default, skip_serializing_if = "Vec::is_empty", rename = "anyOf")]
    pub any_of: Vec<Schema>,
    /// allOf schemas (intersection - all must match).
    #[serde(default, skip_serializing_if = "Vec::is_empty", rename = "allOf")]
    pub all_of: Vec<Schema>,
    /// Discriminator for oneOf schemas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discriminator: Option<Discriminator>,
}

/// Discriminator for oneOf schemas (OpenAPI 3.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Discriminator {
    /// Property name that discriminates between variants.
    #[serde(rename = "propertyName")]
    pub property_name: String,
    /// Mapping from discriminator values to schema references.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub mapping: HashMap<String, String>,
}

/// Schema for a constant value (used for unit enum variants).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstSchema {
    /// The constant value.
    #[serde(rename = "const")]
    pub const_value: serde_json::Value,
}

/// Schema for string enums (unit variants only).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StringEnumSchema {
    /// Schema type (always "string").
    #[serde(rename = "type")]
    pub schema_type: SchemaType,
    /// Allowed enum values.
    #[serde(rename = "enum")]
    pub enum_values: Vec<String>,
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
    /// Minimum value constraint (>= for numbers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<i64>,
    /// Maximum value constraint (<= for numbers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<i64>,
    /// Exclusive minimum value constraint (> for numbers).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "exclusiveMinimum"
    )]
    pub exclusive_minimum: Option<i64>,
    /// Exclusive maximum value constraint (< for numbers).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "exclusiveMaximum"
    )]
    pub exclusive_maximum: Option<i64>,
    /// Minimum length constraint (for strings).
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "minLength")]
    pub min_length: Option<usize>,
    /// Maximum length constraint (for strings).
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "maxLength")]
    pub max_length: Option<usize>,
    /// Pattern constraint (regex for strings).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// Enum values (for string enums with unit variants).
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "enum")]
    pub enum_values: Option<Vec<String>>,
}

impl PrimitiveSchema {
    /// Create a string schema.
    pub fn string() -> Self {
        Self {
            schema_type: SchemaType::String,
            format: None,
            nullable: false,
            minimum: None,
            maximum: None,
            exclusive_minimum: None,
            exclusive_maximum: None,
            min_length: None,
            max_length: None,
            pattern: None,
            enum_values: None,
        }
    }

    /// Create an integer schema with optional format.
    pub fn integer(format: Option<&str>) -> Self {
        Self {
            schema_type: SchemaType::Integer,
            format: format.map(String::from),
            nullable: false,
            minimum: None,
            maximum: None,
            exclusive_minimum: None,
            exclusive_maximum: None,
            min_length: None,
            max_length: None,
            pattern: None,
            enum_values: None,
        }
    }

    /// Create an unsigned integer schema with optional format.
    pub fn unsigned_integer(format: Option<&str>) -> Self {
        Self {
            schema_type: SchemaType::Integer,
            format: format.map(String::from),
            nullable: false,
            minimum: Some(0),
            maximum: None,
            exclusive_minimum: None,
            exclusive_maximum: None,
            min_length: None,
            max_length: None,
            pattern: None,
            enum_values: None,
        }
    }

    /// Create a number schema with optional format.
    pub fn number(format: Option<&str>) -> Self {
        Self {
            schema_type: SchemaType::Number,
            format: format.map(String::from),
            nullable: false,
            minimum: None,
            maximum: None,
            exclusive_minimum: None,
            exclusive_maximum: None,
            min_length: None,
            max_length: None,
            pattern: None,
            enum_values: None,
        }
    }

    /// Create a boolean schema.
    pub fn boolean() -> Self {
        Self {
            schema_type: SchemaType::Boolean,
            format: None,
            nullable: false,
            minimum: None,
            maximum: None,
            exclusive_minimum: None,
            exclusive_maximum: None,
            min_length: None,
            max_length: None,
            pattern: None,
            enum_values: None,
        }
    }

    /// Set minimum value constraint (>=).
    #[must_use]
    pub fn with_minimum(mut self, value: i64) -> Self {
        self.minimum = Some(value);
        self
    }

    /// Set maximum value constraint (<=).
    #[must_use]
    pub fn with_maximum(mut self, value: i64) -> Self {
        self.maximum = Some(value);
        self
    }

    /// Set exclusive minimum value constraint (>).
    #[must_use]
    pub fn with_exclusive_minimum(mut self, value: i64) -> Self {
        self.exclusive_minimum = Some(value);
        self
    }

    /// Set exclusive maximum value constraint (<).
    #[must_use]
    pub fn with_exclusive_maximum(mut self, value: i64) -> Self {
        self.exclusive_maximum = Some(value);
        self
    }

    /// Set minimum length constraint (for strings).
    #[must_use]
    pub fn with_min_length(mut self, len: usize) -> Self {
        self.min_length = Some(len);
        self
    }

    /// Set maximum length constraint (for strings).
    #[must_use]
    pub fn with_max_length(mut self, len: usize) -> Self {
        self.max_length = Some(len);
        self
    }

    /// Set pattern constraint (regex for strings).
    #[must_use]
    pub fn with_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.pattern = Some(pattern.into());
        self
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

    /// Get the schema for this type, registering it with the given registry.
    ///
    /// If the type has a schema name, this registers the full schema definition
    /// and returns a `$ref` reference. Otherwise, returns the inline schema.
    fn schema_with_registry(registry: &SchemaRegistry) -> Schema
    where
        Self: Sized,
    {
        if let Some(name) = Self::schema_name() {
            registry.get_or_register::<Self>(name)
        } else {
            Self::schema()
        }
    }
}

// ============================================================================
// Schema Registry for $ref support
// ============================================================================

/// Registry for collecting and deduplicating JSON schemas.
///
/// The `SchemaRegistry` tracks schemas by name and returns `$ref` references
/// when a schema is already registered. This prevents schema duplication
/// in OpenAPI documents.
///
/// # Example
///
/// ```ignore
/// use fastapi_openapi::{SchemaRegistry, JsonSchema, Schema};
///
/// #[derive(JsonSchema)]
/// struct User {
///     id: i64,
///     name: String,
/// }
///
/// let registry = SchemaRegistry::new();
///
/// // First access registers the schema and returns a $ref
/// let schema1 = User::schema_with_registry(&registry);
/// assert!(matches!(schema1, Schema::Ref(_)));
///
/// // Second access returns the same $ref without re-registering
/// let schema2 = User::schema_with_registry(&registry);
/// assert!(matches!(schema2, Schema::Ref(_)));
///
/// // Export all collected schemas for components/schemas
/// let schemas = registry.into_schemas();
/// assert!(schemas.contains_key("User"));
/// ```
#[derive(Debug, Default)]
pub struct SchemaRegistry {
    /// Registered schemas by name.
    schemas: RefCell<HashMap<String, Schema>>,
}

impl SchemaRegistry {
    /// Create a new empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            schemas: RefCell::new(HashMap::new()),
        }
    }

    /// Get or register a schema by name.
    ///
    /// If the schema is already registered, returns a `$ref` reference.
    /// Otherwise, generates the schema, registers it, and returns a `$ref`.
    pub fn get_or_register<T: JsonSchema>(&self, name: &str) -> Schema {
        let mut schemas = self.schemas.borrow_mut();

        if !schemas.contains_key(name) {
            // Register the full schema definition
            schemas.insert(name.to_string(), T::schema());
        }

        // Return a $ref
        Schema::reference(name)
    }

    /// Register a schema directly by name.
    ///
    /// Returns a `$ref` to the registered schema.
    pub fn register(&self, name: impl Into<String>, schema: Schema) -> Schema {
        let name = name.into();
        self.schemas.borrow_mut().insert(name.clone(), schema);
        Schema::reference(&name)
    }

    /// Check if a schema with the given name is already registered.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.schemas.borrow().contains_key(name)
    }

    /// Get the number of registered schemas.
    #[must_use]
    pub fn len(&self) -> usize {
        self.schemas.borrow().len()
    }

    /// Check if the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.schemas.borrow().is_empty()
    }

    /// Consume the registry and return all collected schemas.
    ///
    /// The returned map is suitable for use in `components.schemas`.
    #[must_use]
    pub fn into_schemas(self) -> HashMap<String, Schema> {
        self.schemas.into_inner()
    }

    /// Get a clone of all registered schemas without consuming the registry.
    #[must_use]
    pub fn schemas(&self) -> HashMap<String, Schema> {
        self.schemas.borrow().clone()
    }

    /// Merge another registry's schemas into this one.
    ///
    /// If a schema with the same name exists in both, the existing one is kept.
    pub fn merge(&self, other: &SchemaRegistry) {
        let mut schemas = self.schemas.borrow_mut();
        for (name, schema) in other.schemas.borrow().iter() {
            schemas
                .entry(name.clone())
                .or_insert_with(|| schema.clone());
        }
    }
}

impl Clone for SchemaRegistry {
    fn clone(&self) -> Self {
        Self {
            schemas: RefCell::new(self.schemas.borrow().clone()),
        }
    }
}

// ============================================================================
// Primitive type implementations
// ============================================================================

// Implement for primitive types
impl JsonSchema for String {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema::string())
    }
}

impl JsonSchema for &str {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema::string())
    }
}

impl JsonSchema for bool {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema::boolean())
    }
}

// Signed integers
impl JsonSchema for i8 {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema::integer(Some("int8")))
    }
}

impl JsonSchema for i16 {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema::integer(Some("int16")))
    }
}

impl JsonSchema for i32 {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema::integer(Some("int32")))
    }
}

impl JsonSchema for i64 {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema::integer(Some("int64")))
    }
}

impl JsonSchema for i128 {
    fn schema() -> Schema {
        // i128 doesn't have a standard OpenAPI format, use integer without format
        Schema::Primitive(PrimitiveSchema::integer(None))
    }
}

impl JsonSchema for isize {
    fn schema() -> Schema {
        // isize is typically 64-bit on modern systems
        Schema::Primitive(PrimitiveSchema::integer(Some("int64")))
    }
}

// Unsigned integers (with minimum: 0)
impl JsonSchema for u8 {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema::unsigned_integer(Some("uint8")))
    }
}

impl JsonSchema for u16 {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema::unsigned_integer(Some("uint16")))
    }
}

impl JsonSchema for u32 {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema::unsigned_integer(Some("uint32")))
    }
}

impl JsonSchema for u64 {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema::unsigned_integer(Some("uint64")))
    }
}

impl JsonSchema for u128 {
    fn schema() -> Schema {
        // u128 doesn't have a standard OpenAPI format
        Schema::Primitive(PrimitiveSchema::unsigned_integer(None))
    }
}

impl JsonSchema for usize {
    fn schema() -> Schema {
        // usize is typically 64-bit on modern systems
        Schema::Primitive(PrimitiveSchema::unsigned_integer(Some("uint64")))
    }
}

// Floating point
impl JsonSchema for f32 {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema::number(Some("float")))
    }
}

impl JsonSchema for f64 {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema::number(Some("double")))
    }
}

// NonZero types
impl JsonSchema for std::num::NonZeroI8 {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema::integer(Some("int8")))
    }
}

impl JsonSchema for std::num::NonZeroI16 {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema::integer(Some("int16")))
    }
}

impl JsonSchema for std::num::NonZeroI32 {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema::integer(Some("int32")))
    }
}

impl JsonSchema for std::num::NonZeroI64 {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema::integer(Some("int64")))
    }
}

impl JsonSchema for std::num::NonZeroI128 {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema::integer(None))
    }
}

impl JsonSchema for std::num::NonZeroIsize {
    fn schema() -> Schema {
        Schema::Primitive(PrimitiveSchema::integer(Some("int64")))
    }
}

impl JsonSchema for std::num::NonZeroU8 {
    fn schema() -> Schema {
        let mut schema = PrimitiveSchema::unsigned_integer(Some("uint8"));
        schema.minimum = Some(1); // NonZero must be >= 1
        Schema::Primitive(schema)
    }
}

impl JsonSchema for std::num::NonZeroU16 {
    fn schema() -> Schema {
        let mut schema = PrimitiveSchema::unsigned_integer(Some("uint16"));
        schema.minimum = Some(1);
        Schema::Primitive(schema)
    }
}

impl JsonSchema for std::num::NonZeroU32 {
    fn schema() -> Schema {
        let mut schema = PrimitiveSchema::unsigned_integer(Some("uint32"));
        schema.minimum = Some(1);
        Schema::Primitive(schema)
    }
}

impl JsonSchema for std::num::NonZeroU64 {
    fn schema() -> Schema {
        let mut schema = PrimitiveSchema::unsigned_integer(Some("uint64"));
        schema.minimum = Some(1);
        Schema::Primitive(schema)
    }
}

impl JsonSchema for std::num::NonZeroU128 {
    fn schema() -> Schema {
        let mut schema = PrimitiveSchema::unsigned_integer(None);
        schema.minimum = Some(1);
        Schema::Primitive(schema)
    }
}

impl JsonSchema for std::num::NonZeroUsize {
    fn schema() -> Schema {
        let mut schema = PrimitiveSchema::unsigned_integer(Some("uint64"));
        schema.minimum = Some(1);
        Schema::Primitive(schema)
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

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to extract primitive schema details
    fn get_primitive(schema: Schema) -> PrimitiveSchema {
        match schema {
            Schema::Primitive(p) => p,
            _ => panic!("Expected primitive schema"),
        }
    }

    #[test]
    fn test_signed_integers() {
        // i8
        let s = get_primitive(i8::schema());
        assert!(matches!(s.schema_type, SchemaType::Integer));
        assert_eq!(s.format.as_deref(), Some("int8"));
        assert_eq!(s.minimum, None);

        // i16
        let s = get_primitive(i16::schema());
        assert!(matches!(s.schema_type, SchemaType::Integer));
        assert_eq!(s.format.as_deref(), Some("int16"));

        // i32
        let s = get_primitive(i32::schema());
        assert!(matches!(s.schema_type, SchemaType::Integer));
        assert_eq!(s.format.as_deref(), Some("int32"));

        // i64
        let s = get_primitive(i64::schema());
        assert!(matches!(s.schema_type, SchemaType::Integer));
        assert_eq!(s.format.as_deref(), Some("int64"));

        // i128 (no standard format)
        let s = get_primitive(i128::schema());
        assert!(matches!(s.schema_type, SchemaType::Integer));
        assert_eq!(s.format, None);

        // isize
        let s = get_primitive(isize::schema());
        assert!(matches!(s.schema_type, SchemaType::Integer));
        assert_eq!(s.format.as_deref(), Some("int64"));
    }

    #[test]
    fn test_unsigned_integers() {
        // u8
        let s = get_primitive(u8::schema());
        assert!(matches!(s.schema_type, SchemaType::Integer));
        assert_eq!(s.format.as_deref(), Some("uint8"));
        assert_eq!(s.minimum, Some(0));

        // u16
        let s = get_primitive(u16::schema());
        assert!(matches!(s.schema_type, SchemaType::Integer));
        assert_eq!(s.format.as_deref(), Some("uint16"));
        assert_eq!(s.minimum, Some(0));

        // u32
        let s = get_primitive(u32::schema());
        assert!(matches!(s.schema_type, SchemaType::Integer));
        assert_eq!(s.format.as_deref(), Some("uint32"));
        assert_eq!(s.minimum, Some(0));

        // u64
        let s = get_primitive(u64::schema());
        assert!(matches!(s.schema_type, SchemaType::Integer));
        assert_eq!(s.format.as_deref(), Some("uint64"));
        assert_eq!(s.minimum, Some(0));

        // u128 (no standard format)
        let s = get_primitive(u128::schema());
        assert!(matches!(s.schema_type, SchemaType::Integer));
        assert_eq!(s.format, None);
        assert_eq!(s.minimum, Some(0));

        // usize
        let s = get_primitive(usize::schema());
        assert!(matches!(s.schema_type, SchemaType::Integer));
        assert_eq!(s.format.as_deref(), Some("uint64"));
        assert_eq!(s.minimum, Some(0));
    }

    #[test]
    fn test_floats() {
        // f32
        let s = get_primitive(f32::schema());
        assert!(matches!(s.schema_type, SchemaType::Number));
        assert_eq!(s.format.as_deref(), Some("float"));

        // f64
        let s = get_primitive(f64::schema());
        assert!(matches!(s.schema_type, SchemaType::Number));
        assert_eq!(s.format.as_deref(), Some("double"));
    }

    #[test]
    fn test_nonzero_signed() {
        use std::num::{NonZeroI32, NonZeroI64};

        let s = get_primitive(NonZeroI32::schema());
        assert!(matches!(s.schema_type, SchemaType::Integer));
        assert_eq!(s.format.as_deref(), Some("int32"));

        let s = get_primitive(NonZeroI64::schema());
        assert!(matches!(s.schema_type, SchemaType::Integer));
        assert_eq!(s.format.as_deref(), Some("int64"));
    }

    #[test]
    fn test_nonzero_unsigned() {
        use std::num::{NonZeroU32, NonZeroU64};

        // NonZero unsigned should have minimum: 1
        let s = get_primitive(NonZeroU32::schema());
        assert!(matches!(s.schema_type, SchemaType::Integer));
        assert_eq!(s.format.as_deref(), Some("uint32"));
        assert_eq!(s.minimum, Some(1));

        let s = get_primitive(NonZeroU64::schema());
        assert!(matches!(s.schema_type, SchemaType::Integer));
        assert_eq!(s.format.as_deref(), Some("uint64"));
        assert_eq!(s.minimum, Some(1));
    }

    #[test]
    fn test_string_and_bool() {
        let s = get_primitive(String::schema());
        assert!(matches!(s.schema_type, SchemaType::String));

        let s = get_primitive(bool::schema());
        assert!(matches!(s.schema_type, SchemaType::Boolean));
    }

    #[test]
    fn test_serialization() {
        // Verify JSON serialization of unsigned integer with minimum
        let schema = u32::schema();
        let json = serde_json::to_string(&schema).unwrap();
        assert!(json.contains(r#""type":"integer""#));
        assert!(json.contains(r#""format":"uint32""#));
        assert!(json.contains(r#""minimum":0"#));

        // Verify NonZero has minimum 1
        let schema = std::num::NonZeroU32::schema();
        let json = serde_json::to_string(&schema).unwrap();
        assert!(json.contains(r#""minimum":1"#));

        // Verify signed doesn't have minimum
        let schema = i32::schema();
        let json = serde_json::to_string(&schema).unwrap();
        assert!(!json.contains("minimum"));
    }

    #[test]
    fn test_string_enum_schema() {
        // Test string_enum helper
        let schema = Schema::string_enum(vec![
            "Red".to_string(),
            "Green".to_string(),
            "Blue".to_string(),
        ]);
        let json = serde_json::to_string(&schema).unwrap();
        assert!(json.contains(r#""type":"string""#));
        assert!(json.contains(r#""enum":["Red","Green","Blue"]"#));
    }

    #[test]
    fn test_one_of_schema() {
        // Test oneOf helper
        let schema = Schema::one_of(vec![Schema::string(), Schema::integer(Some("int32"))]);
        let json = serde_json::to_string(&schema).unwrap();
        assert!(json.contains(r#""oneOf""#));
    }

    #[test]
    fn test_any_of_schema() {
        // Test anyOf helper (for untagged enums)
        let schema = Schema::any_of(vec![Schema::string(), Schema::boolean()]);
        let json = serde_json::to_string(&schema).unwrap();
        assert!(json.contains(r#""anyOf""#));
    }

    #[test]
    fn test_enum_schema_with_discriminator() {
        // Test oneOf with discriminator
        let mut mapping = HashMap::new();
        mapping.insert("dog".to_string(), "#/components/schemas/Dog".to_string());
        mapping.insert("cat".to_string(), "#/components/schemas/Cat".to_string());

        let schema = Schema::one_of_with_discriminator(
            vec![Schema::reference("Dog"), Schema::reference("Cat")],
            "petType",
            mapping,
        );
        let json = serde_json::to_string(&schema).unwrap();
        assert!(json.contains(r#""oneOf""#));
        assert!(json.contains(r#""discriminator""#));
        assert!(json.contains(r#""propertyName":"petType""#));
    }

    // =========================================================================
    // Schema Constraint Tests
    // =========================================================================

    #[test]
    fn test_exclusive_minimum_serialization() {
        let schema = PrimitiveSchema::integer(Some("int32")).with_exclusive_minimum(5);
        let json = serde_json::to_string(&Schema::Primitive(schema)).unwrap();
        assert!(json.contains(r#""exclusiveMinimum":5"#));
        // Should not have regular minimum
        assert!(!json.contains(r#""minimum""#));
    }

    #[test]
    fn test_exclusive_maximum_serialization() {
        let schema = PrimitiveSchema::integer(Some("int32")).with_exclusive_maximum(100);
        let json = serde_json::to_string(&Schema::Primitive(schema)).unwrap();
        assert!(json.contains(r#""exclusiveMaximum":100"#));
        // Should not have regular maximum
        assert!(!json.contains(r#""maximum""#));
    }

    #[test]
    fn test_min_length_serialization() {
        let schema = PrimitiveSchema::string().with_min_length(3);
        let json = serde_json::to_string(&Schema::Primitive(schema)).unwrap();
        assert!(json.contains(r#""minLength":3"#));
    }

    #[test]
    fn test_max_length_serialization() {
        let schema = PrimitiveSchema::string().with_max_length(255);
        let json = serde_json::to_string(&Schema::Primitive(schema)).unwrap();
        assert!(json.contains(r#""maxLength":255"#));
    }

    #[test]
    fn test_pattern_serialization() {
        let schema = PrimitiveSchema::string().with_pattern(r"^[a-z]+$");
        let json = serde_json::to_string(&Schema::Primitive(schema)).unwrap();
        assert!(json.contains(r#""pattern":"^[a-z]+$""#));
    }

    #[test]
    fn test_combined_string_constraints() {
        let schema = PrimitiveSchema::string()
            .with_min_length(1)
            .with_max_length(50)
            .with_pattern(r"^[A-Z][a-z]+$");
        let json = serde_json::to_string(&Schema::Primitive(schema)).unwrap();
        assert!(json.contains(r#""minLength":1"#));
        assert!(json.contains(r#""maxLength":50"#));
        assert!(json.contains(r#""pattern":"^[A-Z][a-z]+$""#));
    }

    #[test]
    fn test_combined_number_constraints() {
        let schema = PrimitiveSchema::integer(Some("int32"))
            .with_minimum(0)
            .with_maximum(100);
        let json = serde_json::to_string(&Schema::Primitive(schema)).unwrap();
        assert!(json.contains(r#""minimum":0"#));
        assert!(json.contains(r#""maximum":100"#));
    }

    #[test]
    fn test_constraints_not_serialized_when_none() {
        let schema = PrimitiveSchema::string();
        let json = serde_json::to_string(&Schema::Primitive(schema)).unwrap();
        // None of the constraint fields should appear
        assert!(!json.contains("minimum"));
        assert!(!json.contains("maximum"));
        assert!(!json.contains("exclusiveMinimum"));
        assert!(!json.contains("exclusiveMaximum"));
        assert!(!json.contains("minLength"));
        assert!(!json.contains("maxLength"));
        assert!(!json.contains("pattern"));
    }

    // =========================================================================
    // SchemaRegistry Tests
    // =========================================================================

    #[test]
    fn test_registry_new_is_empty() {
        let registry = SchemaRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn test_registry_register_direct() {
        let registry = SchemaRegistry::new();
        let schema = Schema::string();

        let result = registry.register("Username", schema);

        assert!(registry.contains("Username"));
        assert_eq!(registry.len(), 1);

        // Result should be a $ref
        if let Schema::Ref(ref_schema) = result {
            assert_eq!(ref_schema.reference, "#/components/schemas/Username");
        } else {
            panic!("Expected Schema::Ref");
        }
    }

    #[test]
    fn test_registry_get_or_register_new() {
        let registry = SchemaRegistry::new();

        // First call registers and returns $ref
        let result = registry.get_or_register::<String>("StringType");

        assert!(registry.contains("StringType"));
        if let Schema::Ref(ref_schema) = result {
            assert_eq!(ref_schema.reference, "#/components/schemas/StringType");
        } else {
            panic!("Expected Schema::Ref");
        }
    }

    #[test]
    fn test_registry_get_or_register_existing() {
        let registry = SchemaRegistry::new();

        // First call
        let _result1 = registry.get_or_register::<String>("StringType");
        let initial_len = registry.len();

        // Second call should not add a new entry
        let result2 = registry.get_or_register::<String>("StringType");

        assert_eq!(registry.len(), initial_len);
        if let Schema::Ref(ref_schema) = result2 {
            assert_eq!(ref_schema.reference, "#/components/schemas/StringType");
        } else {
            panic!("Expected Schema::Ref");
        }
    }

    #[test]
    fn test_registry_into_schemas() {
        let registry = SchemaRegistry::new();
        registry.register("Type1", Schema::string());
        registry.register("Type2", Schema::boolean());

        let schemas = registry.into_schemas();

        assert_eq!(schemas.len(), 2);
        assert!(schemas.contains_key("Type1"));
        assert!(schemas.contains_key("Type2"));
    }

    #[test]
    fn test_registry_schemas_clone() {
        let registry = SchemaRegistry::new();
        registry.register("Type1", Schema::string());

        let schemas = registry.schemas();

        // Registry should still have the schema
        assert!(registry.contains("Type1"));
        assert_eq!(schemas.len(), 1);
    }

    #[test]
    fn test_registry_merge() {
        let registry1 = SchemaRegistry::new();
        registry1.register("Type1", Schema::string());

        let registry2 = SchemaRegistry::new();
        registry2.register("Type2", Schema::boolean());
        registry2.register("Type1", Schema::integer(Some("int32"))); // Duplicate name

        registry1.merge(&registry2);

        // Should have both types
        assert_eq!(registry1.len(), 2);
        assert!(registry1.contains("Type1"));
        assert!(registry1.contains("Type2"));

        // Original Type1 should be preserved (string, not integer)
        let schemas = registry1.into_schemas();
        if let Schema::Primitive(p) = &schemas["Type1"] {
            assert!(matches!(p.schema_type, SchemaType::String));
        } else {
            panic!("Expected primitive string schema");
        }
    }

    #[test]
    fn test_registry_clone() {
        let registry1 = SchemaRegistry::new();
        registry1.register("Type1", Schema::string());

        let registry2 = registry1.clone();

        assert!(registry2.contains("Type1"));
        assert_eq!(registry2.len(), 1);

        // Modifications to clone don't affect original
        registry2.register("Type2", Schema::boolean());
        assert!(!registry1.contains("Type2"));
        assert!(registry2.contains("Type2"));
    }

    #[test]
    fn test_ref_schema_serialization() {
        let ref_schema = Schema::reference("User");
        let json = serde_json::to_string(&ref_schema).unwrap();
        assert!(json.contains(r##""$ref":"#/components/schemas/User""##));
    }

    #[test]
    fn test_registry_with_object_schema() {
        let registry = SchemaRegistry::new();

        let user_schema = Schema::object(
            [
                ("id".to_string(), Schema::integer(Some("int64"))),
                ("name".to_string(), Schema::string()),
            ]
            .into_iter()
            .collect(),
            vec!["id".to_string(), "name".to_string()],
        );

        let result = registry.register("User", user_schema);

        // Should return a $ref
        if let Schema::Ref(ref_schema) = result {
            assert_eq!(ref_schema.reference, "#/components/schemas/User");
        } else {
            panic!("Expected Schema::Ref");
        }

        // The stored schema should be the object
        let schemas = registry.into_schemas();
        if let Schema::Object(obj) = &schemas["User"] {
            assert!(obj.properties.contains_key("id"));
            assert!(obj.properties.contains_key("name"));
            assert!(obj.required.contains(&"id".to_string()));
        } else {
            panic!("Expected object schema");
        }
    }

    #[test]
    fn test_registry_nested_refs() {
        let registry = SchemaRegistry::new();

        // Register Address schema
        let address_schema = Schema::object(
            [
                ("street".to_string(), Schema::string()),
                ("city".to_string(), Schema::string()),
            ]
            .into_iter()
            .collect(),
            vec!["street".to_string(), "city".to_string()],
        );
        let _address_ref = registry.register("Address", address_schema);

        // Register User schema with $ref to Address
        let user_schema = Schema::object(
            [
                ("name".to_string(), Schema::string()),
                ("address".to_string(), Schema::reference("Address")),
            ]
            .into_iter()
            .collect(),
            vec!["name".to_string()],
        );
        let _user_ref = registry.register("User", user_schema);

        let schemas = registry.into_schemas();
        assert_eq!(schemas.len(), 2);

        // User's address field should be a $ref
        if let Schema::Object(obj) = &schemas["User"] {
            if let Schema::Ref(ref_schema) = &obj.properties["address"] {
                assert_eq!(ref_schema.reference, "#/components/schemas/Address");
            } else {
                panic!("Expected address to be a $ref");
            }
        } else {
            panic!("Expected User to be an object");
        }
    }

    #[test]
    fn test_registry_default() {
        let registry = SchemaRegistry::default();
        assert!(registry.is_empty());
    }
}
