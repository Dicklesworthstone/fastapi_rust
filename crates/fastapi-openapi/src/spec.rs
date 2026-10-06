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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Operation {
    /// Operation ID.
    #[serde(
        rename = "operationId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
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
    #[serde(
        rename = "requestBody",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub request_body: Option<RequestBody>,
    /// Responses.
    pub responses: HashMap<String, Response>,
    /// Alternative security requirements (OR), with schemes inside each object combined as AND.
    /// An empty requirement object allows anonymous access.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security: Vec<SecurityRequirement>,
    /// Deprecated flag.
    #[serde(default, skip_serializing_if = "is_false")]
    pub deprecated: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Create default 200 OK response map.
fn default_responses() -> HashMap<String, Response> {
    let mut responses = HashMap::new();
    responses.insert(
        "200".to_string(),
        Response {
            description: "Successful response".to_string(),
            content: HashMap::new(),
        },
    );
    responses
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
    /// Named security scheme definitions used by operation requirements.
    #[serde(
        rename = "securitySchemes",
        default,
        skip_serializing_if = "HashMap::is_empty"
    )]
    pub security_schemes: HashMap<String, SecurityScheme>,
}

/// One security requirement object: all named schemes must be satisfied.
///
/// Entries in an operation's requirement array are alternatives. An empty
/// object represents anonymous access. These definitions document security;
/// they do not validate credentials or enforce authorization.
pub type SecurityRequirement = HashMap<String, Vec<String>>;

/// A location supported by an OpenAPI API key security scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeyLocation {
    /// HTTP header.
    Header,
    /// Query string parameter.
    Query,
    /// Cookie value.
    Cookie,
}

/// Supported OpenAPI security scheme definitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SecurityScheme {
    /// HTTP authentication, including Basic and Bearer schemes.
    #[serde(rename = "http")]
    Http {
        /// HTTP authentication scheme name.
        scheme: String,
        /// Optional documentation hint for the bearer token format.
        #[serde(
            rename = "bearerFormat",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        bearer_format: Option<String>,
        /// Scheme description.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// API key carried in a header, query parameter or cookie.
    #[serde(rename = "apiKey")]
    ApiKey {
        /// Name of the header, query parameter or cookie.
        name: String,
        /// Location carrying the API key.
        #[serde(rename = "in")]
        location: ApiKeyLocation,
        /// Scheme description.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// OAuth2 password-flow documentation.
    #[serde(rename = "oauth2")]
    OAuth2 {
        /// Supported OAuth2 flow definitions.
        flows: OAuthFlows,
        /// Scheme description.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
}

impl SecurityScheme {
    /// Describe HTTP Basic authentication.
    #[must_use]
    pub fn basic() -> Self {
        Self::Http {
            scheme: "basic".to_string(),
            bearer_format: None,
            description: None,
        }
    }

    /// Describe HTTP Bearer authentication without assuming a token format.
    #[must_use]
    pub fn bearer() -> Self {
        Self::Http {
            scheme: "bearer".to_string(),
            bearer_format: None,
            description: None,
        }
    }

    /// Describe an OAuth2 password flow with no scopes or refresh URL.
    ///
    /// This declares metadata only; it does not create a token endpoint.
    #[must_use]
    pub fn password(token_url: impl Into<String>) -> Self {
        Self::OAuth2 {
            flows: OAuthFlows {
                password: OAuthPasswordFlow {
                    token_url: token_url.into(),
                    refresh_url: None,
                    scopes: HashMap::new(),
                },
            },
            description: None,
        }
    }
}

/// OAuth2 flows supported by this model: the password flow only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthFlows {
    /// Password grant flow.
    pub password: OAuthPasswordFlow,
}

/// OpenAPI metadata for an OAuth2 password flow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthPasswordFlow {
    /// Required token endpoint URL.
    #[serde(rename = "tokenUrl")]
    pub token_url: String,
    /// Optional refresh endpoint URL.
    #[serde(
        rename = "refreshUrl",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub refresh_url: Option<String>,
    /// Available scopes and their descriptions, including an explicit empty map.
    pub scopes: HashMap<String, String>,
}

/// Registration-time extractor metadata; this is not a serialized OpenAPI object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityMetadata {
    /// Component name referenced by this extractor.
    pub name: String,
    /// Corresponding scheme definition.
    pub scheme: SecurityScheme,
    /// Required OAuth2 scopes, or an empty list for unscoped authentication.
    pub scopes: Vec<String>,
    /// Whether this extractor requires successful authentication extraction.
    pub required: bool,
}

impl SecurityMetadata {
    /// Create required extractor metadata with no required scopes.
    #[must_use]
    pub fn new(name: impl Into<String>, scheme: SecurityScheme) -> Self {
        Self {
            name: name.into(),
            scheme,
            scopes: Vec::new(),
            required: true,
        }
    }
}

/// Schema registry for `#/components/schemas`.
///
/// This owns a schema map and provides `register()` helpers that return `$ref`s.
#[derive(Debug, Default)]
pub struct SchemaRegistry {
    schemas: HashMap<String, Schema>,
}

impl SchemaRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            schemas: HashMap::new(),
        }
    }

    /// Register `schema` under `name` if it doesn't already exist, and return a `$ref`.
    ///
    /// This does **not** overwrite an existing entry, which enables stable deduplication.
    pub fn register(&mut self, name: impl Into<String>, schema: Schema) -> Schema {
        let name = name.into();
        self.schemas.entry(name.clone()).or_insert(schema);
        Schema::reference(&name)
    }

    /// Consume the registry and return the underlying schema map.
    #[must_use]
    pub fn into_schemas(self) -> HashMap<String, Schema> {
        self.schemas
    }
}

/// A mutable view into an existing component schema map.
pub struct SchemaRegistryMut<'a> {
    schemas: &'a mut HashMap<String, Schema>,
}

impl SchemaRegistryMut<'_> {
    /// Register `schema` under `name` if it doesn't already exist, and return a `$ref`.
    pub fn register(&mut self, name: impl Into<String>, schema: Schema) -> Schema {
        let name = name.into();
        self.schemas.entry(name.clone()).or_insert(schema);
        Schema::reference(&name)
    }
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

    #[test]
    fn http_security_schemes_use_openapi_wire_names_and_round_trip() {
        for (scheme, expected) in [
            (
                SecurityScheme::basic(),
                serde_json::json!({"type": "http", "scheme": "basic"}),
            ),
            (
                SecurityScheme::bearer(),
                serde_json::json!({"type": "http", "scheme": "bearer"}),
            ),
            (
                SecurityScheme::Http {
                    scheme: "bearer".to_string(),
                    bearer_format: Some("JWT".to_string()),
                    description: Some("Caller-supplied access token".to_string()),
                },
                serde_json::json!({
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "JWT",
                    "description": "Caller-supplied access token"
                }),
            ),
        ] {
            let json = serde_json::to_value(&scheme).expect("HTTP scheme serialization");
            assert_eq!(json, expected);
            let decoded: SecurityScheme =
                serde_json::from_value(json).expect("HTTP scheme deserialization");
            assert_eq!(decoded, scheme);
        }
    }

    #[test]
    fn api_key_security_schemes_accept_only_header_query_and_cookie_locations() {
        for (location, wire_location) in [
            (ApiKeyLocation::Header, "header"),
            (ApiKeyLocation::Query, "query"),
            (ApiKeyLocation::Cookie, "cookie"),
        ] {
            let scheme = SecurityScheme::ApiKey {
                name: "api_key".to_string(),
                location,
                description: None,
            };
            let json = serde_json::to_value(&scheme).expect("API key scheme serialization");
            assert_eq!(
                json,
                serde_json::json!({"type": "apiKey", "name": "api_key", "in": wire_location})
            );
            let decoded: SecurityScheme =
                serde_json::from_value(json).expect("API key scheme deserialization");
            assert_eq!(decoded, scheme);
        }
        assert!(
            serde_json::from_value::<SecurityScheme>(serde_json::json!({
                "type": "apiKey", "name": "api_key", "in": "path"
            }))
            .is_err()
        );
    }

    #[test]
    fn oauth_password_flow_retains_empty_scopes_and_required_token_url() {
        let scheme = SecurityScheme::password("/token");
        let json = serde_json::to_value(&scheme).expect("OAuth2 scheme serialization");
        assert_eq!(
            json,
            serde_json::json!({
                "type": "oauth2",
                "flows": {"password": {"tokenUrl": "/token", "scopes": {}}}
            })
        );
        let decoded: SecurityScheme =
            serde_json::from_value(json).expect("OAuth2 scheme deserialization");
        assert_eq!(decoded, scheme);
        for invalid in [
            serde_json::json!({"type": "oauth2", "flows": {"password": {"scopes": {}}}}),
            serde_json::json!({"type": "oauth2", "flows": {"password": {"tokenUrl": "/token"}}}),
        ] {
            assert!(serde_json::from_value::<SecurityScheme>(invalid).is_err());
        }
    }

    #[test]
    fn oauth_password_flow_round_trips_scopes_refresh_url_and_description() {
        let scheme = SecurityScheme::OAuth2 {
            flows: OAuthFlows {
                password: OAuthPasswordFlow {
                    token_url: "/auth/token".to_string(),
                    refresh_url: Some("/auth/refresh".to_string()),
                    scopes: HashMap::from([("read:items".to_string(), "Read items".to_string())]),
                },
            },
            description: Some("Item access".to_string()),
        };
        let json = serde_json::to_value(&scheme).expect("OAuth2 scheme serialization");
        assert_eq!(
            json,
            serde_json::json!({
                "type": "oauth2",
                "description": "Item access",
                "flows": {"password": {
                    "tokenUrl": "/auth/token",
                    "refreshUrl": "/auth/refresh",
                    "scopes": {"read:items": "Read items"}
                }}
            })
        );
        let decoded: SecurityScheme =
            serde_json::from_value(json).expect("OAuth2 scheme deserialization");
        assert_eq!(decoded, scheme);
    }

    #[test]
    fn operation_security_preserves_conjunctions_alternatives_and_anonymous_access() {
        let operation = Operation {
            security: vec![
                HashMap::from([
                    ("AccessToken".to_string(), Vec::new()),
                    ("ApiKey".to_string(), Vec::new()),
                ]),
                HashMap::from([("PasswordAuth".to_string(), vec!["read:items".to_string()])]),
                HashMap::new(),
            ],
            ..Default::default()
        };
        let json = serde_json::to_value(&operation).expect("operation serialization");
        assert_eq!(
            json["security"],
            serde_json::json!([
                {"AccessToken": [], "ApiKey": []},
                {"PasswordAuth": ["read:items"]},
                {}
            ])
        );
        let decoded: Operation = serde_json::from_value(json).expect("operation deserialization");
        assert_eq!(decoded.security, operation.security);
        let unprotected = serde_json::to_value(Operation::default()).expect("default operation");
        assert!(unprotected.get("security").is_none());
    }

    #[test]
    fn scheme_only_components_survive_build_and_round_trip() {
        let document = OpenApiBuilder::new("Security API", "1")
            .security_scheme("AccessToken", SecurityScheme::bearer())
            .build();
        let components = document
            .components
            .as_ref()
            .expect("scheme-only components");
        assert!(components.schemas.is_empty());
        assert_eq!(components.security_schemes.len(), 1);
        let json = serde_json::to_value(&document).expect("document serialization");
        assert_eq!(
            json["components"]["securitySchemes"]["AccessToken"],
            serde_json::json!({"type": "http", "scheme": "bearer"})
        );
        assert!(json["components"].get("schemas").is_none());
        assert!(json["components"].get("security_schemes").is_none());
        let decoded: OpenApi = serde_json::from_value(json).expect("document deserialization");
        assert_eq!(
            decoded
                .components
                .expect("decoded components")
                .security_schemes,
            components.security_schemes
        );
        assert!(
            OpenApiBuilder::new("Public API", "1")
                .build()
                .components
                .is_none()
        );
    }

    #[test]
    fn duplicate_identical_security_schemes_preserve_existing_schema_components() {
        let document = OpenApiBuilder::new("Security API", "1")
            .schema("Item", Schema::integer(Some("int64")))
            .security_scheme("AccessToken", SecurityScheme::bearer())
            .security_scheme("AccessToken", SecurityScheme::bearer())
            .build();
        let components = document.components.expect("schema and security components");
        assert_eq!(components.schemas.len(), 1);
        assert!(components.schemas.contains_key("Item"));
        assert_eq!(components.security_schemes.len(), 1);
        assert_eq!(
            components.security_schemes["AccessToken"],
            SecurityScheme::bearer()
        );
    }

    #[test]
    #[should_panic(expected = "conflicting security scheme definition for 'AccessToken'")]
    fn conflicting_security_scheme_definitions_are_rejected() {
        let _builder = OpenApiBuilder::new("Security API", "1")
            .security_scheme("AccessToken", SecurityScheme::bearer())
            .security_scheme("AccessToken", SecurityScheme::basic());
    }

    #[test]
    fn route_security_preserves_manual_alternatives_and_scopes() {
        let route = fastapi_router::Route::new(fastapi_types::Method::Get, "/protected")
            .security("PasswordAuth", ["read:items"])
            .security_scheme("ApiKey");
        let mut builder = OpenApiBuilder::new("Security API", "1")
            .security_scheme(
                "PasswordAuth",
                SecurityScheme::OAuth2 {
                    flows: OAuthFlows {
                        password: OAuthPasswordFlow {
                            token_url: "/token".to_string(),
                            refresh_url: None,
                            scopes: HashMap::from([(
                                "read:items".to_string(),
                                "Read items".to_string(),
                            )]),
                        },
                    },
                    description: None,
                },
            )
            .security_scheme(
                "ApiKey",
                SecurityScheme::ApiKey {
                    name: "X-Api-Key".to_string(),
                    location: ApiKeyLocation::Header,
                    description: None,
                },
            );
        let operation = builder.add_route(&route).expect("GET operation");
        assert_eq!(operation.security.len(), 2);
        assert_eq!(operation.security[0].len(), 1);
        assert_eq!(operation.security[0]["PasswordAuth"], ["read:items"]);
        assert_eq!(operation.security[1].len(), 1);
        assert!(operation.security[1]["ApiKey"].is_empty());
        let json = serde_json::to_value(builder.build()).expect("secured document");
        assert_eq!(
            json["paths"]["/protected"]["get"]["security"],
            serde_json::json!([{"PasswordAuth": ["read:items"]}, {"ApiKey": []}])
        );
    }

    #[test]
    #[should_panic(
        expected = "security requirement 'MissingScheme' has no registered security scheme"
    )]
    fn build_rejects_unregistered_manual_security_requirement_names() {
        let route = fastapi_router::Route::new(fastapi_types::Method::Get, "/protected")
            .security_scheme("MissingScheme");
        let mut builder = OpenApiBuilder::new("Security API", "1");
        builder.add_route(&route).expect("GET operation");
        let _document = builder.build();
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

    /// Register a named security scheme, deduplicating identical definitions.
    ///
    /// Names are explicit; arbitrary route requirement names do not infer schemes.
    ///
    /// # Panics
    ///
    /// Panics during registration if the name already has a different definition.
    #[must_use]
    pub fn security_scheme(mut self, name: impl Into<String>, scheme: SecurityScheme) -> Self {
        match self.components.security_schemes.entry(name.into()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(scheme);
            }
            std::collections::hash_map::Entry::Occupied(entry) => {
                assert_eq!(
                    entry.get(),
                    &scheme,
                    "conflicting security scheme definition for '{}'",
                    entry.key()
                );
            }
        }
        self
    }

    /// Access the component schema registry for in-place registration.
    pub fn registry(&mut self) -> SchemaRegistryMut<'_> {
        SchemaRegistryMut {
            schemas: &mut self.components.schemas,
        }
    }

    /// Add a metadata-rich route (from `fastapi-router`) as an OpenAPI operation.
    ///
    /// This is a convenience bridge used by integration tests and by higher-level crates.
    #[allow(clippy::too_many_lines)]
    pub fn add_route(&mut self, route: &fastapi_router::Route) -> Option<&mut Operation> {
        use fastapi_router::Converter as RouteConverter;

        fn param_schema(conv: RouteConverter) -> Schema {
            match conv {
                RouteConverter::Str | RouteConverter::Path => Schema::string(),
                RouteConverter::Int => Schema::integer(Some("int64")),
                RouteConverter::Float => Schema::number(Some("double")),
                RouteConverter::Uuid => Schema::Primitive(crate::schema::PrimitiveSchema {
                    schema_type: crate::schema::SchemaType::String,
                    format: Some("uuid".to_string()),
                    nullable: false,
                }),
            }
        }

        let mut op = Operation {
            operation_id: if route.operation_id.is_empty() {
                None
            } else {
                Some(route.operation_id.clone())
            },
            summary: route.summary.clone(),
            description: route.description.clone(),
            tags: route.tags.clone(),
            deprecated: route.deprecated,
            security: route
                .security
                .iter()
                .map(|requirement| {
                    HashMap::from([(requirement.scheme.clone(), requirement.scopes.clone())])
                })
                .collect(),
            ..Default::default()
        };

        // Path parameters.
        for p in &route.path_params {
            let mut examples = HashMap::new();
            for (name, value) in &p.examples {
                examples.insert(
                    name.clone(),
                    Example {
                        summary: None,
                        description: None,
                        value: Some(value.clone()),
                        external_value: None,
                    },
                );
            }

            op.parameters.push(Parameter {
                name: p.name.clone(),
                location: ParameterLocation::Path,
                required: true,
                schema: Some(param_schema(p.converter)),
                title: p.title.clone(),
                description: p.description.clone(),
                deprecated: p.deprecated,
                example: p.example.clone(),
                examples,
            });
        }

        // Request body.
        if let Some(schema_name) = &route.request_body_schema {
            let content_type = route
                .request_body_content_type
                .clone()
                .unwrap_or_else(|| "application/json".to_string());
            let mut content = HashMap::new();
            content.insert(
                content_type,
                MediaType {
                    schema: Some(Schema::reference(schema_name)),
                },
            );
            op.request_body = Some(RequestBody {
                required: route.request_body_required,
                content,
                description: None,
            });
        }

        // Responses.
        let mut responses = HashMap::new();
        if route.responses.is_empty() {
            responses = default_responses();
        } else {
            for r in &route.responses {
                let mut content = HashMap::new();
                content.insert(
                    "application/json".to_string(),
                    MediaType {
                        schema: Some(Schema::reference(&r.schema_name)),
                    },
                );
                responses.insert(
                    r.status.to_string(),
                    Response {
                        description: r.description.clone(),
                        content,
                    },
                );
            }
        }
        op.responses = responses;

        // Converter suffixes belong to the router, not OpenAPI path templates.
        let mut path = route.path.clone();
        for parameter in &route.path_params {
            path = path.replace(
                &format!("{{*{}}}", parameter.name),
                &format!("{{{}}}", parameter.name),
            );
            if let Some(start) = path.find(&format!("{{{}:", parameter.name))
                && let Some(end) = path[start..].find('}')
            {
                path.replace_range(start..=start + end, &format!("{{{}}}", parameter.name));
            }
        }
        let path_item = self.paths.entry(path).or_default();
        let slot = match route.method.as_str() {
            "GET" => &mut path_item.get,
            "POST" => &mut path_item.post,
            "PUT" => &mut path_item.put,
            "DELETE" => &mut path_item.delete,
            "PATCH" => &mut path_item.patch,
            "OPTIONS" => &mut path_item.options,
            "HEAD" => &mut path_item.head,
            _ => return None,
        };
        Some(slot.insert(op))
    }

    /// Add multiple routes.
    pub fn add_routes<'a, I>(&mut self, routes: I)
    where
        I: IntoIterator<Item = &'a fastapi_router::Route>,
    {
        for r in routes {
            self.add_route(r);
        }
    }

    /// Add a path operation (GET, POST, etc.).
    ///
    /// This is the primary method for registering routes with OpenAPI.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let builder = OpenApiBuilder::new("My API", "1.0.0")
    ///     .operation("GET", "/users", Operation {
    ///         operation_id: Some("get_users".to_string()),
    ///         summary: Some("List all users".to_string()),
    ///         responses: HashMap::from([
    ///             ("200".to_string(), Response {
    ///                 description: "Success".to_string(),
    ///                 content: HashMap::new(),
    ///             })
    ///         ]),
    ///         ..Default::default()
    ///     });
    /// ```
    #[must_use]
    pub fn operation(
        mut self,
        method: &str,
        path: impl Into<String>,
        operation: Operation,
    ) -> Self {
        let path = path.into();
        let path_item = self.paths.entry(path).or_default();

        match method.to_uppercase().as_str() {
            "GET" => path_item.get = Some(operation),
            "POST" => path_item.post = Some(operation),
            "PUT" => path_item.put = Some(operation),
            "DELETE" => path_item.delete = Some(operation),
            "PATCH" => path_item.patch = Some(operation),
            "OPTIONS" => path_item.options = Some(operation),
            "HEAD" => path_item.head = Some(operation),
            _ => {} // Ignore unknown methods
        }

        self
    }

    /// Add a simple GET endpoint with default 200 response.
    #[must_use]
    pub fn get(self, path: impl Into<String>, operation_id: impl Into<String>) -> Self {
        let operation_id = operation_id.into();
        self.operation(
            "GET",
            path,
            Operation {
                operation_id: if operation_id.is_empty() {
                    None
                } else {
                    Some(operation_id)
                },
                responses: default_responses(),
                ..Default::default()
            },
        )
    }

    /// Add a simple POST endpoint with default 200 response.
    #[must_use]
    pub fn post(self, path: impl Into<String>, operation_id: impl Into<String>) -> Self {
        let operation_id = operation_id.into();
        self.operation(
            "POST",
            path,
            Operation {
                operation_id: if operation_id.is_empty() {
                    None
                } else {
                    Some(operation_id)
                },
                responses: default_responses(),
                ..Default::default()
            },
        )
    }

    /// Add a simple PUT endpoint with default 200 response.
    #[must_use]
    pub fn put(self, path: impl Into<String>, operation_id: impl Into<String>) -> Self {
        let operation_id = operation_id.into();
        self.operation(
            "PUT",
            path,
            Operation {
                operation_id: if operation_id.is_empty() {
                    None
                } else {
                    Some(operation_id)
                },
                responses: default_responses(),
                ..Default::default()
            },
        )
    }

    /// Add a simple DELETE endpoint with default 200 response.
    #[must_use]
    pub fn delete(self, path: impl Into<String>, operation_id: impl Into<String>) -> Self {
        let operation_id = operation_id.into();
        self.operation(
            "DELETE",
            path,
            Operation {
                operation_id: if operation_id.is_empty() {
                    None
                } else {
                    Some(operation_id)
                },
                responses: default_responses(),
                ..Default::default()
            },
        )
    }

    /// Add a simple PATCH endpoint with default 200 response.
    #[must_use]
    pub fn patch(self, path: impl Into<String>, operation_id: impl Into<String>) -> Self {
        let operation_id = operation_id.into();
        self.operation(
            "PATCH",
            path,
            Operation {
                operation_id: if operation_id.is_empty() {
                    None
                } else {
                    Some(operation_id)
                },
                responses: default_responses(),
                ..Default::default()
            },
        )
    }

    /// Build the OpenAPI document.
    ///
    /// # Panics
    ///
    /// Panics if an operation references an unregistered security scheme.
    #[must_use]
    pub fn build(self) -> OpenApi {
        for item in self.paths.values() {
            for operation in [
                &item.get,
                &item.post,
                &item.put,
                &item.delete,
                &item.patch,
                &item.options,
                &item.head,
            ]
            .into_iter()
            .flatten()
            {
                for requirement in &operation.security {
                    for name in requirement.keys() {
                        assert!(
                            self.components.security_schemes.contains_key(name),
                            "security requirement '{}' has no registered security scheme",
                            name
                        );
                    }
                }
            }
        }
        OpenApi {
            openapi: "3.1.0".to_string(),
            info: self.info,
            servers: self.servers,
            paths: self.paths,
            components: if self.components.schemas.is_empty()
                && self.components.security_schemes.is_empty()
            {
                None
            } else {
                Some(self.components)
            },
            tags: self.tags,
        }
    }
}
