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
            name: "".to_string(),
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
struct NoValidation {
    name: String,
    age: i32,
}

#[test]
fn test_no_validation_always_valid() {
    let valid = NoValidation {
        name: "".to_string(), // Even empty is valid - no constraints
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
