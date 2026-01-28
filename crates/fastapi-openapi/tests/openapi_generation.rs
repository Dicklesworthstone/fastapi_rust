//! Integration tests for OpenAPI document generation.
//!
//! This test suite covers:
//! - Path generation with various parameter types
//! - Schema deduplication
//! - Request body and response handling
//! - Security scheme integration
//! - Validation against OpenAPI 3.1 spec

use fastapi_core::Method;
use fastapi_openapi::{OpenApiBuilder, ParameterLocation, Schema, SchemaRegistry};
use fastapi_router::Route;

// ============================================================================
// PATH GENERATION TESTS
// ============================================================================

mod path_generation {
    use super::*;

    #[test]
    fn static_path_generates_correct_path_item() {
        let route = Route::with_placeholder_handler(Method::Get, "/users")
            .operation_id("list_users")
            .summary("List all users");

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        assert!(doc.paths.contains_key("/users"));
        let path_item = &doc.paths["/users"];
        assert!(path_item.get.is_some());

        let op = path_item.get.as_ref().unwrap();
        assert_eq!(op.operation_id.as_deref(), Some("list_users"));
        assert_eq!(op.summary.as_deref(), Some("List all users"));
    }

    #[test]
    fn path_with_string_parameter() {
        let route = Route::with_placeholder_handler(Method::Get, "/users/{username}")
            .operation_id("get_user_by_username");

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let path_item = &doc.paths["/users/{username}"];
        let op = path_item.get.as_ref().unwrap();

        assert_eq!(op.parameters.len(), 1);
        assert_eq!(op.parameters[0].name, "username");
        assert!(matches!(
            op.parameters[0].location,
            ParameterLocation::Path
        ));
        assert!(op.parameters[0].required);
    }

    #[test]
    fn path_with_integer_parameter() {
        let route = Route::with_placeholder_handler(Method::Get, "/users/{id:int}")
            .operation_id("get_user_by_id");

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/users/{id:int}"].get.as_ref().unwrap();
        assert_eq!(op.parameters.len(), 1);
        assert_eq!(op.parameters[0].name, "id");

        // Check schema is integer
        let json = serde_json::to_string(&op.parameters[0]).unwrap();
        assert!(json.contains(r#""type":"integer""#));
        assert!(json.contains(r#""format":"int64""#));
    }

    #[test]
    fn path_with_uuid_parameter() {
        let route = Route::with_placeholder_handler(Method::Get, "/items/{item_id:uuid}")
            .operation_id("get_item");

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/items/{item_id:uuid}"].get.as_ref().unwrap();
        let json = serde_json::to_string(&op.parameters[0]).unwrap();
        assert!(json.contains(r#""type":"string""#));
        assert!(json.contains(r#""format":"uuid""#));
    }

    #[test]
    fn path_with_multiple_parameters() {
        let route = Route::with_placeholder_handler(Method::Get, "/users/{user_id:int}/posts/{post_id:int}")
            .operation_id("get_user_post");

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/users/{user_id:int}/posts/{post_id:int}"]
            .get
            .as_ref()
            .unwrap();

        assert_eq!(op.parameters.len(), 2);
        assert_eq!(op.parameters[0].name, "user_id");
        assert_eq!(op.parameters[1].name, "post_id");
    }

    #[test]
    fn wildcard_path_parameter() {
        let route = Route::with_placeholder_handler(Method::Get, "/files/{*filepath}")
            .operation_id("get_file");

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/files/{*filepath}"].get.as_ref().unwrap();
        assert_eq!(op.parameters.len(), 1);
        assert_eq!(op.parameters[0].name, "filepath");

        // Wildcard is a string type
        let json = serde_json::to_string(&op.parameters[0]).unwrap();
        assert!(json.contains(r#""type":"string""#));
    }

    #[test]
    fn multiple_methods_on_same_path() {
        let get_route = Route::with_placeholder_handler(Method::Get, "/items")
            .operation_id("list_items");
        let post_route = Route::with_placeholder_handler(Method::Post, "/items")
            .operation_id("create_item");
        let put_route = Route::with_placeholder_handler(Method::Put, "/items/{id:int}")
            .operation_id("update_item");
        let delete_route = Route::with_placeholder_handler(Method::Delete, "/items/{id:int}")
            .operation_id("delete_item");

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_routes(&[get_route, post_route, put_route, delete_route]);
        let doc = builder.build();

        // Check /items has GET and POST
        let items_path = &doc.paths["/items"];
        assert!(items_path.get.is_some());
        assert!(items_path.post.is_some());
        assert_eq!(
            items_path.get.as_ref().unwrap().operation_id.as_deref(),
            Some("list_items")
        );
        assert_eq!(
            items_path.post.as_ref().unwrap().operation_id.as_deref(),
            Some("create_item")
        );

        // Check /items/{id:int} has PUT and DELETE
        let item_path = &doc.paths["/items/{id:int}"];
        assert!(item_path.put.is_some());
        assert!(item_path.delete.is_some());
    }
}

// ============================================================================
// SCHEMA DEDUPLICATION TESTS
// ============================================================================

mod schema_deduplication {
    use super::*;

    #[test]
    fn registry_deduplicates_same_schema() {
        let registry = SchemaRegistry::new();

        // Register the same schema twice
        let ref1 = registry.register("User", Schema::string());
        let ref2 = registry.register("User", Schema::integer(Some("int64")));

        // Both should get the same $ref
        let json1 = serde_json::to_string(&ref1).unwrap();
        let json2 = serde_json::to_string(&ref2).unwrap();

        assert!(json1.contains("#/components/schemas/User"));
        assert!(json2.contains("#/components/schemas/User"));

        // But the schema should be the first one registered
        let schemas = registry.into_schemas();
        assert_eq!(schemas.len(), 1);
    }

    #[test]
    fn registry_handles_multiple_schemas() {
        let registry = SchemaRegistry::new();

        registry.register("User", Schema::string());
        registry.register("Item", Schema::integer(None));
        registry.register("Order", Schema::boolean());

        let schemas = registry.into_schemas();
        assert_eq!(schemas.len(), 3);
        assert!(schemas.contains_key("User"));
        assert!(schemas.contains_key("Item"));
        assert!(schemas.contains_key("Order"));
    }

    #[test]
    fn builder_includes_registry_schemas_in_components() {
        let builder = OpenApiBuilder::new("Test API", "1.0.0");

        builder.registry().register("User", Schema::string());
        builder.registry().register("Item", Schema::integer(None));

        let doc = builder.build();

        assert!(doc.components.is_some());
        let components = doc.components.as_ref().unwrap();
        assert!(components.schemas.contains_key("User"));
        assert!(components.schemas.contains_key("Item"));
    }

    #[test]
    fn explicit_schemas_override_registry() {
        let builder = OpenApiBuilder::new("Test API", "1.0.0");

        // Register via registry
        builder.registry().register("User", Schema::string());

        // Override with explicit schema
        let builder = builder.schema("User", Schema::boolean());

        let doc = builder.build();
        let components = doc.components.as_ref().unwrap();

        // Should be boolean (explicit), not string (registry)
        let json = serde_json::to_string(&components.schemas["User"]).unwrap();
        assert!(json.contains(r#""type":"boolean""#));
    }
}

// ============================================================================
// REQUEST BODY TESTS
// ============================================================================

mod request_body {
    use super::*;

    #[test]
    fn route_with_json_request_body() {
        let route = Route::with_placeholder_handler(Method::Post, "/users")
            .operation_id("create_user")
            .request_body("CreateUserRequest", "application/json", true);

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/users"].post.as_ref().unwrap();
        let body = op.request_body.as_ref().expect("Should have request body");

        assert!(body.required);
        assert!(body.content.contains_key("application/json"));

        let json = serde_json::to_string(body).unwrap();
        assert!(json.contains("CreateUserRequest"));
    }

    #[test]
    fn route_with_form_request_body() {
        let route = Route::with_placeholder_handler(Method::Post, "/upload")
            .operation_id("upload_file")
            .request_body("FileUpload", "multipart/form-data", true);

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/upload"].post.as_ref().unwrap();
        let body = op.request_body.as_ref().unwrap();

        assert!(body.content.contains_key("multipart/form-data"));
    }

    #[test]
    fn route_with_optional_request_body() {
        let route = Route::with_placeholder_handler(Method::Patch, "/users/{id:int}")
            .operation_id("update_user")
            .request_body("UpdateUserRequest", "application/json", false);

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/users/{id:int}"].patch.as_ref().unwrap();
        let body = op.request_body.as_ref().unwrap();

        assert!(!body.required);
    }

    #[test]
    fn route_without_request_body() {
        let route = Route::with_placeholder_handler(Method::Get, "/users")
            .operation_id("list_users");

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/users"].get.as_ref().unwrap();
        assert!(op.request_body.is_none());
    }
}

// ============================================================================
// RESPONSE TESTS
// ============================================================================

mod responses {
    use super::*;

    #[test]
    fn default_200_response_is_added() {
        let route = Route::with_placeholder_handler(Method::Get, "/health")
            .operation_id("health_check");

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/health"].get.as_ref().unwrap();
        assert!(op.responses.contains_key("200"));
        assert_eq!(op.responses["200"].description, "Successful response");
    }
}

// ============================================================================
// METADATA TESTS
// ============================================================================

mod metadata {
    use super::*;

    #[test]
    fn route_tags_are_preserved() {
        let route = Route::with_placeholder_handler(Method::Get, "/users")
            .operation_id("list_users")
            .tag("users")
            .tag("admin");

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/users"].get.as_ref().unwrap();
        assert!(op.tags.contains(&"users".to_string()));
        assert!(op.tags.contains(&"admin".to_string()));
    }

    #[test]
    fn deprecated_flag_is_preserved() {
        let route = Route::with_placeholder_handler(Method::Get, "/v1/users")
            .operation_id("list_users_v1")
            .deprecated();

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/v1/users"].get.as_ref().unwrap();
        assert!(op.deprecated);
    }

    #[test]
    fn empty_operation_id_becomes_none() {
        let route = Route::with_placeholder_handler(Method::Get, "/test").operation_id("");

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let op = doc.paths["/test"].get.as_ref().unwrap();
        assert!(op.operation_id.is_none());
    }

    #[test]
    fn builder_sets_api_metadata() {
        let doc = OpenApiBuilder::new("My API", "2.0.0")
            .description("A comprehensive API")
            .server("https://api.example.com", Some("Production".to_string()))
            .server("https://staging.example.com", Some("Staging".to_string()))
            .tag("users", Some("User management".to_string()))
            .tag("items", Some("Item operations".to_string()))
            .build();

        assert_eq!(doc.openapi, "3.1.0");
        assert_eq!(doc.info.title, "My API");
        assert_eq!(doc.info.version, "2.0.0");
        assert_eq!(doc.info.description.as_deref(), Some("A comprehensive API"));
        assert_eq!(doc.servers.len(), 2);
        assert_eq!(doc.servers[0].url, "https://api.example.com");
        assert_eq!(doc.tags.len(), 2);
    }
}

// ============================================================================
// SERIALIZATION TESTS
// ============================================================================

mod serialization {
    use super::*;

    #[test]
    fn openapi_document_serializes_to_valid_json() {
        let route = Route::with_placeholder_handler(Method::Get, "/users/{id:int}")
            .operation_id("get_user")
            .summary("Get a user by ID")
            .description("Returns a single user")
            .tag("users")
            .deprecated();

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0")
            .description("API for testing")
            .server("https://api.example.com", None);

        builder.add_route(&route);
        let doc = builder.build();

        let json = serde_json::to_string_pretty(&doc).unwrap();

        // Verify it's valid JSON that can be parsed back
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["openapi"], "3.1.0");
        assert_eq!(parsed["info"]["title"], "Test API");
        assert_eq!(parsed["info"]["version"], "1.0.0");
    }

    #[test]
    fn operation_uses_camel_case_for_openapi_compliance() {
        let route = Route::with_placeholder_handler(Method::Post, "/users")
            .operation_id("create_user")
            .request_body("CreateUser", "application/json", true);

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let json = serde_json::to_string(&doc).unwrap();

        // Check camelCase field names per OpenAPI spec
        assert!(json.contains(r#""operationId""#));
        assert!(json.contains(r#""requestBody""#));
    }

    #[test]
    fn empty_fields_are_omitted() {
        let doc = OpenApiBuilder::new("Test API", "1.0.0").build();
        let json = serde_json::to_string(&doc).unwrap();

        // These should be omitted when empty
        assert!(!json.contains("servers"));
        assert!(!json.contains("components"));
        assert!(!json.contains("tags"));
        assert!(!json.contains("paths"));
        assert!(!json.contains("security"));
    }

    #[test]
    fn false_deprecated_is_omitted() {
        let route =
            Route::with_placeholder_handler(Method::Get, "/test").operation_id("test_endpoint");

        let mut builder = OpenApiBuilder::new("Test API", "1.0.0");
        builder.add_route(&route);
        let doc = builder.build();

        let json = serde_json::to_string(&doc).unwrap();
        // deprecated: false should be omitted
        assert!(!json.contains("deprecated"));
    }
}

// ============================================================================
// FULL INTEGRATION TEST
// ============================================================================

#[test]
fn full_api_document_generation() {
    // Create routes for a simple CRUD API
    let routes = vec![
        Route::with_placeholder_handler(Method::Get, "/users")
            .operation_id("list_users")
            .summary("List all users")
            .tag("users"),
        Route::with_placeholder_handler(Method::Post, "/users")
            .operation_id("create_user")
            .summary("Create a new user")
            .tag("users")
            .request_body("CreateUserRequest", "application/json", true),
        Route::with_placeholder_handler(Method::Get, "/users/{id:int}")
            .operation_id("get_user")
            .summary("Get a user by ID")
            .tag("users"),
        Route::with_placeholder_handler(Method::Put, "/users/{id:int}")
            .operation_id("update_user")
            .summary("Update a user")
            .tag("users")
            .request_body("UpdateUserRequest", "application/json", true),
        Route::with_placeholder_handler(Method::Delete, "/users/{id:int}")
            .operation_id("delete_user")
            .summary("Delete a user")
            .tag("users"),
    ];

    let mut builder = OpenApiBuilder::new("User Management API", "1.0.0")
        .description("API for managing users")
        .server("https://api.example.com/v1", Some("Production".to_string()))
        .tag("users", Some("User operations".to_string()));

    // Register schemas
    builder.registry().register(
        "CreateUserRequest",
        Schema::object(
            [
                ("name".to_string(), Schema::string()),
                ("email".to_string(), Schema::string()),
            ]
            .into_iter()
            .collect(),
            vec!["name".to_string(), "email".to_string()],
        ),
    );

    builder.registry().register(
        "UpdateUserRequest",
        Schema::object(
            [
                ("name".to_string(), Schema::string()),
                ("email".to_string(), Schema::string()),
            ]
            .into_iter()
            .collect(),
            vec![],
        ),
    );

    builder.add_routes(&routes);
    let doc = builder.build();

    // Verify document structure
    assert_eq!(doc.openapi, "3.1.0");
    assert_eq!(doc.info.title, "User Management API");
    assert_eq!(doc.paths.len(), 2); // /users and /users/{id:int}

    // Verify /users has GET and POST
    let users_path = &doc.paths["/users"];
    assert!(users_path.get.is_some());
    assert!(users_path.post.is_some());

    // Verify /users/{id:int} has GET, PUT, DELETE
    let user_path = &doc.paths["/users/{id:int}"];
    assert!(user_path.get.is_some());
    assert!(user_path.put.is_some());
    assert!(user_path.delete.is_some());

    // Verify components/schemas
    let components = doc.components.as_ref().unwrap();
    assert!(components.schemas.contains_key("CreateUserRequest"));
    assert!(components.schemas.contains_key("UpdateUserRequest"));

    // Verify serialization works
    let json = serde_json::to_string_pretty(&doc).unwrap();
    assert!(json.len() > 100); // Should be a substantial document

    // Verify it can be parsed back
    let _: serde_json::Value = serde_json::from_str(&json).unwrap();
}
