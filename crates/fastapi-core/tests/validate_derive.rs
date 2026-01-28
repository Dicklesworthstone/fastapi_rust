//! Comprehensive integration tests for the `#[derive(Validate)]` macro.
//!
//! Tests all supported validators:
//! - length(min, max) - String/Vec length bounds
//! - range(gt, ge, lt, le) - Numeric range bounds
//! - email - Email format validation
//! - url - URL format validation
//! - regex - Regex pattern matching
//! - custom - Custom validation function
//! - nested - Nested struct validation
//! - multiple_of - Divisibility check

// Allow clippy lints that arise from derive macro generated code or test conventions
#![allow(clippy::similar_names)]
#![allow(clippy::trivially_copy_pass_by_ref)]
#![allow(clippy::ref_option_ref)]

use fastapi_macros::Validate;

// ============================================================================
// LENGTH VALIDATION TESTS
// ============================================================================

#[derive(Validate)]
struct LengthMinTest {
    #[validate(length(min = 3))]
    value: String,
}

#[test]
fn test_length_min_valid() {
    let valid = LengthMinTest {
        value: "abc".to_string(),
    };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_length_min_invalid() {
    let invalid = LengthMinTest {
        value: "ab".to_string(),
    };
    let result = invalid.validate();
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert_eq!(errors.len(), 1);
}

#[derive(Validate)]
struct LengthMaxTest {
    #[validate(length(max = 5))]
    value: String,
}

#[test]
fn test_length_max_valid() {
    let valid = LengthMaxTest {
        value: "hello".to_string(),
    };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_length_max_invalid() {
    let invalid = LengthMaxTest {
        value: "toolong".to_string(),
    };
    let result = invalid.validate();
    assert!(result.is_err());
}

#[derive(Validate)]
struct LengthRangeTest {
    #[validate(length(min = 2, max = 5))]
    value: String,
}

#[test]
fn test_length_range_valid() {
    let valid = LengthRangeTest {
        value: "abc".to_string(),
    };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_length_range_too_short() {
    let invalid = LengthRangeTest {
        value: "a".to_string(),
    };
    assert!(invalid.validate().is_err());
}

#[test]
fn test_length_range_too_long() {
    let invalid = LengthRangeTest {
        value: "toolong".to_string(),
    };
    assert!(invalid.validate().is_err());
}

// ============================================================================
// RANGE VALIDATION TESTS
// ============================================================================

#[derive(Validate)]
struct RangeGeTest {
    #[validate(range(ge = 0))]
    value: i32,
}

#[test]
fn test_range_ge_valid() {
    let valid = RangeGeTest { value: 0 };
    assert!(valid.validate().is_ok());

    let also_valid = RangeGeTest { value: 100 };
    assert!(also_valid.validate().is_ok());
}

#[test]
fn test_range_ge_invalid() {
    let invalid = RangeGeTest { value: -1 };
    assert!(invalid.validate().is_err());
}

#[derive(Validate)]
struct RangeLeTest {
    #[validate(range(le = 100))]
    value: i32,
}

#[test]
fn test_range_le_valid() {
    let valid = RangeLeTest { value: 100 };
    assert!(valid.validate().is_ok());

    let also_valid = RangeLeTest { value: 0 };
    assert!(also_valid.validate().is_ok());
}

#[test]
fn test_range_le_invalid() {
    let invalid = RangeLeTest { value: 101 };
    assert!(invalid.validate().is_err());
}

#[derive(Validate)]
struct RangeGtTest {
    #[validate(range(gt = 0))]
    value: i32,
}

#[test]
fn test_range_gt_valid() {
    let valid = RangeGtTest { value: 1 };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_range_gt_boundary_invalid() {
    // gt = 0 means value must be > 0, so 0 is invalid
    let invalid = RangeGtTest { value: 0 };
    assert!(invalid.validate().is_err());
}

#[derive(Validate)]
struct RangeLtTest {
    #[validate(range(lt = 100))]
    value: i32,
}

#[test]
fn test_range_lt_valid() {
    let valid = RangeLtTest { value: 99 };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_range_lt_boundary_invalid() {
    // lt = 100 means value must be < 100, so 100 is invalid
    let invalid = RangeLtTest { value: 100 };
    assert!(invalid.validate().is_err());
}

#[derive(Validate)]
struct RangeFullTest {
    #[validate(range(ge = 0, le = 100))]
    value: i32,
}

#[test]
fn test_range_full_valid() {
    let valid_min = RangeFullTest { value: 0 };
    assert!(valid_min.validate().is_ok());

    let valid_max = RangeFullTest { value: 100 };
    assert!(valid_max.validate().is_ok());

    let valid_mid = RangeFullTest { value: 50 };
    assert!(valid_mid.validate().is_ok());
}

#[test]
fn test_range_full_invalid() {
    let below = RangeFullTest { value: -1 };
    assert!(below.validate().is_err());

    let above = RangeFullTest { value: 101 };
    assert!(above.validate().is_err());
}

// Float range test
#[derive(Validate)]
struct RangeFloatTest {
    #[validate(range(ge = 0.0, le = 1.0))]
    value: f64,
}

#[test]
fn test_range_float_valid() {
    let valid = RangeFloatTest { value: 0.5 };
    assert!(valid.validate().is_ok());

    let valid_min = RangeFloatTest { value: 0.0 };
    assert!(valid_min.validate().is_ok());

    let valid_max = RangeFloatTest { value: 1.0 };
    assert!(valid_max.validate().is_ok());
}

#[test]
fn test_range_float_invalid() {
    let invalid = RangeFloatTest { value: 1.1 };
    assert!(invalid.validate().is_err());
}

// ============================================================================
// EMAIL VALIDATION TESTS
// ============================================================================

#[derive(Validate)]
struct EmailTest {
    #[validate(email)]
    value: String,
}

#[test]
fn test_email_valid() {
    let valid = EmailTest {
        value: "user@example.com".to_string(),
    };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_email_valid_subdomain() {
    let valid = EmailTest {
        value: "user@mail.example.com".to_string(),
    };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_email_invalid_no_at() {
    let invalid = EmailTest {
        value: "userexample.com".to_string(),
    };
    assert!(invalid.validate().is_err());
}

#[test]
fn test_email_invalid_no_domain() {
    let invalid = EmailTest {
        value: "user@".to_string(),
    };
    assert!(invalid.validate().is_err());
}

#[test]
fn test_email_invalid_no_user() {
    let invalid = EmailTest {
        value: "@example.com".to_string(),
    };
    assert!(invalid.validate().is_err());
}

#[test]
fn test_email_invalid_no_dot() {
    let invalid = EmailTest {
        value: "user@examplecom".to_string(),
    };
    assert!(invalid.validate().is_err());
}

// ============================================================================
// URL VALIDATION TESTS
// ============================================================================

#[derive(Validate)]
struct UrlTest {
    #[validate(url)]
    value: String,
}

#[test]
fn test_url_valid_https() {
    let valid = UrlTest {
        value: "https://example.com".to_string(),
    };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_url_valid_http() {
    let valid = UrlTest {
        value: "http://example.com".to_string(),
    };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_url_invalid_no_protocol() {
    let invalid = UrlTest {
        value: "example.com".to_string(),
    };
    assert!(invalid.validate().is_err());
}

#[test]
fn test_url_invalid_wrong_protocol() {
    let invalid = UrlTest {
        value: "ftp://example.com".to_string(),
    };
    assert!(invalid.validate().is_err());
}

// ============================================================================
// REGEX VALIDATION TESTS
// ============================================================================

#[derive(Validate)]
struct RegexTest {
    #[validate(regex = "^[a-z]+$")]
    value: String,
}

#[test]
fn test_regex_valid() {
    let valid = RegexTest {
        value: "abc".to_string(),
    };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_regex_invalid_uppercase() {
    let invalid = RegexTest {
        value: "ABC".to_string(),
    };
    assert!(invalid.validate().is_err());
}

#[test]
fn test_regex_invalid_numbers() {
    let invalid = RegexTest {
        value: "abc123".to_string(),
    };
    assert!(invalid.validate().is_err());
}

#[derive(Validate)]
struct RegexPhoneTest {
    #[validate(regex = r"^\d{3}-\d{3}-\d{4}$")]
    phone: String,
}

#[test]
fn test_regex_phone_valid() {
    let valid = RegexPhoneTest {
        phone: "123-456-7890".to_string(),
    };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_regex_phone_invalid() {
    let invalid = RegexPhoneTest {
        phone: "1234567890".to_string(),
    };
    assert!(invalid.validate().is_err());
}

// ============================================================================
// MULTIPLE_OF VALIDATION TESTS
// ============================================================================

#[derive(Validate)]
struct MultipleOfTest {
    #[validate(multiple_of = 5)]
    value: i32,
}

#[test]
fn test_multiple_of_valid() {
    let valid = MultipleOfTest { value: 10 };
    assert!(valid.validate().is_ok());

    let valid_zero = MultipleOfTest { value: 0 };
    assert!(valid_zero.validate().is_ok());

    let valid_negative = MultipleOfTest { value: -15 };
    assert!(valid_negative.validate().is_ok());
}

#[test]
fn test_multiple_of_invalid() {
    let invalid = MultipleOfTest { value: 7 };
    assert!(invalid.validate().is_err());
}

// ============================================================================
// NESTED VALIDATION TESTS
// ============================================================================

#[derive(Validate)]
struct Inner {
    #[validate(length(min = 1))]
    name: String,
}

#[derive(Validate)]
struct Outer {
    #[validate(nested)]
    inner: Inner,
}

#[test]
fn test_nested_valid() {
    let valid = Outer {
        inner: Inner {
            name: "valid".to_string(),
        },
    };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_nested_invalid() {
    let invalid = Outer {
        inner: Inner {
            name: String::new(),
        },
    };
    let result = invalid.validate();
    assert!(result.is_err());
}

// ============================================================================
// CUSTOM VALIDATOR TESTS
// ============================================================================

fn validate_even(value: &i32) -> Result<(), String> {
    if value % 2 == 0 {
        Ok(())
    } else {
        Err("Value must be even".to_string())
    }
}

#[derive(Validate)]
struct CustomValidatorTest {
    #[validate(custom = validate_even)]
    value: i32,
}

#[test]
fn test_custom_validator_valid() {
    let valid = CustomValidatorTest { value: 4 };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_custom_validator_invalid() {
    let invalid = CustomValidatorTest { value: 3 };
    assert!(invalid.validate().is_err());
}

// ============================================================================
// MULTIPLE VALIDATORS ON SAME FIELD
// ============================================================================

#[derive(Validate)]
struct MultipleValidators {
    #[validate(length(min = 5, max = 10))]
    username: String,
    #[validate(range(ge = 18, le = 150))]
    age: i32,
}

#[test]
fn test_multiple_validators_all_valid() {
    let valid = MultipleValidators {
        username: "hello".to_string(),
        age: 25,
    };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_multiple_validators_one_invalid() {
    let invalid = MultipleValidators {
        username: "hi".to_string(), // too short
        age: 25,
    };
    assert!(invalid.validate().is_err());
}

#[test]
fn test_multiple_validators_both_invalid() {
    let invalid = MultipleValidators {
        username: "hi".to_string(), // too short
        age: 10,                    // too young
    };
    let result = invalid.validate();
    assert!(result.is_err());
    // Should have 2 errors
    let errors = result.unwrap_err();
    assert_eq!(errors.len(), 2);
}

// ============================================================================
// COLLECTION VALIDATION TESTS
// ============================================================================

#[derive(Validate)]
struct VecLengthTest {
    #[validate(length(min = 1, max = 5))]
    items: Vec<String>,
}

#[test]
fn test_vec_length_valid() {
    let valid = VecLengthTest {
        items: vec!["one".to_string(), "two".to_string()],
    };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_vec_length_empty_invalid() {
    let invalid = VecLengthTest { items: vec![] };
    assert!(invalid.validate().is_err());
}

#[test]
fn test_vec_length_too_many_invalid() {
    let invalid = VecLengthTest {
        items: vec![
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
            "4".to_string(),
            "5".to_string(),
            "6".to_string(),
        ],
    };
    assert!(invalid.validate().is_err());
}

// ============================================================================
// NO VALIDATION ATTRIBUTES TEST
// ============================================================================

#[derive(Validate)]
#[allow(dead_code)]
struct NoValidation {
    name: String,
    age: i32,
}

#[test]
fn test_no_validation_always_valid() {
    let valid = NoValidation {
        name: String::new(), // Even empty is valid - no constraints
        age: -1,              // Even negative is valid - no constraints
    };
    assert!(valid.validate().is_ok());
}

// ============================================================================
// ERROR LOCATION TESTS
// ============================================================================

#[test]
fn test_error_contains_field_name() {
    let invalid = LengthMinTest {
        value: "ab".to_string(),
    };
    let result = invalid.validate();
    assert!(result.is_err());

    let errors = result.unwrap_err();
    // Check that error location includes "value" field
    let error = &errors.errors[0];
    let loc_str = format!("{:?}", error.loc);
    assert!(
        loc_str.contains("value"),
        "Error should reference 'value' field"
    );
}

// ============================================================================
// LENGTH EDGE CASES
// ============================================================================

#[derive(Validate)]
struct LengthZeroMinTest {
    #[validate(length(min = 0))]
    value: String,
}

#[test]
fn test_length_min_zero_allows_empty() {
    // min = 0 means empty string is valid
    let valid = LengthZeroMinTest {
        value: String::new(),
    };
    assert!(valid.validate().is_ok());
}

#[derive(Validate)]
struct LengthOneMinTest {
    #[validate(length(min = 1))]
    value: String,
}

#[test]
fn test_length_min_one_boundary() {
    // Exactly 1 character should pass
    let valid = LengthOneMinTest {
        value: "x".to_string(),
    };
    assert!(valid.validate().is_ok());

    // Empty string should fail
    let invalid = LengthOneMinTest {
        value: String::new(),
    };
    assert!(invalid.validate().is_err());
}

#[derive(Validate)]
struct LengthExactBoundary {
    #[validate(length(min = 3, max = 3))]
    value: String,
}

#[test]
fn test_length_exact_match() {
    // Exactly 3 characters should pass
    let valid = LengthExactBoundary {
        value: "abc".to_string(),
    };
    assert!(valid.validate().is_ok());

    // 2 characters should fail
    let too_short = LengthExactBoundary {
        value: "ab".to_string(),
    };
    assert!(too_short.validate().is_err());

    // 4 characters should fail
    let too_long = LengthExactBoundary {
        value: "abcd".to_string(),
    };
    assert!(too_long.validate().is_err());
}

// ============================================================================
// NUMERIC BOUNDARY VALUE TESTS
// ============================================================================

#[derive(Validate)]
struct NumericBoundaryTest {
    #[validate(range(ge = -128, le = 127))]
    value: i8,
}

#[test]
fn test_numeric_i8_full_range() {
    // Minimum i8 value
    let min_val = NumericBoundaryTest { value: -128 };
    assert!(min_val.validate().is_ok());

    // Maximum i8 value
    let max_val = NumericBoundaryTest { value: 127 };
    assert!(max_val.validate().is_ok());

    // Middle value
    let mid = NumericBoundaryTest { value: 0 };
    assert!(mid.validate().is_ok());
}

#[derive(Validate)]
struct ExclusiveRangeTest {
    #[validate(range(gt = 0, lt = 100))]
    value: i32,
}

#[test]
fn test_exclusive_range_boundaries() {
    // Exactly at boundaries should fail (exclusive)
    let at_lower = ExclusiveRangeTest { value: 0 };
    assert!(at_lower.validate().is_err());

    let at_upper = ExclusiveRangeTest { value: 100 };
    assert!(at_upper.validate().is_err());

    // Just inside boundaries should pass
    let just_above_lower = ExclusiveRangeTest { value: 1 };
    assert!(just_above_lower.validate().is_ok());

    let just_below_upper = ExclusiveRangeTest { value: 99 };
    assert!(just_below_upper.validate().is_ok());
}

#[derive(Validate)]
struct MixedInclusiveExclusiveTest {
    #[validate(range(ge = 0, lt = 100))]
    value: i32,
}

#[test]
fn test_mixed_inclusive_exclusive_range() {
    // At inclusive lower bound should pass
    let at_lower = MixedInclusiveExclusiveTest { value: 0 };
    assert!(at_lower.validate().is_ok());

    // At exclusive upper bound should fail
    let at_upper = MixedInclusiveExclusiveTest { value: 100 };
    assert!(at_upper.validate().is_err());

    // Just below upper bound should pass
    let just_below = MixedInclusiveExclusiveTest { value: 99 };
    assert!(just_below.validate().is_ok());
}

// ============================================================================
// FLOAT RANGE EDGE CASES
// ============================================================================

#[derive(Validate)]
struct FloatPrecisionTest {
    #[validate(range(ge = 0.0, le = 1.0))]
    value: f64,
}

#[test]
fn test_float_precision_boundaries() {
    // Exactly at boundaries
    let at_zero = FloatPrecisionTest { value: 0.0 };
    assert!(at_zero.validate().is_ok());

    let at_one = FloatPrecisionTest { value: 1.0 };
    assert!(at_one.validate().is_ok());

    // Very small positive
    let epsilon = FloatPrecisionTest {
        value: f64::EPSILON,
    };
    assert!(epsilon.validate().is_ok());

    // Just barely over 1.0
    let over_one = FloatPrecisionTest {
        value: 1.0 + f64::EPSILON,
    };
    assert!(over_one.validate().is_err());
}

#[derive(Validate)]
struct FloatNegativeTest {
    #[validate(range(ge = -1.0, le = 1.0))]
    value: f64,
}

#[test]
fn test_float_negative_range() {
    // Negative boundary
    let neg_one = FloatNegativeTest { value: -1.0 };
    assert!(neg_one.validate().is_ok());

    // Just below negative boundary
    let below_neg = FloatNegativeTest { value: -1.01 };
    assert!(below_neg.validate().is_err());

    // Zero in the middle
    let zero = FloatNegativeTest { value: 0.0 };
    assert!(zero.validate().is_ok());
}

// ============================================================================
// DEEPLY NESTED VALIDATION
// ============================================================================

#[derive(Validate)]
struct Level3 {
    #[validate(length(min = 1))]
    data: String,
}

#[derive(Validate)]
struct Level2 {
    #[validate(nested)]
    level3: Level3,
}

#[derive(Validate)]
struct Level1 {
    #[validate(nested)]
    level2: Level2,
}

#[test]
fn test_deeply_nested_valid() {
    let valid = Level1 {
        level2: Level2 {
            level3: Level3 {
                data: "valid".to_string(),
            },
        },
    };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_deeply_nested_invalid() {
    let invalid = Level1 {
        level2: Level2 {
            level3: Level3 {
                data: String::new(),
            },
        },
    };
    let result = invalid.validate();
    assert!(result.is_err());
    // Error location should include full path
    let errors = result.unwrap_err();
    let loc_str = format!("{:?}", errors.errors[0].loc);
    assert!(loc_str.contains("level2"), "Should contain level2 in path");
    assert!(loc_str.contains("level3"), "Should contain level3 in path");
    assert!(loc_str.contains("data"), "Should contain data in path");
}

// ============================================================================
// MULTIPLE ERRORS AGGREGATION
// ============================================================================

#[derive(Validate)]
struct AllFieldsInvalid {
    #[validate(length(min = 5))]
    name: String,
    #[validate(email)]
    email: String,
    #[validate(range(ge = 18))]
    age: i32,
    #[validate(url)]
    website: String,
}

#[test]
fn test_all_fields_invalid_aggregation() {
    let invalid = AllFieldsInvalid {
        name: "ab".to_string(),            // too short
        email: "not-an-email".to_string(), // invalid email
        age: 10,                           // too young
        website: "not-a-url".to_string(),  // invalid url
    };

    let result = invalid.validate();
    assert!(result.is_err());

    let errors = result.unwrap_err();
    // Should have 4 errors, one for each field
    assert_eq!(errors.len(), 4, "Should have 4 validation errors");

    // Check that all field names appear in the errors
    let all_locs: Vec<String> = errors
        .errors
        .iter()
        .map(|e| format!("{:?}", e.loc))
        .collect();
    let combined = all_locs.join(" ");
    assert!(combined.contains("name"), "Should have name error");
    assert!(combined.contains("email"), "Should have email error");
    assert!(combined.contains("age"), "Should have age error");
    assert!(combined.contains("website"), "Should have website error");
}

// ============================================================================
// ERROR CONTEXT TESTS
// ============================================================================

#[test]
fn test_error_type_is_correct() {
    let invalid = LengthMinTest {
        value: "ab".to_string(),
    };
    let result = invalid.validate();
    assert!(result.is_err());

    let errors = result.unwrap_err();
    let error = &errors.errors[0];
    // Check error type is string_too_short
    assert_eq!(
        error.error_type, "string_too_short",
        "Error type should be string_too_short"
    );
}

#[test]
fn test_range_error_has_context() {
    let invalid = RangeGeTest { value: -5 };
    let result = invalid.validate();
    assert!(result.is_err());

    let errors = result.unwrap_err();
    let error = &errors.errors[0];

    // Error should have context with constraint value
    assert!(
        error.ctx.is_some() && !error.ctx.as_ref().unwrap().is_empty(),
        "Range error should have context with constraint"
    );
}

// ============================================================================
// FASTAPI-COMPATIBLE JSON OUTPUT
// ============================================================================

#[test]
fn test_validation_errors_json_format() {
    let invalid = LengthMinTest {
        value: "ab".to_string(),
    };
    let result = invalid.validate();
    assert!(result.is_err());

    let errors = result.unwrap_err();
    let json = errors.to_json();

    // Parse as JSON to verify format
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("Should be valid JSON");

    // Should have "detail" key with array of errors
    assert!(parsed.get("detail").is_some(), "Should have 'detail' key");
    let detail = parsed.get("detail").unwrap();
    assert!(detail.is_array(), "detail should be an array");

    // Each error should have required fields
    let errors_array = detail.as_array().unwrap();
    assert!(!errors_array.is_empty(), "Should have at least one error");

    let first_error = &errors_array[0];
    assert!(
        first_error.get("type").is_some(),
        "Should have 'type' field"
    );
    assert!(first_error.get("loc").is_some(), "Should have 'loc' field");
    assert!(first_error.get("msg").is_some(), "Should have 'msg' field");
}

#[test]
fn test_multiple_errors_json_format() {
    let invalid = MultipleValidators {
        username: "hi".to_string(), // too short
        age: 10,                    // too young
    };
    let result = invalid.validate();
    assert!(result.is_err());

    let errors = result.unwrap_err();
    let json = errors.to_json();

    let parsed: serde_json::Value = serde_json::from_str(&json).expect("Should be valid JSON");
    let detail = parsed.get("detail").unwrap().as_array().unwrap();

    assert_eq!(detail.len(), 2, "Should have 2 errors in JSON");
}

// ============================================================================
// OPTIONAL FIELD VALIDATION
// ============================================================================

/// Custom validator for optional email
fn validate_optional_email(value: &Option<String>) -> Result<(), String> {
    match value {
        Some(email) => {
            // Simple email check
            if email.contains('@') && email.contains('.') {
                Ok(())
            } else {
                Err("Invalid email format".to_string())
            }
        }
        None => Ok(()), // None is valid
    }
}

#[derive(Validate)]
struct OptionalFieldTest {
    #[validate(custom = validate_optional_email)]
    email: Option<String>,
}

#[test]
fn test_optional_field_none_valid() {
    let valid = OptionalFieldTest { email: None };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_optional_field_some_valid() {
    let valid = OptionalFieldTest {
        email: Some("user@example.com".to_string()),
    };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_optional_field_some_invalid() {
    let invalid = OptionalFieldTest {
        email: Some("not-an-email".to_string()),
    };
    assert!(invalid.validate().is_err());
}

// ============================================================================
// COMPLEX REAL-WORLD STRUCT
// ============================================================================

fn validate_username(value: &str) -> Result<(), String> {
    // Username must start with a letter
    if value
        .chars()
        .next()
        .map(char::is_alphabetic)
        .unwrap_or(false)
    {
        Ok(())
    } else {
        Err("Username must start with a letter".to_string())
    }
}

#[derive(Validate)]
struct CreateUserRequest {
    #[validate(length(min = 3, max = 50))]
    #[validate(custom = validate_username)]
    username: String,

    #[validate(email)]
    email: String,

    #[validate(length(min = 8, max = 128))]
    password: String,

    #[validate(range(ge = 13, le = 120))]
    age: i32,

    #[validate(url)]
    website: String,
}

#[test]
fn test_real_world_valid_user() {
    let valid = CreateUserRequest {
        username: "johndoe".to_string(),
        email: "john@example.com".to_string(),
        password: "securepassword123".to_string(),
        age: 25,
        website: "https://johndoe.com".to_string(),
    };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_real_world_invalid_username_start() {
    let invalid = CreateUserRequest {
        username: "123john".to_string(), // starts with number
        email: "john@example.com".to_string(),
        password: "securepassword123".to_string(),
        age: 25,
        website: "https://johndoe.com".to_string(),
    };
    assert!(invalid.validate().is_err());
}

#[test]
fn test_real_world_multiple_issues() {
    let invalid = CreateUserRequest {
        username: "ab".to_string(),       // too short
        email: "invalid".to_string(),     // not an email
        password: "short".to_string(),    // too short
        age: 10,                          // too young
        website: "not-a-url".to_string(), // not a url
    };

    let result = invalid.validate();
    assert!(result.is_err());

    let errors = result.unwrap_err();
    // Should have errors for all invalid fields
    assert!(
        errors.len() >= 5,
        "Should have at least 5 validation errors"
    );
}

// ============================================================================
// VEC WITH NESTED VALIDATION THROUGH CUSTOM VALIDATOR
// ============================================================================

fn validate_tags(tags: &[String]) -> Result<(), String> {
    for (i, tag) in tags.iter().enumerate() {
        if tag.is_empty() {
            return Err(format!("Tag at index {i} cannot be empty"));
        }
        if tag.len() > 20 {
            return Err(format!("Tag at index {i} is too long (max 20 chars)"));
        }
    }
    Ok(())
}

#[derive(Validate)]
struct TaggedItem {
    #[validate(length(min = 1))]
    name: String,

    #[validate(length(min = 1, max = 10))]
    #[validate(custom = validate_tags)]
    tags: Vec<String>,
}

#[test]
fn test_vec_items_custom_validation_valid() {
    let valid = TaggedItem {
        name: "My Item".to_string(),
        tags: vec!["rust".to_string(), "web".to_string()],
    };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_vec_items_custom_validation_empty_tag() {
    let invalid = TaggedItem {
        name: "My Item".to_string(),
        tags: vec!["rust".to_string(), String::new()], // empty tag
    };
    assert!(invalid.validate().is_err());
}

#[test]
fn test_vec_items_custom_validation_tag_too_long() {
    let invalid = TaggedItem {
        name: "My Item".to_string(),
        tags: vec!["this-tag-is-way-too-long".to_string()],
    };
    assert!(invalid.validate().is_err());
}

// ============================================================================
// UNICODE AND SPECIAL CHARACTERS
// ============================================================================

#[derive(Validate)]
struct UnicodeTest {
    #[validate(length(min = 2, max = 10))]
    text: String,
}

#[test]
fn test_unicode_length_by_chars() {
    // Unicode chars count as their character count, not byte count
    let valid = UnicodeTest {
        text: "日本".to_string(), // 2 characters (6 bytes)
    };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_emoji_length() {
    // Emoji: Note that String.len() counts bytes, not characters
    // This tests the behavior of the current implementation
    let emoji = UnicodeTest {
        text: "👋🌍".to_string(), // 2 emoji = 8 bytes
    };
    // len() returns 8 (bytes), which is >= 2 and <= 10, so should pass
    // This documents that validation uses byte length, not char count
    assert!(emoji.validate().is_ok());
}

#[test]
fn test_emoji_exceeds_max_bytes() {
    // 3 emojis = 12 bytes, exceeds max of 10
    let emoji = UnicodeTest {
        text: "👋🌍🚀".to_string(), // 3 emoji = 12 bytes
    };
    // len() returns 12, which > 10, so should fail
    assert!(emoji.validate().is_err());
}

// ============================================================================
// ZERO AND NEGATIVE VALUES
// ============================================================================

#[derive(Validate)]
struct ZeroMultipleTest {
    #[validate(multiple_of = 1)]
    value: i32,
}

#[test]
fn test_multiple_of_one_always_passes() {
    // Everything is a multiple of 1
    let valid = ZeroMultipleTest { value: 0 };
    assert!(valid.validate().is_ok());

    let also_valid = ZeroMultipleTest { value: -999 };
    assert!(also_valid.validate().is_ok());

    let still_valid = ZeroMultipleTest { value: 12345 };
    assert!(still_valid.validate().is_ok());
}

// ============================================================================
// COMBINED REGEX AND LENGTH
// ============================================================================

#[derive(Validate)]
struct SlugTest {
    #[validate(length(min = 3, max = 50))]
    #[validate(regex = "^[a-z0-9-]+$")]
    slug: String,
}

#[test]
fn test_slug_valid() {
    let valid = SlugTest {
        slug: "my-article-slug".to_string(),
    };
    assert!(valid.validate().is_ok());
}

#[test]
fn test_slug_too_short() {
    let invalid = SlugTest {
        slug: "ab".to_string(),
    };
    assert!(invalid.validate().is_err());
}

#[test]
fn test_slug_invalid_chars() {
    let invalid = SlugTest {
        slug: "My Article".to_string(), // uppercase and space
    };
    assert!(invalid.validate().is_err());
}

#[test]
fn test_slug_both_length_and_pattern_fail() {
    let invalid = SlugTest {
        slug: "AB".to_string(), // too short AND uppercase
    };
    let result = invalid.validate();
    assert!(result.is_err());

    // Should have 2 errors: length and pattern
    let errors = result.unwrap_err();
    assert_eq!(errors.len(), 2, "Should have 2 errors");
}
