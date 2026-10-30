//! Shared assertions for the contract suites.
//!
//! Float fields are compared with a relative tolerance because two protobuf
//! runtimes print `float` differently: the TypeScript runtime prints the
//! shortest decimal that round-trips to the original `f32`, while
//! `prost-reflect` widens to `f64` and prints the shortest decimal for that.
//! They denote the same IEEE-754 single-precision number. Integer fields are
//! compared exactly; a `float`-typed integer such as
//! `secondsToFirstNonemptySlot` is why the tolerance is relative.

#![allow(dead_code)]

use serde_json::Value;

pub fn assert_json_matches(actual: &Value, expected: &str) {
    let expected: Value = serde_json::from_str(expected).expect("golden JSON");
    assert_json_value_matches(actual, &expected);
}

pub fn assert_json_value_matches(actual: &Value, expected: &Value) {
    compare(actual, expected, "");
}

pub fn compare(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (Value::Object(a), Value::Object(e)) => {
            for (key, value) in e {
                let child = format!("{path}/{key}");
                match a.get(key) {
                    Some(actual_value) => compare(actual_value, value, &child),
                    None => panic!("missing field at {child}"),
                }
            }
            for key in a.keys() {
                assert!(
                    e.contains_key(key),
                    "unexpected field at {path}/{key} in the Rust output"
                );
            }
        }
        (Value::Array(a), Value::Array(e)) => {
            assert_eq!(a.len(), e.len(), "array length at {path}");
            for (index, (actual_item, expected_item)) in a.iter().zip(e).enumerate() {
                compare(actual_item, expected_item, &format!("{path}[{index}]"));
            }
        }
        (Value::Number(a), Value::Number(e)) => {
            if a == e {
                return;
            }
            let (Some(a), Some(e)) = (a.as_f64(), e.as_f64()) else {
                panic!("non-numeric number at {path}");
            };
            assert_close(a, e, path);
        }
        (a, e) => assert_eq!(a, e, "value mismatch at {path}"),
    }
}

pub fn assert_close(actual: f64, expected: f64, path: &str) {
    let scale = actual.abs().max(expected.abs()).max(1.0);
    assert!(
        (actual - expected).abs() <= scale * 1e-6,
        "numeric mismatch at {path}: {actual} vs {expected}"
    );
}
