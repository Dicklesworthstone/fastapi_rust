//! OpenAPI 3.1 specification types.

use crate::schema::{Schema, SchemaRegistry};
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
    /// Security requirements applied to all operations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security: Vec<SecurityRequirement>,
}

/// Security requirement object.
///
/// Each key is a security scheme name, and the value is a list of scopes
/// required for that scheme (empty for schemes that don't use scopes).
pub type SecurityRequirement = HashMap<String, Vec<String>>;

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
#[serde(rename_all = "camelCase")]
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
    /// Security requirements for this operation.
    ///
    /// Overrides the top-level security requirements when specified.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security: Vec<SecurityRequirement>,
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

/// Security scheme definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
#[allow(clippy::large_enum_variant)] // OAuth2 variant is large but rarely copied
pub enum SecurityScheme {
    /// API key authentication.
    #[serde(rename = "apiKey")]
    ApiKey {
        /// Parameter name.
        name: String,
        /// Location of the API key.
        #[serde(rename = "in")]
        location: ApiKeyLocation,
        /// Description.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// HTTP authentication (Basic, Bearer, etc.).
    #[serde(rename = "http")]
    Http {
        /// Authentication scheme (e.g., "basic", "bearer").
        scheme: String,
        /// Bearer token format (e.g., "JWT").
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "bearerFormat"
        )]
        bearer_format: Option<String>,
        /// Description.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// OAuth 2.0 authentication.
    #[serde(rename = "oauth2")]
    OAuth2 {
        /// OAuth 2.0 flows.
        flows: OAuth2Flows,
        /// Description.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// OpenID Connect authentication.
    #[serde(rename = "openIdConnect")]
    OpenIdConnect {
        /// OpenID Connect discovery URL.
        #[serde(rename = "openIdConnectUrl")]
        open_id_connect_url: String,
        /// Description.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
}

/// Location of an API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeyLocation {
    /// API key in query parameter.
    Query,
    /// API key in header.
    Header,
    /// API key in cookie.
    Cookie,
}

/// OAuth 2.0 flow configurations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuth2Flows {
    /// Implicit flow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implicit: Option<OAuth2Flow>,
    /// Authorization code flow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_code: Option<OAuth2Flow>,
    /// Client credentials flow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_credentials: Option<OAuth2Flow>,
    /// Resource owner password flow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<OAuth2Flow>,
}

/// OAuth 2.0 flow configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuth2Flow {
    /// Authorization URL (for implicit and authorization_code flows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_url: Option<String>,
    /// Token URL (for password, client_credentials, and authorization_code flows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_url: Option<String>,
    /// Refresh URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_url: Option<String>,
    /// Available scopes.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub scopes: HashMap<String, String>,
}

/// Reusable components.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Components {
    /// Schema definitions.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub schemas: HashMap<String, Schema>,
    /// Security scheme definitions.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub security_schemes: HashMap<String, SecurityScheme>,
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
    fn openapi_builder_with_registry_includes_schemas() {
        use crate::schema::Schema;

        let builder = OpenApiBuilder::new("Test API", "1.0.0");

        // Register schemas via the registry
        builder.registry().register(
            "User",
            Schema::object(
                [
                    ("id".to_string(), Schema::integer(Some("int64"))),
                    ("name".to_string(), Schema::string()),
                ]
                .into_iter()
                .collect(),
                vec!["id".to_string(), "name".to_string()],
            ),
        );

        let doc = builder.build();

        // Components should include the registered schema
        assert!(doc.components.is_some());
        let components = doc.components.unwrap();
        assert!(components.schemas.contains_key("User"));
    }

    #[test]
    fn openapi_builder_registry_returns_refs() {
        use crate::schema::Schema;

        let builder = OpenApiBuilder::new("Test API", "1.0.0");

        // Register and get $ref back
        let user_ref = builder.registry().register("User", Schema::string());

        if let Schema::Ref(ref_schema) = user_ref {
            assert_eq!(ref_schema.reference, "#/components/schemas/User");
        } else {
            panic!("Expected Schema::Ref");
        }
    }

    #[test]
    fn openapi_builder_merges_registry_and_explicit_schemas() {
        use crate::schema::Schema;

        let builder =
            OpenApiBuilder::new("Test API", "1.0.0").schema("ExplicitSchema", Schema::boolean());

        // Also register via registry
        builder
            .registry()
            .register("RegistrySchema", Schema::string());

        let doc = builder.build();

        let components = doc.components.unwrap();
        assert!(components.schemas.contains_key("ExplicitSchema"));
        assert!(components.schemas.contains_key("RegistrySchema"));
    }

    #[test]
    fn openapi_builder_explicit_schemas_override_registry() {
        use crate::schema::Schema;

        let builder = OpenApiBuilder::new("Test API", "1.0.0");

        // Register via registry first
        builder.registry().register("MyType", Schema::string());

        // Then add explicitly (should override)
        let builder = builder.schema("MyType", Schema::boolean());

        let doc = builder.build();
        let components = doc.components.unwrap();

        // Should be boolean (explicit), not string (registry)
        if let Schema::Primitive(p) = &components.schemas["MyType"] {
            assert!(matches!(p.schema_type, crate::schema::SchemaType::Boolean));
        } else {
            panic!("Expected primitive boolean schema");
        }
    }

    #[test]
    fn openapi_builder_with_existing_registry() {
        use crate::schema::Schema;

        // Pre-populate a registry
        let registry = SchemaRegistry::new();
        registry.register("PreRegistered", Schema::string());

        // Use the pre-populated registry
        let builder = OpenApiBuilder::with_registry("Test API", "1.0.0", registry);

        let doc = builder.build();
        let components = doc.components.unwrap();
        assert!(components.schemas.contains_key("PreRegistered"));
    }

    #[test]
    fn openapi_builder_registry_serializes_refs_correctly() {
        use crate::schema::Schema;

        let builder = OpenApiBuilder::new("Test API", "1.0.0");

        // Register User schema
        let user_ref = builder.registry().register(
            "User",
            Schema::object(
                [("name".to_string(), Schema::string())]
                    .into_iter()
                    .collect(),
                vec!["name".to_string()],
            ),
        );

        // Create a response that uses the $ref
        let doc = builder.build();
        let json = serde_json::to_string_pretty(&doc).unwrap();

        // Should have components/schemas/User
        assert!(json.contains(r#""User""#));

        // The user_ref should serialize as a $ref
        let ref_json = serde_json::to_string(&user_ref).unwrap();
        assert!(ref_json.contains(r##""$ref":"#/components/schemas/User""##));
    }
}

/// OpenAPI document builder.
pub struct OpenApiBuilder {
    info: Info,
    servers: Vec<Server>,
    paths: HashMap<String, PathItem>,
    components: Components,
    tags: Vec<Tag>,
    /// Global security requirements.
    security: Vec<SecurityRequirement>,
    /// Schema registry for collecting and deduplicating schemas.
    registry: SchemaRegistry,
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
            security: Vec::new(),
            registry: SchemaRegistry::new(),
        }
    }

    /// Create a new builder with an existing schema registry.
    ///
    /// Use this when you want to share schemas across multiple OpenAPI documents
    /// or when you've pre-registered schemas.
    #[must_use]
    pub fn with_registry(
        title: impl Into<String>,
        version: impl Into<String>,
        registry: SchemaRegistry,
    ) -> Self {
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
            security: Vec::new(),
            registry,
        }
    }

    /// Get a reference to the schema registry.
    ///
    /// Use this to register schemas that should be in `#/components/schemas/`
    /// and get `$ref` references to them.
    #[must_use]
    pub fn registry(&self) -> &SchemaRegistry {
        &self.registry
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

    /// Add a security scheme.
    ///
    /// Security schemes define authentication methods used by the API.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi_openapi::{OpenApiBuilder, SecurityScheme, ApiKeyLocation};
    ///
    /// let doc = OpenApiBuilder::new("My API", "1.0.0")
    ///     .security_scheme("api_key", SecurityScheme::ApiKey {
    ///         name: "X-API-Key".to_string(),
    ///         location: ApiKeyLocation::Header,
    ///         description: Some("API key for authentication".to_string()),
    ///     })
    ///     .security_scheme("bearer", SecurityScheme::Http {
    ///         scheme: "bearer".to_string(),
    ///         bearer_format: Some("JWT".to_string()),
    ///         description: None,
    ///     })
    ///     .build();
    /// ```
    #[must_use]
    pub fn security_scheme(mut self, name: impl Into<String>, scheme: SecurityScheme) -> Self {
        self.components.security_schemes.insert(name.into(), scheme);
        self
    }

    /// Add a global security requirement.
    ///
    /// Global security requirements apply to all operations unless overridden
    /// at the operation level.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi_openapi::OpenApiBuilder;
    ///
    /// let doc = OpenApiBuilder::new("My API", "1.0.0")
    ///     .security_scheme("api_key", /* ... */)
    ///     .security_requirement("api_key", vec![])  // No scopes needed for API key
    ///     .build();
    /// ```
    #[must_use]
    pub fn security_requirement(
        mut self,
        scheme: impl Into<String>,
        scopes: Vec<String>,
    ) -> Self {
        let mut req = SecurityRequirement::new();
        req.insert(scheme.into(), scopes);
        self.security.push(req);
        self
    }

    /// Add a route to the OpenAPI document.
    ///
    /// Converts a route's metadata into an OpenAPI Operation and adds it
    /// to the appropriate path. Multiple routes on the same path with
    /// different methods are merged into a single PathItem.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use fastapi_openapi::OpenApiBuilder;
    /// use fastapi_router::Router;
    ///
    /// let router = Router::new();
    /// // ... add routes to router ...
    ///
    /// let mut builder = OpenApiBuilder::new("My API", "1.0.0");
    /// for route in router.routes() {
    ///     builder.add_route(route);
    /// }
    /// let doc = builder.build();
    /// ```
    pub fn add_route(&mut self, route: &Route) {
        let operation = self.route_to_operation(route);
        let path_item = self.paths.entry(route.path.clone()).or_default();

        match route.method {
            Method::Get => path_item.get = Some(operation),
            Method::Post => path_item.post = Some(operation),
            Method::Put => path_item.put = Some(operation),
            Method::Delete => path_item.delete = Some(operation),
            Method::Patch => path_item.patch = Some(operation),
            Method::Options => path_item.options = Some(operation),
            Method::Head => path_item.head = Some(operation),
            Method::Trace => {
                // OpenAPI PathItem doesn't have a trace field by default,
                // but we could add it. For now, skip TRACE methods.
            }
        }
    }

    /// Add multiple routes to the OpenAPI document.
    ///
    /// Convenience method that calls `add_route` for each route.
    pub fn add_routes(&mut self, routes: &[Route]) {
        for route in routes {
            self.add_route(route);
        }
    }

    /// Convert a Route to an OpenAPI Operation.
    #[allow(clippy::unused_self)] // Will use self.registry for schema lookups in future
    fn route_to_operation(&self, route: &Route) -> Operation {
        // Convert path parameters
        let parameters: Vec<Parameter> = route
            .path_params
            .iter()
            .map(param_info_to_parameter)
            .collect();

        // Build request body if present
        let request_body = route.request_body_schema.as_ref().map(|schema_name| {
            let content_type = route
                .request_body_content_type
                .as_deref()
                .unwrap_or("application/json");

            let mut content = HashMap::new();
            content.insert(
                content_type.to_string(),
                MediaType {
                    schema: Some(Schema::reference(schema_name)),
                },
            );

            RequestBody {
                required: route.request_body_required,
                content,
                description: None,
            }
        });

        // Build a default 200 response (can be extended later with response metadata)
        let mut responses = HashMap::new();
        responses.insert(
            "200".to_string(),
            Response {
                description: "Successful response".to_string(),
                content: HashMap::new(),
            },
        );

        Operation {
            operation_id: if route.operation_id.is_empty() {
                None
            } else {
                Some(route.operation_id.clone())
            },
            summary: route.summary.clone(),
            description: route.description.clone(),
            tags: route.tags.clone(),
            parameters,
            request_body,
            responses,
            deprecated: route.deprecated,
            security: Vec::new(),
        }
    }

    /// Build the OpenAPI document.
    ///
    /// This merges all schemas from the registry into `components.schemas`.
    #[must_use]
    pub fn build(self) -> OpenApi {
        // Merge registry schemas with explicitly added schemas
        let mut all_schemas = self.registry.into_schemas();
        for (name, schema) in self.components.schemas {
            // Explicitly added schemas take precedence
            all_schemas.insert(name, schema);
        }

        OpenApi {
            openapi: "3.1.0".to_string(),
            info: self.info,
            servers: self.servers,
            paths: self.paths,
            components: if all_schemas.is_empty() && self.components.security_schemes.is_empty() {
                None
            } else {
                Some(Components {
                    schemas: all_schemas,
                    security_schemes: self.components.security_schemes,
                })
            },
            tags: self.tags,
            security: self.security,
        }
    }
}

// ============================================================================
// Path Parameter Generation
// ============================================================================

use fastapi_core::Method;
use fastapi_router::{Converter, ParamInfo, Route, extract_path_params};

/// Convert a router `Converter` type to an OpenAPI `Schema`.
///
/// Maps path parameter type converters to appropriate JSON Schema types:
/// - `Str` → string
/// - `Int` → integer (int64)
/// - `Float` → number (double)
/// - `Uuid` → string (uuid format)
/// - `Path` → string (catch-all wildcard)
#[must_use]
pub fn converter_to_schema(converter: &Converter) -> Schema {
    match converter {
        Converter::Int => Schema::integer(Some("int64")),
        Converter::Float => Schema::number(Some("double")),
        Converter::Uuid => Schema::Primitive(crate::schema::PrimitiveSchema {
            schema_type: crate::schema::SchemaType::String,
            format: Some("uuid".to_string()),
            nullable: false,
            minimum: None,
            maximum: None,
            exclusive_minimum: None,
            exclusive_maximum: None,
            min_length: None,
            max_length: None,
            pattern: None,
            enum_values: None,
        }),
        // Str and Path both map to string type
        Converter::Str | Converter::Path => Schema::string(),
    }
}

/// Convert a `ParamInfo` to an OpenAPI `Parameter` object.
///
/// Creates a path parameter with the appropriate schema type based on
/// the converter. All path parameters are required. Metadata (title,
/// description, deprecated, examples) is copied from the ParamInfo.
#[must_use]
pub fn param_info_to_parameter(param: &ParamInfo) -> Parameter {
    // Convert named examples from Vec<(String, Value)> to HashMap<String, Example>
    let examples: HashMap<String, Example> = param
        .examples
        .iter()
        .map(|(name, value)| {
            (
                name.clone(),
                Example {
                    summary: None,
                    description: None,
                    value: Some(value.clone()),
                    external_value: None,
                },
            )
        })
        .collect();

    Parameter {
        name: param.name.clone(),
        location: ParameterLocation::Path,
        required: true, // Path parameters are always required
        schema: Some(converter_to_schema(&param.converter)),
        title: param.title.clone(),
        description: param.description.clone(),
        deprecated: param.deprecated,
        example: param.example.clone(),
        examples,
    }
}

/// Extract path parameters from a route path pattern and convert them to OpenAPI Parameters.
///
/// Parses a path pattern like `/users/{id}/posts/{post_id:int}` and returns
/// OpenAPI Parameter objects for each path parameter.
///
/// # Examples
///
/// ```ignore
/// use fastapi_openapi::path_params_to_parameters;
///
/// let params = path_params_to_parameters("/users/{id}");
/// assert_eq!(params.len(), 1);
/// assert_eq!(params[0].name, "id");
///
/// // Typed parameters map to appropriate schemas
/// let params = path_params_to_parameters("/items/{item_id:int}");
/// // item_id will have an integer schema with int64 format
/// ```
#[must_use]
pub fn path_params_to_parameters(path: &str) -> Vec<Parameter> {
    extract_path_params(path)
        .iter()
        .map(param_info_to_parameter)
        .collect()
}

// ============================================================================
// Path Parameter Tests
// ============================================================================

#[cfg(test)]
mod path_param_tests {
    use super::*;
    use crate::schema::SchemaType;

    #[test]
    fn converter_to_schema_str() {
        let schema = converter_to_schema(&Converter::Str);
        if let Schema::Primitive(p) = schema {
            assert!(matches!(p.schema_type, SchemaType::String));
            assert!(p.format.is_none());
        } else {
            panic!("Expected primitive schema");
        }
    }

    #[test]
    fn converter_to_schema_int() {
        let schema = converter_to_schema(&Converter::Int);
        if let Schema::Primitive(p) = schema {
            assert!(matches!(p.schema_type, SchemaType::Integer));
            assert_eq!(p.format.as_deref(), Some("int64"));
        } else {
            panic!("Expected primitive schema");
        }
    }

    #[test]
    fn converter_to_schema_float() {
        let schema = converter_to_schema(&Converter::Float);
        if let Schema::Primitive(p) = schema {
            assert!(matches!(p.schema_type, SchemaType::Number));
            assert_eq!(p.format.as_deref(), Some("double"));
        } else {
            panic!("Expected primitive schema");
        }
    }

    #[test]
    fn converter_to_schema_uuid() {
        let schema = converter_to_schema(&Converter::Uuid);
        if let Schema::Primitive(p) = schema {
            assert!(matches!(p.schema_type, SchemaType::String));
            assert_eq!(p.format.as_deref(), Some("uuid"));
        } else {
            panic!("Expected primitive schema");
        }
    }

    #[test]
    fn converter_to_schema_path() {
        let schema = converter_to_schema(&Converter::Path);
        if let Schema::Primitive(p) = schema {
            assert!(matches!(p.schema_type, SchemaType::String));
        } else {
            panic!("Expected primitive schema");
        }
    }

    #[test]
    fn param_info_to_parameter_basic() {
        let param = param_info_to_parameter(&ParamInfo::new("id", Converter::Str));

        assert_eq!(param.name, "id");
        assert!(matches!(param.location, ParameterLocation::Path));
        assert!(param.required);
        assert!(param.schema.is_some());
    }

    #[test]
    fn param_info_to_parameter_int() {
        let param = param_info_to_parameter(&ParamInfo::new("item_id", Converter::Int));

        assert_eq!(param.name, "item_id");
        assert!(param.required);
        if let Some(Schema::Primitive(p)) = &param.schema {
            assert!(matches!(p.schema_type, SchemaType::Integer));
            assert_eq!(p.format.as_deref(), Some("int64"));
        } else {
            panic!("Expected integer schema");
        }
    }

    #[test]
    fn path_params_to_parameters_simple() {
        let params = path_params_to_parameters("/users/{id}");
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "id");
        assert!(matches!(params[0].location, ParameterLocation::Path));
        assert!(params[0].required);
    }

    #[test]
    fn path_params_to_parameters_multiple() {
        let params = path_params_to_parameters("/users/{user_id}/posts/{post_id}");
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].name, "user_id");
        assert_eq!(params[1].name, "post_id");
    }

    #[test]
    fn path_params_to_parameters_typed() {
        let params = path_params_to_parameters("/items/{id:int}/price/{value:float}");
        assert_eq!(params.len(), 2);

        // First param should be integer
        if let Some(Schema::Primitive(p)) = &params[0].schema {
            assert!(matches!(p.schema_type, SchemaType::Integer));
        } else {
            panic!("Expected integer schema for id");
        }

        // Second param should be number
        if let Some(Schema::Primitive(p)) = &params[1].schema {
            assert!(matches!(p.schema_type, SchemaType::Number));
        } else {
            panic!("Expected number schema for value");
        }
    }

    #[test]
    fn path_params_to_parameters_uuid() {
        let params = path_params_to_parameters("/resources/{uuid:uuid}");
        assert_eq!(params.len(), 1);

        if let Some(Schema::Primitive(p)) = &params[0].schema {
            assert!(matches!(p.schema_type, SchemaType::String));
            assert_eq!(p.format.as_deref(), Some("uuid"));
        } else {
            panic!("Expected string/uuid schema");
        }
    }

    #[test]
    fn path_params_to_parameters_wildcard() {
        let params = path_params_to_parameters("/files/{*filepath}");
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "filepath");

        if let Some(Schema::Primitive(p)) = &params[0].schema {
            assert!(matches!(p.schema_type, SchemaType::String));
        } else {
            panic!("Expected string schema for wildcard");
        }
    }

    #[test]
    fn path_params_to_parameters_no_params() {
        let params = path_params_to_parameters("/static/path");
        assert!(params.is_empty());
    }

    #[test]
    fn path_params_to_parameters_serialization() {
        let params = path_params_to_parameters("/users/{id:int}");
        let json = serde_json::to_string(&params[0]).unwrap();

        // Should have path location
        assert!(json.contains(r#""in":"path""#));
        // Should be required
        assert!(json.contains(r#""required":true"#));
        // Should have integer schema
        assert!(json.contains(r#""type":"integer""#));
        assert!(json.contains(r#""format":"int64""#));
    }

    #[test]
    fn path_params_complex_route() {
        let params = path_params_to_parameters("/api/v1/users/{user_id:int}/files/{*path}");
        assert_eq!(params.len(), 2);

        // user_id is integer
        assert_eq!(params[0].name, "user_id");
        if let Some(Schema::Primitive(p)) = &params[0].schema {
            assert!(matches!(p.schema_type, SchemaType::Integer));
        } else {
            panic!("Expected integer schema");
        }

        // path is string (wildcard)
        assert_eq!(params[1].name, "path");
        if let Some(Schema::Primitive(p)) = &params[1].schema {
            assert!(matches!(p.schema_type, SchemaType::String));
        } else {
            panic!("Expected string schema");
        }
    }

    // =========================================================================
    // PARAMETER METADATA TESTS
    // =========================================================================

    #[test]
    fn param_info_with_title() {
        let info = ParamInfo::new("user_id", Converter::Int).with_title("User ID");
        let param = param_info_to_parameter(&info);

        assert_eq!(param.title.as_deref(), Some("User ID"));
    }

    #[test]
    fn param_info_with_description() {
        let info =
            ParamInfo::new("page", Converter::Int).with_description("Page number for pagination");
        let param = param_info_to_parameter(&info);

        assert_eq!(
            param.description.as_deref(),
            Some("Page number for pagination")
        );
    }

    #[test]
    fn param_info_deprecated() {
        let info = ParamInfo::new("old_id", Converter::Str).deprecated();
        let param = param_info_to_parameter(&info);

        assert!(param.deprecated);
    }

    #[test]
    fn param_info_with_example() {
        let info = ParamInfo::new("user_id", Converter::Int).with_example(serde_json::json!(42));
        let param = param_info_to_parameter(&info);

        assert_eq!(param.example, Some(serde_json::json!(42)));
    }

    #[test]
    fn param_info_with_named_examples() {
        let info = ParamInfo::new("status", Converter::Str)
            .with_named_example("active", serde_json::json!("active"))
            .with_named_example("inactive", serde_json::json!("inactive"));
        let param = param_info_to_parameter(&info);

        assert_eq!(param.examples.len(), 2);
        assert!(param.examples.contains_key("active"));
        assert!(param.examples.contains_key("inactive"));
        assert_eq!(
            param.examples.get("active").unwrap().value,
            Some(serde_json::json!("active"))
        );
    }

    #[test]
    fn param_info_all_metadata() {
        let info = ParamInfo::new("item_id", Converter::Int)
            .with_title("Item ID")
            .with_description("The unique identifier for the item")
            .deprecated()
            .with_example(serde_json::json!(123))
            .with_named_example("first", serde_json::json!(1))
            .with_named_example("last", serde_json::json!(999));
        let param = param_info_to_parameter(&info);

        assert_eq!(param.name, "item_id");
        assert_eq!(param.title.as_deref(), Some("Item ID"));
        assert_eq!(
            param.description.as_deref(),
            Some("The unique identifier for the item")
        );
        assert!(param.deprecated);
        assert_eq!(param.example, Some(serde_json::json!(123)));
        assert_eq!(param.examples.len(), 2);
    }

    #[test]
    fn param_info_metadata_serialization() {
        let info = ParamInfo::new("id", Converter::Int)
            .with_title("ID")
            .with_description("Resource identifier")
            .deprecated();
        let param = param_info_to_parameter(&info);
        let json = serde_json::to_string(&param).unwrap();

        assert!(json.contains(r#""title":"ID""#));
        assert!(json.contains(r#""description":"Resource identifier""#));
        assert!(json.contains(r#""deprecated":true"#));
    }

    #[test]
    fn param_info_no_metadata_skips_fields() {
        let info = ParamInfo::new("id", Converter::Str);
        let param = param_info_to_parameter(&info);
        let json = serde_json::to_string(&param).unwrap();

        // Fields with None/false/empty should be skipped
        assert!(!json.contains("title"));
        assert!(!json.contains("description"));
        assert!(!json.contains("deprecated"));
        assert!(!json.contains("example"));
    }
}

// ============================================================================
// Route-to-OpenAPI Conversion Tests
// ============================================================================

#[cfg(test)]
mod route_conversion_tests {
    use super::*;
    use crate::schema::SchemaType;
    use fastapi_router::Route;

    fn make_test_route(path: &str, method: Method) -> Route {
        Route::with_placeholder_handler(method, path).operation_id("test_operation")
    }

    fn make_full_route() -> Route {
        Route::with_placeholder_handler(Method::Get, "/users/{id:int}/posts/{post_id:int}")
            .operation_id("get_user_post")
            .summary("Get a user's post")
            .description("Retrieves a specific post by a user")
            .tag("users")
            .tag("posts")
            .deprecated()
    }

    #[test]
    fn add_route_creates_operation_for_get() {
        let route = make_test_route("/users", Method::Get);
        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        assert!(doc.paths.contains_key("/users"));
        let path_item = &doc.paths["/users"];
        assert!(path_item.get.is_some());
        assert!(path_item.post.is_none());
    }

    #[test]
    fn add_route_creates_operation_for_post() {
        let route = make_test_route("/users", Method::Post);
        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let path_item = &doc.paths["/users"];
        assert!(path_item.post.is_some());
        assert!(path_item.get.is_none());
    }

    #[test]
    fn add_route_merges_methods_on_same_path() {
        let get_route = make_test_route("/users", Method::Get);
        let post_route = make_test_route("/users", Method::Post);

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&get_route);
        builder.add_route(&post_route);
        let doc = builder.build();

        let path_item = &doc.paths["/users"];
        assert!(path_item.get.is_some());
        assert!(path_item.post.is_some());
    }

    #[test]
    fn add_routes_batch_adds_multiple() {
        let routes = vec![
            make_test_route("/users", Method::Get),
            make_test_route("/users", Method::Post),
            make_test_route("/items", Method::Get),
        ];

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_routes(&routes);
        let doc = builder.build();

        assert!(doc.paths.contains_key("/users"));
        assert!(doc.paths.contains_key("/items"));
        assert!(doc.paths["/users"].get.is_some());
        assert!(doc.paths["/users"].post.is_some());
        assert!(doc.paths["/items"].get.is_some());
    }

    #[test]
    fn route_operation_id_is_preserved() {
        let route = Route::with_placeholder_handler(Method::Get, "/test")
            .operation_id("my_custom_operation");

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/test"].get.as_ref().unwrap();
        assert_eq!(op.operation_id.as_deref(), Some("my_custom_operation"));
    }

    #[test]
    fn route_summary_and_description_preserved() {
        let route = make_full_route();
        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/users/{id:int}/posts/{post_id:int}"]
            .get
            .as_ref()
            .unwrap();
        assert_eq!(op.summary.as_deref(), Some("Get a user's post"));
        assert_eq!(
            op.description.as_deref(),
            Some("Retrieves a specific post by a user")
        );
    }

    #[test]
    fn route_tags_preserved() {
        let route = make_full_route();
        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/users/{id:int}/posts/{post_id:int}"]
            .get
            .as_ref()
            .unwrap();
        assert!(op.tags.contains(&"users".to_string()));
        assert!(op.tags.contains(&"posts".to_string()));
    }

    #[test]
    fn route_deprecated_preserved() {
        let route = make_full_route();
        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/users/{id:int}/posts/{post_id:int}"]
            .get
            .as_ref()
            .unwrap();
        assert!(op.deprecated);
    }

    #[test]
    fn route_path_params_converted_to_parameters() {
        let route = make_full_route();
        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/users/{id:int}/posts/{post_id:int}"]
            .get
            .as_ref()
            .unwrap();

        // Should have two path parameters
        assert_eq!(op.parameters.len(), 2);
        assert_eq!(op.parameters[0].name, "id");
        assert_eq!(op.parameters[1].name, "post_id");

        // Both should be path parameters and required
        assert!(matches!(op.parameters[0].location, ParameterLocation::Path));
        assert!(matches!(op.parameters[1].location, ParameterLocation::Path));
        assert!(op.parameters[0].required);
        assert!(op.parameters[1].required);

        // Both should have integer schemas
        if let Some(Schema::Primitive(p)) = &op.parameters[0].schema {
            assert!(matches!(p.schema_type, SchemaType::Integer));
        } else {
            panic!("Expected integer schema for id");
        }
    }

    #[test]
    fn route_with_request_body() {
        let route = Route::with_placeholder_handler(Method::Post, "/users")
            .operation_id("create_user")
            .request_body("CreateUserRequest", "application/json", true);

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/users"].post.as_ref().unwrap();
        let body = op.request_body.as_ref().expect("Expected request body");

        assert!(body.required);
        assert!(body.content.contains_key("application/json"));

        let media_type = &body.content["application/json"];
        if let Some(Schema::Ref(ref_schema)) = &media_type.schema {
            assert_eq!(
                ref_schema.reference,
                "#/components/schemas/CreateUserRequest"
            );
        } else {
            panic!("Expected $ref schema for request body");
        }
    }

    #[test]
    fn route_with_custom_content_type() {
        let route = Route::with_placeholder_handler(Method::Post, "/upload")
            .operation_id("upload_file")
            .request_body("FileUpload", "multipart/form-data", false);

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/upload"].post.as_ref().unwrap();
        let body = op.request_body.as_ref().unwrap();
        assert!(body.content.contains_key("multipart/form-data"));
    }

    #[test]
    fn route_without_request_body() {
        let route = make_test_route("/users", Method::Get);
        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/users"].get.as_ref().unwrap();
        assert!(op.request_body.is_none());
    }

    #[test]
    fn all_http_methods_supported() {
        let methods = [
            Method::Get,
            Method::Post,
            Method::Put,
            Method::Delete,
            Method::Patch,
            Method::Options,
            Method::Head,
        ];

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        for method in methods {
            builder.add_route(&make_test_route("/test", method));
        }
        let doc = builder.build();

        let path_item = &doc.paths["/test"];
        assert!(path_item.get.is_some());
        assert!(path_item.post.is_some());
        assert!(path_item.put.is_some());
        assert!(path_item.delete.is_some());
        assert!(path_item.patch.is_some());
        assert!(path_item.options.is_some());
        assert!(path_item.head.is_some());
    }

    #[test]
    fn default_response_is_added() {
        let route = make_test_route("/users", Method::Get);
        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/users"].get.as_ref().unwrap();
        assert!(op.responses.contains_key("200"));
        assert_eq!(op.responses["200"].description, "Successful response");
    }

    #[test]
    fn route_conversion_serializes_to_valid_json() {
        let route = make_full_route();
        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        // Use compact JSON for easier substring matching
        let json = serde_json::to_string(&doc).unwrap();

        // Verify key elements are in the JSON (camelCase per OpenAPI spec)
        assert!(json.contains(r#""operationId":"get_user_post""#));
        assert!(json.contains(r#""summary":"Get a user's post""#));
        assert!(json.contains(r#""deprecated":true"#));
        assert!(json.contains(r#""in":"path""#));
        assert!(json.contains(r#""required":true"#));
    }

    #[test]
    fn empty_operation_id_becomes_none() {
        let route = Route::with_placeholder_handler(Method::Get, "/test").operation_id("");

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/test"].get.as_ref().unwrap();
        assert!(op.operation_id.is_none());

        // Verify it doesn't appear in serialized JSON
        let json = serde_json::to_string(&doc).unwrap();
        assert!(!json.contains("operationId"));
    }
}

// ============================================================================
// Security Scheme Tests
// ============================================================================

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn api_key_header_security_scheme() {
        let scheme = SecurityScheme::ApiKey {
            name: "X-API-Key".to_string(),
            location: ApiKeyLocation::Header,
            description: Some("API key for authentication".to_string()),
        };

        let json = serde_json::to_string(&scheme).unwrap();
        assert!(json.contains(r#""type":"apiKey""#));
        assert!(json.contains(r#""name":"X-API-Key""#));
        assert!(json.contains(r#""in":"header""#));
        assert!(json.contains(r#""description":"API key for authentication""#));
    }

    #[test]
    fn api_key_query_security_scheme() {
        let scheme = SecurityScheme::ApiKey {
            name: "api_key".to_string(),
            location: ApiKeyLocation::Query,
            description: None,
        };

        let json = serde_json::to_string(&scheme).unwrap();
        assert!(json.contains(r#""type":"apiKey""#));
        assert!(json.contains(r#""in":"query""#));
        assert!(!json.contains("description"));
    }

    #[test]
    fn http_bearer_security_scheme() {
        let scheme = SecurityScheme::Http {
            scheme: "bearer".to_string(),
            bearer_format: Some("JWT".to_string()),
            description: None,
        };

        let json = serde_json::to_string(&scheme).unwrap();
        assert!(json.contains(r#""type":"http""#));
        assert!(json.contains(r#""scheme":"bearer""#));
        assert!(json.contains(r#""bearerFormat":"JWT""#));
    }

    #[test]
    fn http_basic_security_scheme() {
        let scheme = SecurityScheme::Http {
            scheme: "basic".to_string(),
            bearer_format: None,
            description: Some("Basic HTTP authentication".to_string()),
        };

        let json = serde_json::to_string(&scheme).unwrap();
        assert!(json.contains(r#""type":"http""#));
        assert!(json.contains(r#""scheme":"basic""#));
        assert!(!json.contains("bearerFormat"));
    }

    #[test]
    fn oauth2_security_scheme() {
        let mut scopes = HashMap::new();
        scopes.insert("read:users".to_string(), "Read user data".to_string());
        scopes.insert("write:users".to_string(), "Modify user data".to_string());

        let scheme = SecurityScheme::OAuth2 {
            flows: OAuth2Flows {
                authorization_code: Some(OAuth2Flow {
                    authorization_url: Some("https://example.com/oauth/authorize".to_string()),
                    token_url: Some("https://example.com/oauth/token".to_string()),
                    refresh_url: None,
                    scopes,
                }),
                ..Default::default()
            },
            description: None,
        };

        let json = serde_json::to_string(&scheme).unwrap();
        assert!(json.contains(r#""type":"oauth2""#));
        assert!(json.contains(r#""authorizationCode""#));
        assert!(json.contains(r#""authorizationUrl""#));
        assert!(json.contains(r#""tokenUrl""#));
        assert!(json.contains(r#""read:users""#));
    }

    #[test]
    fn openid_connect_security_scheme() {
        let scheme = SecurityScheme::OpenIdConnect {
            open_id_connect_url: "https://example.com/.well-known/openid-configuration".to_string(),
            description: Some("OpenID Connect authentication".to_string()),
        };

        let json = serde_json::to_string(&scheme).unwrap();
        assert!(json.contains(r#""type":"openIdConnect""#));
        assert!(json.contains(r#""openIdConnectUrl""#));
    }

    #[test]
    fn builder_adds_security_scheme() {
        let doc = OpenApiBuilder::new("Test API", "1.0.0")
            .security_scheme(
                "api_key",
                SecurityScheme::ApiKey {
                    name: "X-API-Key".to_string(),
                    location: ApiKeyLocation::Header,
                    description: None,
                },
            )
            .build();

        assert!(doc.components.is_some());
        let components = doc.components.as_ref().unwrap();
        assert!(components.security_schemes.contains_key("api_key"));
    }

    #[test]
    fn builder_adds_global_security_requirement() {
        let doc = OpenApiBuilder::new("Test API", "1.0.0")
            .security_scheme(
                "bearer",
                SecurityScheme::Http {
                    scheme: "bearer".to_string(),
                    bearer_format: Some("JWT".to_string()),
                    description: None,
                },
            )
            .security_requirement("bearer", vec![])
            .build();

        assert_eq!(doc.security.len(), 1);
        assert!(doc.security[0].contains_key("bearer"));
    }

    #[test]
    fn builder_adds_security_with_scopes() {
        let doc = OpenApiBuilder::new("Test API", "1.0.0")
            .security_requirement("oauth2", vec!["read:users".to_string(), "write:users".to_string()])
            .build();

        assert_eq!(doc.security.len(), 1);
        let scopes = doc.security[0].get("oauth2").unwrap();
        assert_eq!(scopes.len(), 2);
        assert!(scopes.contains(&"read:users".to_string()));
        assert!(scopes.contains(&"write:users".to_string()));
    }

    #[test]
    fn full_security_document_serializes() {
        let doc = OpenApiBuilder::new("Secure API", "1.0.0")
            .security_scheme(
                "api_key",
                SecurityScheme::ApiKey {
                    name: "X-API-Key".to_string(),
                    location: ApiKeyLocation::Header,
                    description: Some("API key authentication".to_string()),
                },
            )
            .security_scheme(
                "bearer",
                SecurityScheme::Http {
                    scheme: "bearer".to_string(),
                    bearer_format: Some("JWT".to_string()),
                    description: None,
                },
            )
            .security_requirement("api_key", vec![])
            .build();

        let json = serde_json::to_string_pretty(&doc).unwrap();

        // Verify the document structure
        assert!(json.contains(r#""securitySchemes""#));
        assert!(json.contains(r#""api_key""#));
        assert!(json.contains(r#""bearer""#));
        assert!(json.contains(r#""security""#));
    }
}
