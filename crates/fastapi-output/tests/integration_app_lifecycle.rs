//! Integration tests for fastapi-output with App lifecycle scenarios.
//!
//! These tests verify that fastapi-output components work correctly
//! in scenarios that mirror fastapi-core's App startup, request handling,
//! and shutdown phases.
//!
//! Note: These tests don't depend on fastapi-core directly, but they
//! simulate the same patterns used when integrating with the framework.

use fastapi_output::prelude::*;
use serial_test::serial;
use std::env;
use std::time::Duration;

// =============================================================================
// Environment Management Helpers
// =============================================================================

#[allow(unsafe_code)]
fn clean_env() {
    unsafe {
        env::remove_var("FASTAPI_OUTPUT_MODE");
        env::remove_var("FASTAPI_AGENT_MODE");
        env::remove_var("FASTAPI_HUMAN_MODE");
        env::remove_var("CLAUDE_CODE");
        env::remove_var("CI");
        env::remove_var("NO_COLOR");
        env::remove_var("FORCE_COLOR");
    }
}

fn with_clean_env<F: FnOnce()>(f: F) {
    clean_env();
    f();
    clean_env();
}

#[allow(unsafe_code)]
fn set_env(key: &str, value: &str) {
    unsafe {
        env::set_var(key, value);
    }
}

// =============================================================================
// App Startup Lifecycle Integration Tests
// =============================================================================

/// Tests the complete App startup sequence in plain mode (agent environment).
/// This mirrors what happens when an App is started with logging enabled.
#[test]
#[serial]
fn test_app_startup_lifecycle_plain_mode() {
    with_clean_env(|| {
        set_env("FASTAPI_OUTPUT_MODE", "plain");

        let output = RichOutput::auto();
        assert_eq!(output.mode(), OutputMode::Plain);

        // Phase 1: Configuration loading
        let config_output = capture(OutputMode::Plain, || {
            let out = RichOutput::plain();
            out.info("Loading configuration from environment...");
            out.success("Configuration loaded");
            out.info("Debug mode: enabled");
            out.info("Max body size: 10MB");
        });

        assert_no_ansi(&config_output);
        assert_contains(&config_output, "[INFO]");
        assert_contains(&config_output, "[OK]");
        assert_contains(&config_output, "Configuration loaded");

        // Phase 2: Route registration
        let routes = vec![
            RouteEntry {
                method: "GET".to_string(),
                path: "/".to_string(),
                handler: Some("root_handler".to_string()),
                tags: vec!["root".to_string()],
                deprecated: false,
            },
            RouteEntry {
                method: "GET".to_string(),
                path: "/health".to_string(),
                handler: Some("health_check".to_string()),
                tags: vec!["health".to_string()],
                deprecated: false,
            },
            RouteEntry {
                method: "POST".to_string(),
                path: "/api/users".to_string(),
                handler: Some("create_user".to_string()),
                tags: vec!["users".to_string()],
                deprecated: false,
            },
            RouteEntry {
                method: "GET".to_string(),
                path: "/api/users/{id}".to_string(),
                handler: Some("get_user".to_string()),
                tags: vec!["users".to_string()],
                deprecated: false,
            },
            RouteEntry {
                method: "DELETE".to_string(),
                path: "/api/users/{id}".to_string(),
                handler: Some("delete_user".to_string()),
                tags: vec!["users".to_string()],
                deprecated: false,
            },
        ];

        let route_config = RouteTableConfig {
            show_handlers: true,
            ..RouteTableConfig::default()
        };
        let route_display = RouteDisplay::with_config(OutputMode::Plain, route_config);
        let route_output = route_display.render(&routes);

        assert_no_ansi(&route_output);
        assert_contains(&route_output, "GET");
        assert_contains(&route_output, "POST");
        assert_contains(&route_output, "DELETE");
        assert_contains(&route_output, "/api/users");
        assert_contains(&route_output, "create_user");

        // Phase 3: Server startup banner
        let banner = Banner::new(OutputMode::Plain);
        let server_info = ServerInfo::new("0.1.0", "0.0.0.0", 8000)
            .docs_path("/docs")
            .redoc_path("/redoc")
            .openapi_path("/openapi.json");
        let banner_output = banner.render(&server_info);

        assert_no_ansi(&banner_output);
        assert_contains(&banner_output, "FastAPI Rust");
        assert_contains(&banner_output, "0.1.0");
        assert_contains(&banner_output, "http://0.0.0.0:8000");

        // Phase 4: Startup complete status
        let startup_output = capture(OutputMode::Plain, || {
            let out = RichOutput::plain();
            out.success("Application startup complete");
            out.info("Press Ctrl+C to stop");
        });

        assert_no_ansi(&startup_output);
        assert_contains(&startup_output, "startup complete");
    });
}

/// Tests the complete App startup sequence in rich mode (human terminal).
#[test]
#[serial]
fn test_app_startup_lifecycle_rich_mode() {
    with_clean_env(|| {
        set_env("FASTAPI_OUTPUT_MODE", "rich");

        let _output = RichOutput::auto();

        // Rich mode should have ANSI codes when forced
        let banner = Banner::new(OutputMode::Rich);
        let server_info = ServerInfo::new("0.1.0", "127.0.0.1", 8000).docs_path("/docs");
        let banner_output = banner.render(&server_info);

        // Should have ANSI styling
        assert_has_ansi(&banner_output);
        // Check for version and URL in banner (ASCII art header doesn't contain literal "FastAPI Rust")
        assert_contains(&banner_output, "0.1.0");
        assert_contains(&banner_output, "127.0.0.1");

        // Route table should be styled
        let routes = vec![RouteEntry {
            method: "GET".to_string(),
            path: "/".to_string(),
            handler: Some("root".to_string()),
            tags: vec![],
            deprecated: false,
        }];

        let route_display = RouteDisplay::new(OutputMode::Rich);
        let route_output = route_display.render(&routes);

        assert_has_ansi(&route_output);
        assert_contains(&route_output, "GET");
    });
}

// =============================================================================
// Request Handling Integration Tests
// =============================================================================

/// Tests request logging output in the format used during App.handle().
#[test]
#[serial]
fn test_request_logging_integration() {
    with_clean_env(|| {
        set_env("FASTAPI_OUTPUT_MODE", "plain");

        let logger = RequestLogger::new(OutputMode::Plain);

        // Simulate various requests that would go through App.handle()
        let requests = vec![
            LogEntry {
                method: HttpMethod::Get,
                path: "/api/users".to_string(),
                query: None,
                status: 200,
                timing: Some(ResponseTiming::new(Duration::from_millis(15))),
                client_ip: Some("127.0.0.1".to_string()),
                request_id: None,
            },
            LogEntry {
                method: HttpMethod::Post,
                path: "/api/users".to_string(),
                query: None,
                status: 201,
                timing: Some(ResponseTiming::new(Duration::from_millis(45))),
                client_ip: Some("127.0.0.1".to_string()),
                request_id: None,
            },
            LogEntry {
                method: HttpMethod::Get,
                path: "/api/users/123".to_string(),
                query: None,
                status: 404,
                timing: Some(ResponseTiming::new(Duration::from_millis(8))),
                client_ip: Some("192.168.1.1".to_string()),
                request_id: None,
            },
            LogEntry {
                method: HttpMethod::Delete,
                path: "/api/users/456".to_string(),
                query: None,
                status: 204,
                timing: Some(ResponseTiming::new(Duration::from_millis(22))),
                client_ip: None,
                request_id: None,
            },
        ];

        for entry in &requests {
            let log_output = logger.format(entry);

            // All logs should be ANSI-free in plain mode
            assert_no_ansi(&log_output);

            // Should contain method, path, and status
            assert_contains(&log_output, entry.method.as_str());
            assert_contains(&log_output, &entry.path);
            assert_contains(&log_output, &entry.status.to_string());
        }
    });
}

/// Tests that request logging handles high-volume scenarios.
#[test]
#[serial]
fn test_request_logging_performance() {
    with_clean_env(|| {
        set_env("FASTAPI_OUTPUT_MODE", "plain");

        let logger = RequestLogger::new(OutputMode::Plain);

        // Simulate high-throughput logging
        let start = std::time::Instant::now();

        for i in 0..1000 {
            let entry = LogEntry {
                method: if i % 3 == 0 {
                    HttpMethod::Post
                } else {
                    HttpMethod::Get
                },
                path: format!("/api/items/{i}"),
                query: None,
                status: if i % 10 == 0 { 500 } else { 200 },
                timing: Some(ResponseTiming::new(Duration::from_micros(
                    #[allow(clippy::cast_sign_loss)]
                    { (i % 100) as u64 * 100 },
                ))),
                client_ip: Some("127.0.0.1".to_string()),
                request_id: None,
            };

            let _ = logger.format(&entry);
        }

        let elapsed = start.elapsed();

        // Should complete 1000 log entries in under 100ms
        assert!(
            elapsed < Duration::from_millis(100),
            "Logging 1000 entries took {elapsed:?}, expected < 100ms",
        );
    });
}

// =============================================================================
// Error Handling Integration Tests
// =============================================================================

/// Tests validation error formatting as it would appear in App error responses.
#[test]
#[serial]
fn test_validation_error_formatting() {
    with_clean_env(|| {
        set_env("FASTAPI_OUTPUT_MODE", "plain");

        let formatter = ErrorFormatter::new(OutputMode::Plain);

        // Simulate validation errors from request body parsing
        let errors = vec![
            ValidationErrorDetail {
                loc: vec![
                    LocItem::Field("body".to_string()),
                    LocItem::Field("email".to_string()),
                ],
                msg: "Invalid email format".to_string(),
                error_type: "value_error.email".to_string(),
                input: Some("not-an-email".to_string()),
                expected: Some("valid email format".to_string()),
                ctx: None,
            },
            ValidationErrorDetail {
                loc: vec![
                    LocItem::Field("body".to_string()),
                    LocItem::Field("age".to_string()),
                ],
                msg: "Value must be positive".to_string(),
                error_type: "value_error.number.not_positive".to_string(),
                input: Some("-5".to_string()),
                expected: Some("positive integer".to_string()),
                ctx: Some(ValidationContext {
                    min: Some("1".to_string()),
                    max: None,
                    pattern: None,
                    expected_type: Some("integer".to_string()),
                    extra: vec![],
                }),
            },
        ];

        let error_output = formatter.format_validation_errors(&errors);

        assert_no_ansi(&error_output.plain);
        assert_contains(&error_output.plain, "email");
        assert_contains(&error_output.plain, "Invalid email format");
        assert_contains(&error_output.plain, "age");
        assert_contains(&error_output.plain, "must be positive");
    });
}

/// Tests HTTP error formatting as it would appear in App error responses.
#[test]
#[serial]
fn test_http_error_formatting() {
    with_clean_env(|| {
        set_env("FASTAPI_OUTPUT_MODE", "plain");

        let formatter = ErrorFormatter::new(OutputMode::Plain);

        // Test various HTTP error scenarios
        let errors = vec![
            HttpErrorInfo {
                status: 404,
                detail: "User not found: No user with ID 12345 exists".to_string(),
                code: Some("USER_NOT_FOUND".to_string()),
                path: Some("/api/users/12345".to_string()),
                method: Some("GET".to_string()),
            },
            HttpErrorInfo {
                status: 401,
                detail: "Unauthorized: Invalid or expired token".to_string(),
                code: Some("AUTH_INVALID_TOKEN".to_string()),
                path: Some("/api/protected".to_string()),
                method: Some("GET".to_string()),
            },
            HttpErrorInfo {
                status: 500,
                detail: "Internal Server Error: Database connection failed".to_string(),
                code: None,
                path: None,
                method: None,
            },
        ];

        for error in &errors {
            let error_output = formatter.format_http_error(error);

            assert_no_ansi(&error_output.plain);
            assert_contains(&error_output.plain, &error.status.to_string());
            assert_contains(&error_output.plain, &error.detail);
        }
    });
}

// =============================================================================
// Shutdown Lifecycle Integration Tests
// =============================================================================

/// Tests shutdown progress display as it would appear during App shutdown.
#[test]
#[serial]
fn test_shutdown_lifecycle() {
    with_clean_env(|| {
        set_env("FASTAPI_OUTPUT_MODE", "plain");

        let display = ShutdownProgressDisplay::new(OutputMode::Plain);

        // Simulate graceful shutdown phases
        let phases = vec![
            ShutdownProgress {
                phase: ShutdownPhase::GracePeriod,
                total_connections: 5,
                drained_connections: 0,
                in_flight_requests: 3,
                background_tasks: 2,
                cleanup_done: 0,
                cleanup_total: 3,
                notes: vec!["Starting graceful shutdown".to_string()],
            },
            ShutdownProgress {
                phase: ShutdownPhase::GracePeriod,
                total_connections: 5,
                drained_connections: 2,
                in_flight_requests: 2,
                background_tasks: 2,
                cleanup_done: 0,
                cleanup_total: 3,
                notes: vec![],
            },
            ShutdownProgress {
                phase: ShutdownPhase::GracePeriod,
                total_connections: 5,
                drained_connections: 4,
                in_flight_requests: 1,
                background_tasks: 1,
                cleanup_done: 1,
                cleanup_total: 3,
                notes: vec![],
            },
            ShutdownProgress {
                phase: ShutdownPhase::ForceClose,
                total_connections: 5,
                drained_connections: 5,
                in_flight_requests: 0,
                background_tasks: 0,
                cleanup_done: 2,
                cleanup_total: 3,
                notes: vec!["Force-closing remaining resources".to_string()],
            },
            ShutdownProgress {
                phase: ShutdownPhase::Complete,
                total_connections: 5,
                drained_connections: 5,
                in_flight_requests: 0,
                background_tasks: 0,
                cleanup_done: 3,
                cleanup_total: 3,
                notes: vec!["Shutdown complete".to_string()],
            },
        ];

        for progress in &phases {
            let output = display.render(progress);

            assert_no_ansi(&output);

            // Should show current phase
            match progress.phase {
                ShutdownPhase::GracePeriod => {
                    assert_contains(&output, "Grace");
                }
                ShutdownPhase::ForceClose => {
                    assert_contains(&output, "Force");
                }
                ShutdownPhase::Complete => {
                    assert_contains(&output, "Complete");
                }
            }
        }
    });
}

// =============================================================================
// Middleware Stack Display Integration Tests
// =============================================================================

/// Tests middleware stack visualization as it would appear during App startup.
#[test]
#[serial]
fn test_middleware_stack_display() {
    with_clean_env(|| {
        set_env("FASTAPI_OUTPUT_MODE", "plain");

        // Simulate typical middleware stack using constructor
        let middleware = vec![
            MiddlewareInfo::new("RequestLogger", 1).with_type_name("LoggingMiddleware"),
            MiddlewareInfo::new("CORS", 2)
                .with_type_name("CorsMiddleware")
                .short_circuits(),
            MiddlewareInfo::new("Auth", 3)
                .with_type_name("AuthMiddleware")
                .short_circuits(),
            MiddlewareInfo::new("RateLimiter", 4)
                .with_type_name("RateLimitMiddleware")
                .short_circuits(),
        ];

        let display = MiddlewareStackDisplay::new(middleware.clone());
        let output = display.as_plain_text();

        assert_no_ansi(&output);

        // Should show all middleware names
        for mw in &middleware {
            assert_contains(&output, &mw.name);
        }

        // Should indicate execution order
        assert_contains(&output, "RequestLogger");
        assert_contains(&output, "RateLimiter");
    });
}

// =============================================================================
// Dependency Tree Display Integration Tests
// =============================================================================

/// Tests dependency tree visualization as it would appear for DI debugging.
#[test]
#[serial]
fn test_dependency_tree_display() {
    with_clean_env(|| {
        set_env("FASTAPI_OUTPUT_MODE", "plain");

        // Simulate dependency injection tree using constructors
        let root = DependencyNode::new("UserService")
            .scope("request")
            .cached()
            .children(vec![
                DependencyNode::new("DbPool").scope("singleton").cached(),
                DependencyNode::new("CacheClient")
                    .scope("singleton")
                    .cached(),
            ]);

        let display = DependencyTreeDisplay::new(OutputMode::Plain, vec![root]);
        let output = display.render();

        assert_no_ansi(&output);
        assert_contains(&output, "UserService");
        assert_contains(&output, "DbPool");
        assert_contains(&output, "CacheClient");
    });
}

// =============================================================================
// Mode Consistency Tests
// =============================================================================

/// Verifies that mode selection is consistent throughout the App lifecycle.
#[test]
#[serial]
fn test_mode_consistency_across_components() {
    with_clean_env(|| {
        set_env("CLAUDE_CODE", "1");

        // All components should detect agent mode
        let output = RichOutput::auto();
        assert_eq!(output.mode(), OutputMode::Plain);

        let banner = Banner::new(output.mode());
        let route_display = RouteDisplay::new(output.mode());
        let logger = RequestLogger::new(output.mode());
        let error_formatter = ErrorFormatter::new(output.mode());
        let shutdown_display = ShutdownProgressDisplay::new(output.mode());

        // Verify banner output is plain
        let server_info = ServerInfo::new("0.1.0", "localhost", 8000);
        assert_no_ansi(&banner.render(&server_info));

        // Verify route output is plain
        let routes = vec![RouteEntry {
            method: "GET".to_string(),
            path: "/".to_string(),
            handler: Some("root".to_string()),
            tags: vec![],
            deprecated: false,
        }];
        assert_no_ansi(&route_display.render(&routes));

        // Verify log output is plain
        let log_entry = LogEntry {
            method: HttpMethod::Get,
            path: "/".to_string(),
            query: None,
            status: 200,
            timing: Some(ResponseTiming::new(Duration::from_millis(10))),
            client_ip: None,
            request_id: None,
        };
        assert_no_ansi(&logger.format(&log_entry));

        // Verify error output is plain
        let error = HttpErrorInfo {
            status: 404,
            detail: "Not Found".to_string(),
            code: None,
            path: None,
            method: None,
        };
        assert_no_ansi(&error_formatter.format_http_error(&error).plain);

        // Verify shutdown output is plain
        let shutdown = ShutdownProgress {
            phase: ShutdownPhase::Complete,
            total_connections: 0,
            drained_connections: 0,
            in_flight_requests: 0,
            background_tasks: 0,
            cleanup_done: 0,
            cleanup_total: 0,
            notes: vec![],
        };
        assert_no_ansi(&shutdown_display.render(&shutdown));
    });
}

/// Tests that FASTAPI_HUMAN_MODE can override agent detection.
#[test]
#[serial]
fn test_human_mode_override_in_agent_env() {
    with_clean_env(|| {
        // Set both agent env and human mode override
        set_env("CLAUDE_CODE", "1");
        set_env("FASTAPI_HUMAN_MODE", "1");

        let output = RichOutput::auto();

        // Human mode should win over agent detection
        assert_eq!(output.mode(), OutputMode::Rich);

        // Components should use rich mode
        let banner = Banner::new(output.mode());
        let server_info = ServerInfo::new("0.1.0", "localhost", 8000);
        let banner_output = banner.render(&server_info);

        assert_has_ansi(&banner_output);
    });
}
