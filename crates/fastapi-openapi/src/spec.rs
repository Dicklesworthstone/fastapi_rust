//! OpenAPI 3.1 specification types.

use crate::schema::Schema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// OpenAPI 3.1 document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApi {
    /// OpenAPI version.
    pub openapi: String,
    /// API information.
    pub info: Info,
    /// Server list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<Server>,
    /// Path items.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub paths: HashMap<String, PathItem>,
    /// Reusable components.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub components: Option<Components>,
    /// API tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Tag>,
}

/// API information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Info {
    /// API title.
    pub title: String,
    /// API version.
    pub version: String,
    /// API description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Terms of service URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terms_of_service: Option<String>,
    /// Contact information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<Contact>,
    /// License information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<License>,
}

/// Contact information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contact {
    /// Name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Email.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// License information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct License {
    /// License name.
    pub name: String,
    /// License URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Server information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
    /// Server URL.
    pub url: String,
    /// Server description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Path item (operations for a path).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PathItem {
    /// GET operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub get: Option<Operation>,
    /// POST operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post: Option<Operation>,
    /// PUT operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub put: Option<Operation>,
    /// DELETE operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete: Option<Operation>,
    /// PATCH operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<Operation>,
    /// OPTIONS operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Operation>,
    /// HEAD operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<Operation>,
}

/// API operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    /// Operation ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    /// Summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Parameters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<Parameter>,
    /// Request body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body: Option<RequestBody>,
    /// Responses.
    pub responses: HashMap<String, Response>,
    /// Deprecated flag.
    #[serde(default, skip_serializing_if = "is_false")]
    pub deprecated: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Operation parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parameter {
    /// Parameter name.
    pub name: String,
    /// Parameter location.
    #[serde(rename = "in")]
    pub location: ParameterLocation,
    /// Required flag.
    #[serde(default)]
    pub required: bool,
    /// Parameter schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Schema>,
    /// Title for display in documentation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether the parameter is deprecated.
    #[serde(default, skip_serializing_if = "is_false")]
    pub deprecated: bool,
    /// Example value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,
    /// Named examples.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub examples: HashMap<String, Example>,
}

/// Example object for OpenAPI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Example {
    /// Summary of the example.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Long description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Example value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    /// External URL for the example.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_value: Option<String>,
}

/// Parameter metadata for OpenAPI documentation.
///
/// This struct captures metadata attributes that can be specified on
/// struct fields using `#[param(...)]` attributes.
///
/// # Example
///
/// ```ignore
/// #[derive(FromRequest)]
/// struct MyQuery {
///     #[param(description = "Search term", deprecated)]
///     q: Option<String>,
///
///     #[param(title = "Page Number", ge = 1)]
///     page: i32,
/// }
/// ```
#[derive(Debug, Clone, Default)]
pub struct ParamMeta {
    /// Display title for the parameter.
    pub title: Option<String>,
    /// Description of the parameter.
    pub description: Option<String>,
    /// Whether the parameter is deprecated.
    pub deprecated: bool,
    /// Whether to include in OpenAPI schema.
    pub include_in_schema: bool,
    /// Example value.
    pub example: Option<serde_json::Value>,
    /// Named examples.
    pub examples: HashMap<String, Example>,
    /// Minimum value constraint (for numbers).
    pub ge: Option<f64>,
    /// Maximum value constraint (for numbers).
    pub le: Option<f64>,
    /// Exclusive minimum (for numbers).
    pub gt: Option<f64>,
    /// Exclusive maximum (for numbers).
    pub lt: Option<f64>,
    /// Minimum length (for strings).
    pub min_length: Option<usize>,
    /// Maximum length (for strings).
    pub max_length: Option<usize>,
    /// Pattern constraint (regex).
    pub pattern: Option<String>,
}

impl ParamMeta {
    /// Create a new parameter metadata with default values.
    #[must_use]
    pub fn new() -> Self {
        Self {
            include_in_schema: true,
            ..Default::default()
        }
    }

    /// Set the title.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the description.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Mark as deprecated.
    #[must_use]
    pub fn deprecated(mut self) -> Self {
        self.deprecated = true;
        self
    }

    /// Exclude from OpenAPI schema.
    #[must_use]
    pub fn exclude_from_schema(mut self) -> Self {
        self.include_in_schema = false;
        self
    }

    /// Set an example value.
    #[must_use]
    pub fn example(mut self, example: serde_json::Value) -> Self {
        self.example = Some(example);
        self
    }

    /// Set minimum value constraint (>=).
    #[must_use]
    pub fn ge(mut self, value: f64) -> Self {
        self.ge = Some(value);
        self
    }

    /// Set maximum value constraint (<=).
    #[must_use]
    pub fn le(mut self, value: f64) -> Self {
        self.le = Some(value);
        self
    }

    /// Set exclusive minimum constraint (>).
    #[must_use]
    pub fn gt(mut self, value: f64) -> Self {
        self.gt = Some(value);
        self
    }

    /// Set exclusive maximum constraint (<).
    #[must_use]
    pub fn lt(mut self, value: f64) -> Self {
        self.lt = Some(value);
        self
    }

    /// Set minimum length for strings.
    #[must_use]
    pub fn min_length(mut self, len: usize) -> Self {
        self.min_length = Some(len);
        self
    }

    /// Set maximum length for strings.
    #[must_use]
    pub fn max_length(mut self, len: usize) -> Self {
        self.max_length = Some(len);
        self
    }

    /// Set a regex pattern constraint.
    #[must_use]
    pub fn pattern(mut self, pattern: impl Into<String>) -> Self {
        self.pattern = Some(pattern.into());
        self
    }

    /// Convert to an OpenAPI Parameter.
    #[must_use]
    pub fn to_parameter(
        &self,
        name: impl Into<String>,
        location: ParameterLocation,
        required: bool,
        schema: Option<Schema>,
    ) -> Parameter {
        Parameter {
            name: name.into(),
            location,
            required,
            schema,
            title: self.title.clone(),
            description: self.description.clone(),
            deprecated: self.deprecated,
            example: self.example.clone(),
            examples: self.examples.clone(),
        }
    }
}

/// Trait for types that provide parameter metadata.
///
/// Implement this trait to enable automatic OpenAPI parameter documentation.
pub trait HasParamMeta {
    /// Get the parameter metadata for this type.
    fn param_meta() -> ParamMeta {
        ParamMeta::new()
    }
}

/// Parameter location.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParameterLocation {
    /// Path parameter.
    Path,
    /// Query parameter.
    Query,
    /// Header parameter.
    Header,
    /// Cookie parameter.
    Cookie,
}

/// Request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestBody {
    /// Required flag.
    #[serde(default)]
    pub required: bool,
    /// Content by media type.
    pub content: HashMap<String, MediaType>,
    /// Description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Media type content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaType {
    /// Schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Schema>,
}

/// Response definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// Description.
    pub description: String,
    /// Content by media type.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub content: HashMap<String, MediaType>,
}

/// Reusable components.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Components {
    /// Schema definitions.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub schemas: HashMap<String, Schema>,
}

/// API tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    /// Tag name.
    pub name: String,
    /// Tag description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// ============================================================================
// Tests for ParamMeta
// ============================================================================

#[cfg(test)]
mod param_meta_tests {
    use super::*;

    #[test]
    fn new_creates_default_with_include_in_schema_true() {
        let meta = ParamMeta::new();
        assert!(meta.include_in_schema);
        assert!(meta.title.is_none());
        assert!(meta.description.is_none());
        assert!(!meta.deprecated);
    }

    #[test]
    fn title_sets_title() {
        let meta = ParamMeta::new().title("User ID");
        assert_eq!(meta.title.as_deref(), Some("User ID"));
    }

    #[test]
    fn description_sets_description() {
        let meta = ParamMeta::new().description("The unique identifier");
        assert_eq!(meta.description.as_deref(), Some("The unique identifier"));
    }

    #[test]
    fn deprecated_marks_as_deprecated() {
        let meta = ParamMeta::new().deprecated();
        assert!(meta.deprecated);
    }

    #[test]
    fn exclude_from_schema_sets_include_false() {
        let meta = ParamMeta::new().exclude_from_schema();
        assert!(!meta.include_in_schema);
    }

    #[test]
    fn example_sets_example_value() {
        let meta = ParamMeta::new().example(serde_json::json!(42));
        assert_eq!(meta.example, Some(serde_json::json!(42)));
    }

    #[test]
    fn ge_sets_minimum_constraint() {
        let meta = ParamMeta::new().ge(1.0);
        assert_eq!(meta.ge, Some(1.0));
    }

    #[test]
    fn le_sets_maximum_constraint() {
        let meta = ParamMeta::new().le(100.0);
        assert_eq!(meta.le, Some(100.0));
    }

    #[test]
    fn gt_sets_exclusive_minimum() {
        let meta = ParamMeta::new().gt(0.0);
        assert_eq!(meta.gt, Some(0.0));
    }

    #[test]
    fn lt_sets_exclusive_maximum() {
        let meta = ParamMeta::new().lt(1000.0);
        assert_eq!(meta.lt, Some(1000.0));
    }

    #[test]
    fn min_length_sets_minimum_string_length() {
        let meta = ParamMeta::new().min_length(3);
        assert_eq!(meta.min_length, Some(3));
    }

    #[test]
    fn max_length_sets_maximum_string_length() {
        let meta = ParamMeta::new().max_length(255);
        assert_eq!(meta.max_length, Some(255));
    }

    #[test]
    fn pattern_sets_regex_constraint() {
        let meta = ParamMeta::new().pattern(r"^\d{4}-\d{2}-\d{2}$");
        assert_eq!(meta.pattern.as_deref(), Some(r"^\d{4}-\d{2}-\d{2}$"));
    }

    #[test]
    fn builder_methods_chain() {
        let meta = ParamMeta::new()
            .title("Page")
            .description("Page number for pagination")
            .ge(1.0)
            .le(1000.0)
            .example(serde_json::json!(1));

        assert_eq!(meta.title.as_deref(), Some("Page"));
        assert_eq!(
            meta.description.as_deref(),
            Some("Page number for pagination")
        );
        assert_eq!(meta.ge, Some(1.0));
        assert_eq!(meta.le, Some(1000.0));
        assert_eq!(meta.example, Some(serde_json::json!(1)));
    }

    #[test]
    fn to_parameter_creates_parameter_with_metadata() {
        let meta = ParamMeta::new()
            .title("User ID")
            .description("Unique user identifier")
            .deprecated()
            .example(serde_json::json!(42));

        let param = meta.to_parameter("user_id", ParameterLocation::Path, true, None);

        assert_eq!(param.name, "user_id");
        assert!(matches!(param.location, ParameterLocation::Path));
        assert!(param.required);
        assert_eq!(param.title.as_deref(), Some("User ID"));
        assert_eq!(param.description.as_deref(), Some("Unique user identifier"));
        assert!(param.deprecated);
        assert_eq!(param.example, Some(serde_json::json!(42)));
    }

    #[test]
    fn to_parameter_with_query_location() {
        let meta = ParamMeta::new().description("Search query");
        let param = meta.to_parameter("q", ParameterLocation::Query, false, None);

        assert_eq!(param.name, "q");
        assert!(matches!(param.location, ParameterLocation::Query));
        assert!(!param.required);
    }

    #[test]
    fn to_parameter_with_header_location() {
        let meta = ParamMeta::new().description("API key");
        let param = meta.to_parameter("X-API-Key", ParameterLocation::Header, true, None);

        assert_eq!(param.name, "X-API-Key");
        assert!(matches!(param.location, ParameterLocation::Header));
    }

    #[test]
    fn to_parameter_with_cookie_location() {
        let meta = ParamMeta::new().description("Session cookie");
        let param = meta.to_parameter("session", ParameterLocation::Cookie, false, None);

        assert_eq!(param.name, "session");
        assert!(matches!(param.location, ParameterLocation::Cookie));
    }

    #[test]
    fn default_param_meta_is_empty() {
        let meta = ParamMeta::default();
        assert!(meta.title.is_none());
        assert!(meta.description.is_none());
        assert!(!meta.deprecated);
        assert!(!meta.include_in_schema); // Default::default() sets to false
        assert!(meta.example.is_none());
        assert!(meta.ge.is_none());
        assert!(meta.le.is_none());
        assert!(meta.gt.is_none());
        assert!(meta.lt.is_none());
        assert!(meta.min_length.is_none());
        assert!(meta.max_length.is_none());
        assert!(meta.pattern.is_none());
    }

    #[test]
    fn string_constraints_together() {
        let meta = ParamMeta::new()
            .min_length(1)
            .max_length(100)
            .pattern(r"^[a-zA-Z]+$");

        assert_eq!(meta.min_length, Some(1));
        assert_eq!(meta.max_length, Some(100));
        assert_eq!(meta.pattern.as_deref(), Some(r"^[a-zA-Z]+$"));
    }

    #[test]
    fn numeric_constraints_together() {
        let meta = ParamMeta::new().gt(0.0).lt(100.0).ge(1.0).le(99.0);

        assert_eq!(meta.gt, Some(0.0));
        assert_eq!(meta.lt, Some(100.0));
        assert_eq!(meta.ge, Some(1.0));
        assert_eq!(meta.le, Some(99.0));
    }
}

// ============================================================================
// Tests for OpenAPI types serialization
// ============================================================================

#[cfg(test)]
mod serialization_tests {
    use super::*;

    #[test]
    fn parameter_serializes_location_as_in() {
        let param = Parameter {
            name: "id".to_string(),
            location: ParameterLocation::Path,
            required: true,
            schema: None,
            title: None,
            description: None,
            deprecated: false,
            example: None,
            examples: HashMap::new(),
        };

        let json = serde_json::to_string(&param).unwrap();
        assert!(json.contains(r#""in":"path""#));
    }

    #[test]
    fn parameter_location_serializes_lowercase() {
        let path_json = serde_json::to_string(&ParameterLocation::Path).unwrap();
        assert_eq!(path_json, r#""path""#);

        let query_json = serde_json::to_string(&ParameterLocation::Query).unwrap();
        assert_eq!(query_json, r#""query""#);

        let header_json = serde_json::to_string(&ParameterLocation::Header).unwrap();
        assert_eq!(header_json, r#""header""#);

        let cookie_json = serde_json::to_string(&ParameterLocation::Cookie).unwrap();
        assert_eq!(cookie_json, r#""cookie""#);
    }

    #[test]
    fn parameter_skips_false_deprecated() {
        let param = Parameter {
            name: "id".to_string(),
            location: ParameterLocation::Path,
            required: true,
            schema: None,
            title: None,
            description: None,
            deprecated: false,
            example: None,
            examples: HashMap::new(),
        };

        let json = serde_json::to_string(&param).unwrap();
        assert!(!json.contains("deprecated"));
    }

    #[test]
    fn parameter_includes_true_deprecated() {
        let param = Parameter {
            name: "old_id".to_string(),
            location: ParameterLocation::Path,
            required: true,
            schema: None,
            title: None,
            description: Some("Deprecated, use new_id instead".to_string()),
            deprecated: true,
            example: None,
            examples: HashMap::new(),
        };

        let json = serde_json::to_string(&param).unwrap();
        assert!(json.contains(r#""deprecated":true"#));
    }

    #[test]
    fn openapi_builder_creates_valid_document() {
        let doc = OpenApiBuilder::new("Test API", "1.0.0")
            .description("A test API")
            .server("https://api.example.com", Some("Production".to_string()))
            .tag("users", Some("User operations".to_string()))
            .build();

        assert_eq!(doc.openapi, "3.1.0");
        assert_eq!(doc.info.title, "Test API");
        assert_eq!(doc.info.version, "1.0.0");
        assert_eq!(doc.info.description.as_deref(), Some("A test API"));
        assert_eq!(doc.servers.len(), 1);
        assert_eq!(doc.servers[0].url, "https://api.example.com");
        assert_eq!(doc.tags.len(), 1);
        assert_eq!(doc.tags[0].name, "users");
    }

    #[test]
    fn openapi_serializes_to_valid_json() {
        let doc = OpenApiBuilder::new("Test API", "1.0.0").build();
        let json = serde_json::to_string_pretty(&doc).unwrap();

        assert!(json.contains(r#""openapi": "3.1.0""#));
        assert!(json.contains(r#""title": "Test API""#));
        assert!(json.contains(r#""version": "1.0.0""#));
    }

    #[test]
    fn example_serializes_all_fields() {
        let example = Example {
            summary: Some("Example summary".to_string()),
            description: Some("Example description".to_string()),
            value: Some(serde_json::json!({"key": "value"})),
            external_value: None,
        };

        let json = serde_json::to_string(&example).unwrap();
        assert!(json.contains(r#""summary":"Example summary""#));
        assert!(json.contains(r#""description":"Example description""#));
        assert!(json.contains(r#""value""#));
    }
}

/// OpenAPI document builder.
pub struct OpenApiBuilder {
    info: Info,
    servers: Vec<Server>,
    paths: HashMap<String, PathItem>,
    components: Components,
    tags: Vec<Tag>,
}

impl OpenApiBuilder {
    /// Create a new builder.
    #[must_use]
    pub fn new(title: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            info: Info {
                title: title.into(),
                version: version.into(),
                description: None,
                terms_of_service: None,
                contact: None,
                license: None,
            },
            servers: Vec::new(),
            paths: HashMap::new(),
            components: Components::default(),
            tags: Vec::new(),
        }
    }

    /// Add a description.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.info.description = Some(description.into());
        self
    }

    /// Add a server.
    #[must_use]
    pub fn server(mut self, url: impl Into<String>, description: Option<String>) -> Self {
        self.servers.push(Server {
            url: url.into(),
            description,
        });
        self
    }

    /// Add a tag.
    #[must_use]
    pub fn tag(mut self, name: impl Into<String>, description: Option<String>) -> Self {
        self.tags.push(Tag {
            name: name.into(),
            description,
        });
        self
    }

    /// Add a schema component.
    #[must_use]
    pub fn schema(mut self, name: impl Into<String>, schema: Schema) -> Self {
        self.components.schemas.insert(name.into(), schema);
        self
    }

    /// Build the OpenAPI document.
    #[must_use]
    pub fn build(self) -> OpenApi {
        OpenApi {
            openapi: "3.1.0".to_string(),
            info: self.info,
            servers: self.servers,
            paths: self.paths,
            components: if self.components.schemas.is_empty() {
                None
            } else {
                Some(self.components)
            },
            tags: self.tags,
        }
    }
}
