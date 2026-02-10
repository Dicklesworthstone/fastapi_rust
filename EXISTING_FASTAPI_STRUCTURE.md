# Existing FastAPI Structure — THE SPEC

> **THIS IS THE SPEC.** After reading this, you should NOT need to consult legacy code.

**Source:** FastAPI v0.128.0
**Extracted:** 2026-01-17

---

## Table of Contents

1. [Parameter System](#1-parameter-system)
2. [Dependency Injection](#2-dependency-injection)
3. [Route Handling](#3-route-handling)
4. [Request Processing Pipeline](#4-request-processing-pipeline)
5. [Response Handling](#5-response-handling)
6. [Validation Rules](#6-validation-rules)
7. [OpenAPI Generation](#7-openapi-generation)
8. [Error Handling](#8-error-handling)
9. [Security Module](#9-security-module)
10. [Application Configuration](#10-application-configuration)
11. [Router Configuration](#11-router-configuration)

---

## 1. Parameter System

### 1.1 Parameter Types Enum

```
ParamTypes:
  - query   → Extracted from URL query string (?key=value)
  - header  → Extracted from HTTP headers
  - path    → Extracted from URL path segments (/items/{id})
  - cookie  → Extracted from cookies
```

### 1.2 Parameter Base Class (Param)

All parameters inherit common validation/metadata fields:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `default` | Any | `Undefined` | Default value if not provided |
| `default_factory` | Callable | None | Factory function for default |
| `alias` | str | None | Alternative name in request (e.g., header `X-Token` → param `x_token`) |
| `validation_alias` | str/AliasPath/AliasChoices | None | Alias used for validation |
| `serialization_alias` | str | None | Alias used for serialization |
| `title` | str | None | OpenAPI title |
| `description` | str | None | OpenAPI description |
| `deprecated` | bool/str | None | Mark as deprecated in OpenAPI |
| `include_in_schema` | bool | `true` | Include in OpenAPI schema |
| `examples` | list[Any] | None | OpenAPI examples |
| `openapi_examples` | dict[str, Example] | None | Named OpenAPI examples |
| `json_schema_extra` | dict | None | Extra JSON Schema properties |

### 1.3 Numeric Validation Constraints

| Constraint | Type | Description |
|------------|------|-------------|
| `gt` | float | Greater than |
| `ge` | float | Greater than or equal |
| `lt` | float | Less than |
| `le` | float | Less than or equal |
| `multiple_of` | float | Must be multiple of value |
| `allow_inf_nan` | bool | Allow infinity/NaN |
| `max_digits` | int | Max decimal digits |
| `decimal_places` | int | Max decimal places |

### 1.4 String Validation Constraints

| Constraint | Type | Description |
|------------|------|-------------|
| `min_length` | int | Minimum string length |
| `max_length` | int | Maximum string length |
| `pattern` | str | Regex pattern to match |
| `strict` | bool | Strict type checking |

### 1.5 Concrete Parameter Classes

#### Path Parameters
- **Class:** `Path`
- **in_:** `ParamTypes.path`
- **Default:** `...` (REQUIRED — must be Ellipsis)
- **Constraint:** Path parameters CANNOT have defaults (assertion enforced)
- **Extracted from:** URL path segments matching `{param_name}` pattern

#### Query Parameters
- **Class:** `Query`
- **in_:** `ParamTypes.query`
- **Default:** `Undefined` (optional)
- **Extracted from:** URL query string `?key=value&key2=value2`

#### Header Parameters
- **Class:** `Header`
- **in_:** `ParamTypes.header`
- **Default:** `Undefined` (optional)
- **Special field:** `convert_underscores: bool = true`
  - When true: `user_agent` → looks for header `User-Agent`
  - Converts underscores to hyphens for header lookup
- **Extracted from:** HTTP request headers

#### Cookie Parameters
- **Class:** `Cookie`
- **in_:** `ParamTypes.cookie`
- **Default:** `Undefined` (optional)
- **Extracted from:** `Cookie` header, parsed as key=value pairs

### 1.6 Body Parameters

#### Body
- **Class:** `Body`
- **NOT a Param subclass** (inherits from `FieldInfo` directly)
- **Special fields:**
  - `embed: bool | None = None` — Force embedding in JSON object with field name as key
  - `media_type: str = "application/json"` — Expected content type

#### Form
- **Class:** `Form` (extends `Body`)
- **media_type:** `"application/x-www-form-urlencoded"`
- **Requires:** `python-multipart` package

#### File
- **Class:** `File` (extends `Form`)
- **media_type:** `"multipart/form-data"`
- **Requires:** `python-multipart` package

### 1.7 Alias Resolution Behavior

When `alias` is provided and others are not:
```
if serialization_alias is None and alias is str:
    serialization_alias = alias
if validation_alias is None:
    validation_alias = alias
```

---

## 2. Dependency Injection

### 2.1 Depends Dataclass

```
@dataclass(frozen=True)
Depends:
    dependency: Callable | None = None   # The callable to invoke
    use_cache: bool = true               # Cache result for request scope
    scope: "function" | "request" | None = None
```

**Scope semantics:**
- `"request"` — Dependency persists for entire request (default for generators)
- `"function"` — Dependency is function-scoped
- `None` — Auto-determined (generators default to "request")

### 2.2 Security Dataclass

```
@dataclass(frozen=True)
Security(Depends):
    scopes: Sequence[str] | None = None  # OAuth2 scopes required
```

### 2.3 Dependant — DI Graph Node

The `Dependant` dataclass represents a node in the dependency graph:

```
@dataclass
Dependant:
    # Parameters by source
    path_params: list[ModelField]
    query_params: list[ModelField]
    header_params: list[ModelField]
    cookie_params: list[ModelField]
    body_params: list[ModelField]

    # Nested dependencies
    dependencies: list[Dependant]

    # Identity
    name: str | None
    call: Callable | None           # The actual function/class
    path: str | None                # Route path pattern

    # Special parameter names (detected by type annotation)
    request_param_name: str | None       # Parameter typed as Request
    websocket_param_name: str | None     # Parameter typed as WebSocket
    http_connection_param_name: str | None
    response_param_name: str | None      # Parameter typed as Response
    background_tasks_param_name: str | None
    security_scopes_param_name: str | None

    # OAuth2 scopes
    own_oauth_scopes: list[str] | None
    parent_oauth_scopes: list[str] | None

    # Caching
    use_cache: bool = true
    scope: "function" | "request" | None
```

### 2.4 Computed Properties of Dependant

| Property | Type | Logic |
|----------|------|-------|
| `oauth_scopes` | list[str] | parent_oauth_scopes + own_oauth_scopes (preserving order) |
| `cache_key` | tuple | `(call, sorted_scopes_tuple, computed_scope)` |
| `is_gen_callable` | bool | True if `call` is a generator function |
| `is_async_gen_callable` | bool | True if `call` is an async generator function |
| `is_coroutine_callable` | bool | True if `call` is a coroutine function |
| `computed_scope` | str\|None | `scope` if set, else `"request"` for generators, else `None` |

### 2.5 Dependency Resolution Algorithm

```
get_dependant(path, call, ...) -> Dependant:
    1. Create Dependant with call, path, scope
    2. Extract path parameter names from path pattern
    3. Get typed signature of call (resolve ForwardRefs)
    4. For each parameter in signature:
        a. If has Depends annotation → recursively create sub-Dependant
        b. Else if type is special (Request, Response, etc.) → set special param name
        c. Else if Body annotation → add to body_params
        d. Else → add to appropriate params list (path/query/header/cookie)
    5. Return Dependant
```

### 2.6 Parameter Type Detection

When no explicit annotation (Path/Query/etc.), detection order:

```
1. If param_name is in path pattern → Path parameter
2. If type is scalar (int, str, float, bool, etc.) → Query parameter
3. If type is BaseModel or complex → Body parameter
```

### 2.7 Special Parameter Types (Auto-Injected)

These types are detected by annotation and auto-injected:

| Type | Injection |
|------|-----------|
| `Request` | Starlette Request object |
| `WebSocket` | Starlette WebSocket object |
| `HTTPConnection` | Starlette HTTPConnection |
| `Response` | Mutable response object |
| `BackgroundTasks` | Task queue for background jobs |
| `SecurityScopes` | OAuth2 scopes from dependency chain |

### 2.8 Dependency Scope Rules

**CRITICAL CONSTRAINT:**
```
If parent dependency has scope="request" (or is a generator),
it CANNOT depend on scope="function" dependencies.

Raises: DependencyScopeError
```

---

## 3. Route Handling

### 3.1 APIRoute Configuration

```
APIRoute:
    path: str                           # URL pattern (e.g., "/items/{item_id}")
    endpoint: Callable                  # Handler function
    methods: set[str]                   # HTTP methods (GET, POST, etc.)

    # Response configuration
    response_model: Any                 # Pydantic model for response validation
    status_code: int | None             # HTTP status code (default varies by method)
    response_class: type[Response]      # Response class (default: JSONResponse)
    response_description: str           # OpenAPI response description
    responses: dict[int|str, dict]      # Additional OpenAPI responses

    # OpenAPI metadata
    tags: list[str | Enum]              # OpenAPI tags
    summary: str | None                 # OpenAPI summary (auto from docstring first line)
    description: str | None             # OpenAPI description (auto from docstring)
    operation_id: str | None            # OpenAPI operationId
    deprecated: bool                    # Mark as deprecated
    include_in_schema: bool             # Include in OpenAPI schema

    # Dependencies
    dependencies: Sequence[Depends]     # Route-level dependencies

    # Response serialization
    response_model_include: set | dict  # Fields to include
    response_model_exclude: set | dict  # Fields to exclude
    response_model_by_alias: bool       # Use alias names
    response_model_exclude_unset: bool  # Exclude unset fields
    response_model_exclude_defaults: bool
    response_model_exclude_none: bool
```

### 3.2 Path Pattern Syntax

```
/items/{item_id}           → item_id is a path parameter
/items/{item_id:int}       → item_id with type converter
/items/{item_id:path}      → item_id matches remaining path (including slashes)
/files/{file_path:path}    → file_path = "a/b/c.txt"
```

**Supported converters:**
- `str` (default)
- `int`
- `float`
- `path` (matches /)
- `uuid`

### 3.3 HTTP Method Decorators

| Decorator | Methods | Default Status |
|-----------|---------|----------------|
| `@app.get` | GET | 200 |
| `@app.post` | POST | 200 |
| `@app.put` | PUT | 200 |
| `@app.delete` | DELETE | 200 |
| `@app.patch` | PATCH | 200 |
| `@app.options` | OPTIONS | 200 |
| `@app.head` | HEAD | 200 |
| `@app.trace` | TRACE | 200 |
| `@app.api_route` | (specified) | 200 |

### 3.4 Route Registration Order

Routes are matched in registration order. First match wins.

**Exception:** Starlette's router checks path specificity. Static paths match before parameter paths at the same level.

---

## 4. Request Processing Pipeline

### 4.1 Pipeline Stages

```
1. ROUTING: Match request path to route
2. PARSE_BODY: Read and parse request body (if needed)
3. SOLVE_DEPENDENCIES: Resolve entire dependency tree
4. VALIDATE: Validate all parameters against types/constraints
5. CALL_ENDPOINT: Invoke handler function
6. SERIALIZE_RESPONSE: Validate and serialize response
7. CLEANUP: Run exit stacks for dependencies with yield
```

### 4.2 Body Parsing Logic

```python
if body_field exists:
    if body is Form:
        body = await request.form()
        # Auto-close form when request completes
    else:
        body_bytes = await request.body()
        if body_bytes:
            content_type = request.headers.get("content-type")
            if content_type is None:
                body = await request.json()
            elif content_type is "application/json" or "+json":
                body = await request.json()
            else:
                body = body_bytes  # Raw bytes
```

### 4.3 solve_dependencies Result

```
@dataclass
SolvedDependency:
    values: dict[str, Any]           # Resolved parameter values
    errors: list[ErrorDict]          # Validation errors
    response: Response               # Mutable response (for setting headers)
    background_tasks: BackgroundTasks
```

### 4.4 Dependency Cache Key

```python
cache_key = (
    call,                              # The callable
    tuple(sorted(set(oauth_scopes))),  # Unique sorted scopes
    computed_scope,                    # "function" | "request" | ""
)
```

---

## 5. Response Handling

### 5.1 Response Serialization

```python
serialize_response(
    field: ModelField,           # Response model field for validation
    response_content: Any,       # Raw return value from endpoint
    include: set | dict,         # Fields to include
    exclude: set | dict,         # Fields to exclude
    by_alias: bool,              # Use field aliases
    exclude_unset: bool,         # Exclude fields not explicitly set
    exclude_defaults: bool,      # Exclude fields with default values
    exclude_none: bool,          # Exclude None values
)
```

### 5.2 Response Type Handling

| Return Type | Behavior |
|-------------|----------|
| `Response` subclass | Return directly (don't wrap) |
| `dict` | Serialize to JSON |
| `BaseModel` | Validate against response_model, serialize |
| `list` | Serialize to JSON array |
| Scalar | Serialize to JSON |

### 5.3 Status Code Rules

```python
# No body allowed for these status codes
NO_BODY_STATUS_CODES = {100, 101, 102, 103, 204, 304}

if status_code in NO_BODY_STATUS_CODES:
    response.body = b""
```

---

## 6. Validation Rules

### 6.1 Numeric Validation

| Rule | Error Type | Condition |
|------|------------|-----------|
| `gt` | `greater_than` | `value > gt` |
| `ge` | `greater_than_equal` | `value >= ge` |
| `lt` | `less_than` | `value < lt` |
| `le` | `less_than_equal` | `value <= le` |
| `multiple_of` | `multiple_of` | `value % multiple_of == 0` |

### 6.2 String Validation

| Rule | Error Type | Condition |
|------|------------|-----------|
| `min_length` | `string_too_short` | `len(value) >= min_length` |
| `max_length` | `string_too_long` | `len(value) <= max_length` |
| `pattern` | `string_pattern_mismatch` | `re.match(pattern, value)` |

### 6.3 Error Format

```json
{
    "type": "validation_error_type",
    "loc": ["body", "field_name"],
    "msg": "Human-readable message",
    "input": "the invalid value",
    "ctx": {
        "constraint_name": "constraint_value"
    }
}
```

### 6.4 Location Tuples

| Source | Location Prefix |
|--------|-----------------|
| Path | `("path", "param_name")` |
| Query | `("query", "param_name")` |
| Header | `("header", "param_name")` |
| Cookie | `("cookie", "param_name")` |
| Body | `("body",)` or `("body", "field", ...)` |
| Response | `("response",)` or `("response", "field", ...)` |

---

## 7. OpenAPI Generation

### 7.1 Schema Generation Flow

```
1. Collect all routes
2. For each route:
   a. Extract path parameters
   b. Extract query parameters
   c. Extract header parameters
   d. Extract body schema
   e. Extract response schemas
   f. Generate operation object
3. Collect all models referenced
4. Generate component schemas
5. Assemble OpenAPI document
```

### 7.2 Operation ID Generation

```python
def generate_unique_id(route: APIRoute) -> str:
    # Default: "{function_name}_{path}_{methods}"
    # Custom via operation_id parameter
    operation_id = route.operation_id
    if operation_id is None:
        operation_id = f"{route.name}_{route.path_format}_{','.join(route.methods)}"
    return operation_id
```

### 7.3 OpenAPI Document Structure

```yaml
openapi: "3.1.0"
info:
  title: str
  version: str
  description: str | null
  termsOfService: str | null
  contact: Contact | null
  license: License | null
servers:
  - url: str
    description: str | null
paths:
  /path:
    get:
      tags: [str]
      summary: str
      description: str
      operationId: str
      parameters: [Parameter]
      requestBody: RequestBody | null
      responses:
        200:
          description: str
          content:
            application/json:
              schema: Schema
components:
  schemas:
    ModelName: Schema
```

---

## 8. Error Handling

### 8.1 Exception Types

| Exception | Status | Use Case |
|-----------|--------|----------|
| `HTTPException` | varies | General HTTP errors |
| `RequestValidationError` | 422 | Request validation failures |
| `ResponseValidationError` | 500 | Response validation failures (internal) |
| `WebSocketRequestValidationError` | 400 | WebSocket validation failures |

### 8.2 HTTPException Structure

```python
HTTPException:
    status_code: int
    detail: Any = None
    headers: dict | None = None
```

### 8.3 RequestValidationError Structure

```python
RequestValidationError:
    errors: list[ErrorDict]
    body: Any              # The invalid request body
    endpoint_ctx: dict     # {file, line, function, path}
```

### 8.4 Default Error Response Format (422)

```json
{
    "detail": [
        {
            "type": "missing",
            "loc": ["query", "q"],
            "msg": "Field required",
            "input": null
        }
    ]
}
```

### 8.5 Endpoint Context (for debugging)

```python
EndpointContext = TypedDict("EndpointContext", {
    "file": str,      # Source file path
    "line": int,      # Line number
    "function": str,  # Function name
    "path": str,      # HTTP method + route path
}, total=False)
```

---

## 9. Security Module

### 9.1 Security Base Class

All security classes inherit from `SecurityBase` which provides:
- OpenAPI integration via `model` attribute
- `scheme_name` for OpenAPI security scheme identification
- `auto_error` behavior (raise HTTPException or return None)

### 9.2 OAuth2 Security

#### OAuth2 Base
```
OAuth2:
    flows: OAuthFlowsModel         # OAuth2 flows configuration
    scheme_name: str | None        # Security scheme name (default: class name)
    description: str | None        # OpenAPI description
    auto_error: bool = true        # Raise exception if auth fails

    __call__(request) -> str | None:
        # Returns full Authorization header value
        # Raises 401 with "WWW-Authenticate: Bearer" if auto_error
```

#### OAuth2PasswordBearer
```
OAuth2PasswordBearer:
    tokenUrl: str                  # REQUIRED - URL to obtain token
    scopes: dict[str, str] | None  # OAuth2 scopes {scope: description}
    refreshUrl: str | None         # Token refresh URL
    scheme_name: str | None
    description: str | None
    auto_error: bool = true

    __call__(request) -> str | None:
        # Validates Authorization: Bearer <token>
        # Returns token (without "Bearer " prefix)
        # Returns None or raises 401 if missing/invalid
```

#### OAuth2AuthorizationCodeBearer
```
OAuth2AuthorizationCodeBearer:
    authorizationUrl: str          # REQUIRED - Authorization endpoint
    tokenUrl: str                  # REQUIRED - Token endpoint
    refreshUrl: str | None
    scopes: dict[str, str] | None
    scheme_name: str | None
    description: str | None
    auto_error: bool = true

    __call__(request) -> str | None:
        # Same extraction as OAuth2PasswordBearer
```

#### OAuth2PasswordRequestForm (Dependency)
```
OAuth2PasswordRequestForm:
    grant_type: str | None = None  # Optional (should be "password")
    username: str                  # REQUIRED
    password: str                  # REQUIRED
    scope: str = ""                # Space-separated scopes
    client_id: str | None = None
    client_secret: str | None = None

    # Computed:
    scopes: list[str]              # scope.split()
```

#### OAuth2PasswordRequestFormStrict
Same as `OAuth2PasswordRequestForm` but `grant_type` is REQUIRED and must be "password".

#### SecurityScopes (Auto-Injected)
```
SecurityScopes:
    scopes: list[str]              # All scopes from dependency chain
    scope_str: str                 # " ".join(scopes)
```

### 9.3 HTTP Authentication

#### HTTPBasicCredentials
```
HTTPBasicCredentials:
    username: str
    password: str
```

#### HTTPAuthorizationCredentials
```
HTTPAuthorizationCredentials:
    scheme: str      # e.g., "Bearer", "Digest"
    credentials: str # The token/credential value
```

#### HTTPBasic
```
HTTPBasic:
    scheme_name: str | None
    realm: str | None              # HTTP Basic realm
    description: str | None
    auto_error: bool = true

    __call__(request) -> HTTPBasicCredentials | None:
        # Validates Authorization: Basic <base64>
        # Decodes base64 → "username:password"
        # Returns HTTPBasicCredentials or raises 401
        # WWW-Authenticate: Basic [realm="..."]
```

#### HTTPBearer
```
HTTPBearer:
    bearerFormat: str | None       # OpenAPI bearer format hint
    scheme_name: str | None
    description: str | None
    auto_error: bool = true

    __call__(request) -> HTTPAuthorizationCredentials | None:
        # Validates Authorization: Bearer <token>
        # Returns {scheme: "Bearer", credentials: <token>}
```

#### HTTPDigest
```
HTTPDigest:
    scheme_name: str | None
    description: str | None
    auto_error: bool = true

    # Note: Stub only - doesn't implement full Digest
    __call__(request) -> HTTPAuthorizationCredentials | None
```

### 9.4 API Key Authentication

#### APIKeyQuery
```
APIKeyQuery:
    name: str                      # REQUIRED - Query parameter name
    scheme_name: str | None
    description: str | None
    auto_error: bool = true

    __call__(request) -> str | None:
        # Extracts request.query_params.get(name)
```

#### APIKeyHeader
```
APIKeyHeader:
    name: str                      # REQUIRED - Header name
    scheme_name: str | None
    description: str | None
    auto_error: bool = true

    __call__(request) -> str | None:
        # Extracts request.headers.get(name)
```

#### APIKeyCookie
```
APIKeyCookie:
    name: str                      # REQUIRED - Cookie name
    scheme_name: str | None
    description: str | None
    auto_error: bool = true

    __call__(request) -> str | None:
        # Extracts request.cookies.get(name)
```

### 9.5 Authorization Header Parsing

```python
def get_authorization_scheme_param(authorization: str | None) -> tuple[str, str]:
    if not authorization:
        return "", ""
    scheme, _, param = authorization.partition(" ")
    return scheme, param
```

### 9.6 Security Error Responses

| Security Type | WWW-Authenticate Header |
|---------------|------------------------|
| OAuth2 | `Bearer` |
| HTTPBasic | `Basic` or `Basic realm="..."` |
| HTTPBearer | `Bearer` |
| HTTPDigest | `Digest` |
| APIKey* | `APIKey` (custom, non-standard) |

---

## 10. Application Configuration

### 10.1 FastAPI Class Parameters

#### Core Settings
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `debug` | bool | `false` | Enable debug tracebacks |
| `title` | str | `"FastAPI"` | API title for OpenAPI |
| `summary` | str\|None | `None` | Short API summary |
| `description` | str | `""` | API description (Markdown) |
| `version` | str | `"0.1.0"` | API version string |

#### OpenAPI Configuration
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `openapi_url` | str\|None | `"/openapi.json"` | OpenAPI schema URL (None disables) |
| `openapi_tags` | list[dict] | `None` | Tag definitions with descriptions |
| `openapi_external_docs` | dict | `None` | External docs link {url, description} |
| `servers` | list[dict] | `None` | Server list [{url, description}] |

#### Documentation UI
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `docs_url` | str\|None | `"/docs"` | Swagger UI URL (None disables) |
| `redoc_url` | str\|None | `"/redoc"` | ReDoc URL (None disables) |
| `swagger_ui_oauth2_redirect_url` | str | `"/docs/oauth2-redirect"` | OAuth2 redirect |
| `swagger_ui_init_oauth` | dict | `None` | Swagger OAuth2 config |
| `swagger_ui_parameters` | dict | `None` | Swagger UI parameters |

#### API Metadata
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `terms_of_service` | str\|None | `None` | Terms of service URL |
| `contact` | dict | `None` | Contact {name, url, email} |
| `license_info` | dict | `None` | License {name, identifier, url} |

#### Routing & Responses
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `redirect_slashes` | bool | `true` | Auto-redirect trailing slashes |
| `default_response_class` | type | `JSONResponse` | Default response class |
| `responses` | dict | `None` | Global additional responses |
| `dependencies` | list[Depends] | `None` | Global dependencies |
| `callbacks` | list[BaseRoute] | `None` | Global OpenAPI callbacks |
| `webhooks` | APIRouter | `None` | Webhook documentation router |

#### Schema Generation
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `separate_input_output_schemas` | bool | `true` | Separate input/output schemas |
| `generate_unique_id_function` | Callable | `generate_unique_id` | Operation ID generator |
| `include_in_schema` | bool | `true` | Include routes in OpenAPI |
| `deprecated` | bool | `None` | Mark all routes deprecated |

#### Lifecycle
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `lifespan` | Lifespan | `None` | Async context manager for startup/shutdown |
| `on_startup` | list[Callable] | `None` | Startup event handlers (deprecated) |
| `on_shutdown` | list[Callable] | `None` | Shutdown event handlers (deprecated) |

#### Middleware & Exceptions
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `middleware` | list[Middleware] | `None` | Middleware stack |
| `exception_handlers` | dict | `None` | Custom exception handlers |

#### Proxy Configuration
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `root_path` | str | `""` | ASGI root path for reverse proxy |
| `root_path_in_servers` | bool | `true` | Add root_path to OpenAPI servers |

### 10.2 Runtime Attributes

| Attribute | Type | Description |
|-----------|------|-------------|
| `state` | State | Application-wide state object |
| `dependency_overrides` | dict | Dependency override mapping for testing |
| `openapi_schema` | dict\|None | Cached OpenAPI schema |
| `openapi_version` | str | OpenAPI version (default "3.1.0") |

### 10.3 Default Exception Handlers

```
exception_handlers = {
    HTTPException: http_exception_handler,
    RequestValidationError: request_validation_exception_handler,
    WebSocketRequestValidationError: websocket_request_validation_exception_handler,
}
```

---

## 11. Router Configuration

### 11.1 APIRouter Parameters

```
APIRouter:
    prefix: str = ""                      # URL prefix for all routes
    tags: list[str | Enum] | None = None  # Default tags
    dependencies: list[Depends] | None    # Default dependencies
    responses: dict | None                # Default additional responses
    callbacks: list[BaseRoute] | None     # OpenAPI callbacks
    redirect_slashes: bool = True         # Slash redirect behavior
    default_response_class: type = Default(JSONResponse)
    deprecated: bool | None = None        # Mark all routes deprecated
    include_in_schema: bool = True        # Include in OpenAPI
    generate_unique_id_function: Callable # Operation ID generator
    route_class: type = APIRoute          # Custom route class
    on_startup: list[Callable] | None     # Startup handlers
    on_shutdown: list[Callable] | None    # Shutdown handlers
    lifespan: Lifespan | None             # Async context manager
```

### 11.2 Router Inclusion (include_router)

```python
app.include_router(
    router,
    prefix="/api",           # Added to router's prefix
    tags=["api"],            # Merged with router's tags
    dependencies=[...],      # Added to router's dependencies
    responses={...},         # Merged with router's responses
    deprecated=False,        # Override router's deprecated
    include_in_schema=True,  # Override router's include_in_schema
    callbacks=[...],         # Merged with router's callbacks
    generate_unique_id_function=...,  # Override
    default_response_class=...,       # Override
)
```

### 11.3 Route Inclusion Order

1. `prefix` is prepended to all route paths
2. `tags` are prepended to route tags
3. `dependencies` are prepended to route dependencies
4. `responses` are merged (route overrides router overrides app)
5. `callbacks` are merged
6. Boolean flags use override if provided, else inherit

### 11.4 Sub-Application Mounting

```python
app.mount("/subapp", subapp)  # Mount at path prefix
```

**Behavior:**
- Sub-app receives requests with path prefix stripped
- Sub-app's OpenAPI is NOT merged into parent
- Independent middleware stacks
- Use `include_router` for OpenAPI integration instead

---

## Appendix A: Behaviors NOT to Implement

Per `PLAN_TO_PORT_FASTAPI_TO_RUST.md`, these are EXCLUDED:

1. **Pydantic v1 compatibility**
2. **Deprecated parameter aliases** (`regex` → use `pattern`)
3. **Bundled Swagger UI/ReDoc HTML**
4. **CLI tooling** (`fastapi dev`, `fastapi run`)
5. **TestClient** (use Rust testing tools)
6. **Multipart handling** (defer to external crate)
7. **BackgroundTasks** (if/when added, use **asupersync** structured concurrency; no detached task runtime)
8. **WebSocket support** (Phase 2)
9. **ASGI middleware bridging** (out of scope; use native middleware stack, no Tower)

---

## Appendix B: Key Type Mappings (Python → Rust)

| Python | Rust |
|--------|------|
| `str` | `String` / `&str` |
| `int` | `i64` / `i32` |
| `float` | `f64` |
| `bool` | `bool` |
| `list[T]` | `Vec<T>` |
| `dict[K, V]` | `HashMap<K, V>` |
| `Optional[T]` | `Option<T>` |
| `Union[A, B]` | `enum { A(A), B(B) }` |
| `Any` | `serde_json::Value` |
| `Callable` | `fn` / `Fn` trait |
| `BaseModel` | `struct` with `#[derive(Serialize, Deserialize)]` |

---

## Appendix C: Validation Constraint Mapping

| FastAPI | Rust Validator | serde Attribute |
|---------|---------------|-----------------|
| `gt=5` | `#[validate(range(min = 6))]` | — |
| `ge=5` | `#[validate(range(min = 5))]` | — |
| `lt=10` | `#[validate(range(max = 9))]` | — |
| `le=10` | `#[validate(range(max = 10))]` | — |
| `min_length=1` | `#[validate(length(min = 1))]` | — |
| `max_length=100` | `#[validate(length(max = 100))]` | — |
| `pattern="^[a-z]+$"` | `#[validate(regex = "...")]` | — |
| `alias="X-Token"` | — | `#[serde(rename = "X-Token")]` |
| `default=42` | — | `#[serde(default = "default_42")]` |

---

*Document version: 2.0*
*Extracted from FastAPI v0.128.0*
*Last updated: 2026-01-17*
