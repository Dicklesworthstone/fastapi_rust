//! Test fixtures for common output scenarios.

/// Sample route metadata for output component tests.
#[derive(Debug, Clone)]
pub struct FixtureRoute {
    /// HTTP method.
    pub method: &'static str,
    /// Route path.
    pub path: &'static str,
    /// Optional handler name.
    pub handler_name: Option<&'static str>,
}

/// Create a sample set of routes for testing.
#[must_use]
pub fn sample_routes() -> Vec<FixtureRoute> {
    vec![
        FixtureRoute {
            method: "GET",
            path: "/users",
            handler_name: Some("list_users"),
        },
        FixtureRoute {
            method: "POST",
            path: "/users",
            handler_name: Some("create_user"),
        },
        FixtureRoute {
            method: "GET",
            path: "/users/{id}",
            handler_name: Some("get_user"),
        },
    ]
}

/// Sample validation error fixture.
#[derive(Debug, Clone)]
pub struct FixtureValidationError {
    /// Path segments to the invalid field.
    pub path: Vec<&'static str>,
    /// Error message.
    pub message: &'static str,
    /// Error code.
    pub code: &'static str,
    /// Expected value description.
    pub expected: Option<&'static str>,
    /// Received value description.
    pub received: Option<&'static str>,
}

/// Create sample validation errors.
#[must_use]
pub fn sample_validation_errors() -> Vec<FixtureValidationError> {
    vec![FixtureValidationError {
        path: vec!["email"],
        message: "Invalid email format",
        code: "email",
        expected: Some("valid email address"),
        received: Some("not-an-email"),
    }]
}

/// Sample middleware metadata for output tests.
#[derive(Debug, Clone)]
pub struct FixtureMiddlewareInfo {
    /// Middleware name.
    pub name: &'static str,
    /// Type name for display.
    pub type_name: &'static str,
    /// Registration order.
    pub order: usize,
    /// Whether it can short-circuit.
    pub can_short_circuit: bool,
    /// Optional configuration summary.
    pub config_summary: Option<&'static str>,
}

/// Create sample middleware stack.
#[must_use]
pub fn sample_middleware() -> Vec<FixtureMiddlewareInfo> {
    vec![
        FixtureMiddlewareInfo {
            name: "RequestLogger",
            type_name: "fastapi::middleware::RequestLogger",
            order: 0,
            can_short_circuit: false,
            config_summary: Some("level=INFO"),
        },
        FixtureMiddlewareInfo {
            name: "Auth",
            type_name: "fastapi::middleware::Auth",
            order: 1,
            can_short_circuit: true,
            config_summary: Some("scheme=Bearer"),
        },
    ]
}
