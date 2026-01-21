# Configuration

Configure your fastapi_rust application using `AppConfig`.

## AppConfig

```rust
use fastapi::core::{App, AppConfig};

let config = AppConfig::new()
    .name("My API")
    .version("1.0.0")
    .debug(true)
    .max_body_size(10 * 1024 * 1024)  // 10 MB
    .request_timeout_ms(30_000);       // 30 seconds

let app = App::builder()
    .config(config)
    .get("/", handler)
    .build();
```

## Configuration Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `name` | String | "fastapi" | Application name |
| `version` | String | "0.1.0" | API version |
| `debug` | bool | false | Enable debug mode |
| `max_body_size` | usize | 1MB | Maximum request body size |
| `request_timeout_ms` | u64 | 30000 | Request timeout in milliseconds |

## Accessing Configuration

Access configuration from the App:

```rust
let app = App::builder()
    .config(AppConfig::new().name("My API"))
    .build();

println!("App: {}", app.config().name);
println!("Version: {}", app.config().version);
println!("Debug: {}", app.config().debug);
```

## Environment-Based Configuration

Load configuration from environment:

```rust
use std::env;

fn load_config() -> AppConfig {
    let debug = env::var("DEBUG")
        .map(|v| v == "true")
        .unwrap_or(false);

    let max_body = env::var("MAX_BODY_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1024 * 1024);

    AppConfig::new()
        .name(env::var("APP_NAME").unwrap_or_else(|_| "API".into()))
        .debug(debug)
        .max_body_size(max_body)
}

let app = App::builder()
    .config(load_config())
    .build();
```

## Application State

Add typed state to your application:

```rust
struct DatabasePool {
    connection_string: String,
}

struct Config {
    api_key: String,
}

let app = App::builder()
    .state(DatabasePool {
        connection_string: "postgres://localhost/db".into()
    })
    .state(Config {
        api_key: "secret".into()
    })
    .get("/", handler)
    .build();

// Access state later
let pool = app.get_state::<DatabasePool>();
let config = app.get_state::<Config>();
```

## Startup and Shutdown Hooks

Run code at application lifecycle events:

```rust
let app = App::builder()
    .on_startup(|| {
        println!("Application starting...");
        // Initialize resources
    })
    .on_shutdown(|| {
        println!("Application stopping...");
        // Cleanup resources
    })
    .build();
```

Async hooks:

```rust
let app = App::builder()
    .on_startup_async(|| Box::pin(async {
        // Async initialization
        Ok(())
    }))
    .build();
```

## Next Steps

- [Routing](routing.md) - Define your API routes
- [Deployment](deployment.md) - Production configuration
