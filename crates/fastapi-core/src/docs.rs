//! Interactive API documentation endpoints.
//!
//! This module provides handlers for serving Swagger UI and ReDoc
//! documentation interfaces, as well as the OpenAPI JSON specification.
//!
//! # Usage
//!
//! ```ignore
//! use fastapi_core::{App, DocsConfig};
//!
//! let app = App::builder()
//!     .get("/items", list_items)
//!     .post("/items", create_item)
//!     .enable_docs(DocsConfig::default())
//!     .build();
//! ```
//!
//! This will add the following endpoints:
//! - `GET /docs` - Swagger UI interface
//! - `GET /redoc` - ReDoc interface
//! - `GET /openapi.json` - OpenAPI 3.1 specification
//! - `GET /docs/oauth2-redirect` - OAuth2 callback handler
//!
//! # Configuration
//!
//! ```ignore
//! let config = DocsConfig::new()
//!     .docs_path("/api-docs")  // Default: /docs
//!     .redoc_path("/api-redoc")  // Default: /redoc
//!     .openapi_path("/api-spec.json")  // Default: /openapi.json
//!     .title("My API Documentation")
//!     .swagger_ui_parameters(r#"{"docExpansion": "none"}"#);
//! ```

use crate::response::{Response, ResponseBody};

/// Configuration for the API documentation endpoints.
#[derive(Debug, Clone)]
pub struct DocsConfig {
    /// Path for Swagger UI. Set to None to disable.
    pub docs_path: Option<String>,
    /// Path for ReDoc. Set to None to disable.
    pub redoc_path: Option<String>,
    /// Path for the OpenAPI JSON specification.
    pub openapi_path: String,
    /// Title shown in the documentation.
    pub title: String,
    /// Swagger UI configuration parameters (JSON).
    pub swagger_ui_parameters: Option<String>,
    /// Swagger UI OAuth initialization config (JSON).
    pub swagger_ui_init_oauth: Option<String>,
    /// Custom favicon URL.
    pub favicon_url: Option<String>,
    /// CDN base URL for Swagger UI assets.
    pub swagger_cdn_url: String,
    /// CDN base URL for ReDoc assets.
    pub redoc_cdn_url: String,
}

impl Default for DocsConfig {
    fn default() -> Self {
        Self {
            docs_path: Some("/docs".to_string()),
            redoc_path: Some("/redoc".to_string()),
            openapi_path: "/openapi.json".to_string(),
            title: "API Documentation".to_string(),
            swagger_ui_parameters: None,
            swagger_ui_init_oauth: None,
            favicon_url: None,
            swagger_cdn_url: "https://cdn.jsdelivr.net/npm/swagger-ui-dist@5".to_string(),
            redoc_cdn_url: "https://cdn.jsdelivr.net/npm/redoc@latest".to_string(),
        }
    }
}

impl DocsConfig {
    /// Create a new DocsConfig with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the path for Swagger UI. Use None to disable.
    #[must_use]
    pub fn docs_path(mut self, path: impl Into<Option<String>>) -> Self {
        self.docs_path = path.into();
        self
    }

    /// Set the path for ReDoc. Use None to disable.
    #[must_use]
    pub fn redoc_path(mut self, path: impl Into<Option<String>>) -> Self {
        self.redoc_path = path.into();
        self
    }

    /// Set the path for the OpenAPI JSON specification.
    #[must_use]
    pub fn openapi_path(mut self, path: impl Into<String>) -> Self {
        self.openapi_path = path.into();
        self
    }

    /// Set the title shown in documentation.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Set Swagger UI configuration parameters (JSON object).
    ///
    /// # Example
    ///
    /// ```ignore
    /// config.swagger_ui_parameters(r#"{"docExpansion": "none", "filter": true}"#)
    /// ```
    #[must_use]
    pub fn swagger_ui_parameters(mut self, params: impl Into<String>) -> Self {
        self.swagger_ui_parameters = Some(params.into());
        self
    }

    /// Set Swagger UI OAuth initialization config (JSON object).
    ///
    /// # Example
    ///
    /// ```ignore
    /// config.swagger_ui_init_oauth(r#"{"clientId": "my-client-id"}"#)
    /// ```
    #[must_use]
    pub fn swagger_ui_init_oauth(mut self, config: impl Into<String>) -> Self {
        self.swagger_ui_init_oauth = Some(config.into());
        self
    }

    /// Set a custom favicon URL.
    #[must_use]
    pub fn favicon_url(mut self, url: impl Into<String>) -> Self {
        self.favicon_url = Some(url.into());
        self
    }

    /// Set the CDN base URL for Swagger UI assets.
    #[must_use]
    pub fn swagger_cdn_url(mut self, url: impl Into<String>) -> Self {
        self.swagger_cdn_url = url.into();
        self
    }

    /// Set the CDN base URL for ReDoc assets.
    #[must_use]
    pub fn redoc_cdn_url(mut self, url: impl Into<String>) -> Self {
        self.redoc_cdn_url = url.into();
        self
    }
}

/// Generate the Swagger UI HTML page.
///
/// # Arguments
///
/// * `config` - Documentation configuration
/// * `openapi_url` - URL to the OpenAPI JSON specification
#[must_use]
pub fn swagger_ui_html(config: &DocsConfig, openapi_url: &str) -> String {
    let title = html_escape(&config.title);
    let swagger_cdn = &config.swagger_cdn_url;

    let favicon = config.favicon_url.as_ref().map_or_else(
        || format!(r#"<link rel="icon" type="image/png" href="{swagger_cdn}/favicon-32x32.png" sizes="32x32" />"#),
        |url| format!(r#"<link rel="icon" href="{}" />"#, html_escape(url)),
    );

    let ui_parameters = config.swagger_ui_parameters.as_ref().map_or_else(
        || "{}".to_string(),
        String::clone,
    );

    let init_oauth = config.swagger_ui_init_oauth.as_ref().map_or_else(
        String::new,
        |o| format!("ui.initOAuth({});", o),
    );

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{title}</title>
    {favicon}
    <link rel="stylesheet" type="text/css" href="{swagger_cdn}/swagger-ui.css">
</head>
<body>
    <div id="swagger-ui"></div>
    <script src="{swagger_cdn}/swagger-ui-bundle.js"></script>
    <script src="{swagger_cdn}/swagger-ui-standalone-preset.js"></script>
    <script>
        window.onload = function() {{
            const ui = SwaggerUIBundle(Object.assign({{
                url: "{openapi_url}",
                dom_id: '#swagger-ui',
                deepLinking: true,
                presets: [
                    SwaggerUIBundle.presets.apis,
                    SwaggerUIStandalonePreset
                ],
                plugins: [
                    SwaggerUIBundle.plugins.DownloadUrl
                ],
                layout: "StandaloneLayout"
            }}, {ui_parameters}));
            {init_oauth}
            window.ui = ui;
        }};
    </script>
</body>
</html>"#,
        title = title,
        favicon = favicon,
        swagger_cdn = swagger_cdn,
        openapi_url = html_escape(openapi_url),
        ui_parameters = ui_parameters,
        init_oauth = init_oauth,
    )
}

/// Generate the ReDoc HTML page.
///
/// # Arguments
///
/// * `config` - Documentation configuration
/// * `openapi_url` - URL to the OpenAPI JSON specification
#[must_use]
pub fn redoc_html(config: &DocsConfig, openapi_url: &str) -> String {
    let title = html_escape(&config.title);
    let redoc_cdn = &config.redoc_cdn_url;

    let favicon = config.favicon_url.as_ref().map_or_else(
        String::new,
        |url| format!(r#"<link rel="icon" href="{}" />"#, html_escape(url)),
    );

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{title}</title>
    {favicon}
    <style>
        body {{
            margin: 0;
            padding: 0;
        }}
    </style>
</head>
<body>
    <redoc spec-url="{openapi_url}"></redoc>
    <script src="{redoc_cdn}/bundles/redoc.standalone.js"></script>
</body>
</html>"#,
        title = title,
        favicon = favicon,
        openapi_url = html_escape(openapi_url),
        redoc_cdn = redoc_cdn,
    )
}

/// Generate the OAuth2 redirect HTML page.
///
/// This page is used as the callback URL for OAuth2 authorization flows
/// in Swagger UI.
#[must_use]
pub fn oauth2_redirect_html() -> &'static str {
    r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <title>OAuth2 Redirect</title>
</head>
<body>
    <script>
        'use strict';
        function run() {
            var oauth2 = window.opener.swaggerUIRedirectOauth2;
            var sentState = oauth2.state;
            var redirectUrl = oauth2.redirectUrl;
            var isValid, qp, arr;

            if (/code|token|error/.test(window.location.hash)) {
                qp = window.location.hash.substring(1);
            } else {
                qp = window.location.search.substring(1);
            }

            arr = qp.split("&");
            arr.forEach(function(v, i, _arr) { _arr[i] = '"' + v.replace('=', '":"') + '"'; });
            qp = qp ? JSON.parse('{' + arr.join(',') + '}',
                function(key, value) {
                    return key === "" ? value : decodeURIComponent(value);
                }
            ) : {};

            isValid = qp.state === sentState;

            if ((oauth2.auth.schema.get("flow") === "accessCode" ||
                 oauth2.auth.schema.get("flow") === "authorizationCode" ||
                 oauth2.auth.schema.get("flow") === "authorization_code") &&
                !oauth2.auth.code) {
                if (!isValid) {
                    oauth2.errCb({
                        authId: oauth2.auth.name,
                        source: "auth",
                        level: "warning",
                        message: "Authorization may be unsafe, passed state was changed in server. The passed state wasn't returned from auth server."
                    });
                }

                if (qp.code) {
                    delete oauth2.state;
                    oauth2.auth.code = qp.code;
                    oauth2.callback({auth: oauth2.auth, redirectUrl: redirectUrl});
                } else {
                    var oauthErrorMsg;
                    if (qp.error) {
                        oauthErrorMsg = "[" + qp.error + "]: " +
                            (qp.error_description ? qp.error_description + ". " : "no accessCode received from the server. ") +
                            (qp.error_uri ? "More info: " + qp.error_uri : "");
                    }

                    oauth2.errCb({
                        authId: oauth2.auth.name,
                        source: "auth",
                        level: "error",
                        message: oauthErrorMsg || "[Authorization failed]: no accessCode received from the server."
                    });
                }
            } else {
                oauth2.callback({auth: oauth2.auth, token: qp, isValid: isValid, redirectUrl: redirectUrl});
            }
            window.close();
        }

        if (document.readyState !== 'loading') {
            run();
        } else {
            document.addEventListener('DOMContentLoaded', function() {
                run();
            });
        }
    </script>
</body>
</html>"#
}

/// Create a response with Swagger UI HTML.
#[must_use]
pub fn swagger_ui_response(config: &DocsConfig, openapi_url: &str) -> Response {
    let html = swagger_ui_html(config, openapi_url);
    Response::ok()
        .header("content-type", b"text/html; charset=utf-8".to_vec())
        .body(ResponseBody::Bytes(html.into_bytes()))
}

/// Create a response with ReDoc HTML.
#[must_use]
pub fn redoc_response(config: &DocsConfig, openapi_url: &str) -> Response {
    let html = redoc_html(config, openapi_url);
    Response::ok()
        .header("content-type", b"text/html; charset=utf-8".to_vec())
        .body(ResponseBody::Bytes(html.into_bytes()))
}

/// Create a response with OAuth2 redirect HTML.
#[must_use]
pub fn oauth2_redirect_response() -> Response {
    Response::ok()
        .header("content-type", b"text/html; charset=utf-8".to_vec())
        .body(ResponseBody::Bytes(oauth2_redirect_html().as_bytes().to_vec()))
}

/// Simple HTML escaping for attribute values.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = DocsConfig::default();
        assert_eq!(config.docs_path, Some("/docs".to_string()));
        assert_eq!(config.redoc_path, Some("/redoc".to_string()));
        assert_eq!(config.openapi_path, "/openapi.json");
        assert_eq!(config.title, "API Documentation");
    }

    #[test]
    fn test_config_builder() {
        let config = DocsConfig::new()
            .docs_path(Some("/api-docs".to_string()))
            .redoc_path(None::<String>)
            .openapi_path("/spec.json")
            .title("My API")
            .swagger_ui_parameters(r#"{"docExpansion": "none"}"#)
            .swagger_ui_init_oauth(r#"{"clientId": "test"}"#);

        assert_eq!(config.docs_path, Some("/api-docs".to_string()));
        assert_eq!(config.redoc_path, None);
        assert_eq!(config.openapi_path, "/spec.json");
        assert_eq!(config.title, "My API");
        assert!(config.swagger_ui_parameters.is_some());
        assert!(config.swagger_ui_init_oauth.is_some());
    }

    #[test]
    fn test_swagger_ui_html() {
        let config = DocsConfig::new().title("Test API");
        let html = swagger_ui_html(&config, "/openapi.json");

        assert!(html.contains("<title>Test API</title>"));
        assert!(html.contains("swagger-ui-bundle.js"));
        assert!(html.contains("url: \"/openapi.json\""));
    }

    #[test]
    fn test_redoc_html() {
        let config = DocsConfig::new().title("Test API");
        let html = redoc_html(&config, "/openapi.json");

        assert!(html.contains("<title>Test API</title>"));
        assert!(html.contains("redoc.standalone.js"));
        assert!(html.contains("spec-url=\"/openapi.json\""));
    }

    #[test]
    fn test_oauth2_redirect_html() {
        let html = oauth2_redirect_html();
        assert!(html.contains("OAuth2 Redirect"));
        assert!(html.contains("swaggerUIRedirectOauth2"));
    }

    #[test]
    fn test_html_escape() {
        assert_eq!(html_escape("<script>"), "&lt;script&gt;");
        assert_eq!(html_escape("a&b"), "a&amp;b");
        assert_eq!(html_escape("\"test\""), "&quot;test&quot;");
    }

    #[test]
    fn test_swagger_ui_with_custom_params() {
        let config = DocsConfig::new()
            .swagger_ui_parameters(r#"{"filter": true}"#);
        let html = swagger_ui_html(&config, "/openapi.json");

        assert!(html.contains(r#"{"filter": true}"#));
    }

    #[test]
    fn test_swagger_ui_with_oauth() {
        let config = DocsConfig::new()
            .swagger_ui_init_oauth(r#"{"clientId": "my-app"}"#);
        let html = swagger_ui_html(&config, "/openapi.json");

        assert!(html.contains(r#"ui.initOAuth({"clientId": "my-app"});"#));
    }

    #[test]
    fn test_custom_cdn_urls() {
        let config = DocsConfig::new()
            .swagger_cdn_url("https://custom.cdn/swagger")
            .redoc_cdn_url("https://custom.cdn/redoc");

        let swagger_html = swagger_ui_html(&config, "/spec.json");
        let redoc_html = redoc_html(&config, "/spec.json");

        assert!(swagger_html.contains("https://custom.cdn/swagger"));
        assert!(redoc_html.contains("https://custom.cdn/redoc"));
    }
}
