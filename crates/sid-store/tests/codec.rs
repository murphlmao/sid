use proptest::prelude::*;
use serde::{Deserialize, Serialize};
use sid_store::codec::{decode_versioned, encode_versioned};
use sid_store::SessionRecord;

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct ExampleV1 {
    a: u32,
    b: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct SimpleBytes {
    data: Vec<u8>,
}

// ── Happy-path tests (plan minimums) ─────────────────────────────────────────

#[test]
fn round_trip_postcard_with_version_prefix() {
    let v = ExampleV1 { a: 42, b: "hi".into() };
    let bytes = encode_versioned(1, &v).unwrap();
    assert_eq!(bytes[0], 1, "first byte must be the version");
    let (version, decoded) = decode_versioned::<ExampleV1>(&bytes).unwrap();
    assert_eq!(version, 1);
    assert_eq!(decoded, v);
}

#[test]
fn version_byte_is_always_first() {
    for v in [0u8, 1, 127, 255] {
        let val = ExampleV1 { a: 0, b: String::new() };
        let bytes = encode_versioned(v, &val).unwrap();
        assert_eq!(bytes[0], v);
        assert!(bytes.len() > 1, "must have at least version + payload");
    }
}

#[test]
fn unknown_version_still_decodes_if_payload_valid() {
    // Version byte 99 is just metadata — decode_versioned doesn't validate
    // the version, it just returns it; callers decide if version is known.
    let v = ExampleV1 { a: 1, b: "x".into() };
    let bytes = encode_versioned(99, &v).unwrap();
    let (ver, decoded) = decode_versioned::<ExampleV1>(&bytes).unwrap();
    assert_eq!(ver, 99);
    assert_eq!(decoded, v);
}

// ── Adversarial tests ─────────────────────────────────────────────────────────

#[test]
fn empty_bytes_returns_error() {
    let r: Result<(u8, ExampleV1), _> = decode_versioned(&[]);
    assert!(r.is_err(), "empty payload must be an error");
}

#[test]
fn single_byte_version_only_is_error_for_struct() {
    // One byte = version only, no payload.
    let r: Result<(u8, ExampleV1), _> = decode_versioned(&[1u8]);
    // postcard will fail to decode an empty slice into ExampleV1
    assert!(r.is_err());
}

#[test]
fn truncated_payload_returns_error() {
    let v = ExampleV1 { a: 42, b: "hello world".into() };
    let bytes = encode_versioned(1, &v).unwrap();
    assert!(bytes.len() > 3, "need enough bytes to truncate");
    // Truncate after the version byte to something very short
    let truncated = &bytes[..3];
    let r: Result<(u8, ExampleV1), _> = decode_versioned(truncated);
    assert!(r.is_err(), "truncated payload must be an error");
}

#[test]
fn junk_payload_returns_error() {
    let mut bytes = vec![1u8]; // valid version
    bytes.extend_from_slice(b"\xff\xfe\xfd\xfc"); // not valid postcard
    let r: Result<(u8, ExampleV1), _> = decode_versioned(&bytes);
    assert!(r.is_err(), "junk payload must be an error");
}

#[test]
fn max_size_payload_round_trips() {
    // 1 MB payload
    let big = SimpleBytes { data: vec![0xABu8; 1024 * 1024] };
    let encoded = encode_versioned(1, &big).unwrap();
    assert_eq!(encoded[0], 1);
    let (_, decoded) = decode_versioned::<SimpleBytes>(&encoded).unwrap();
    assert_eq!(decoded, big);
}

#[test]
fn encode_version_zero_works() {
    let v = ExampleV1 { a: 0, b: "".into() };
    let bytes = encode_versioned(0, &v).unwrap();
    assert_eq!(bytes[0], 0);
    let (ver, decoded) = decode_versioned::<ExampleV1>(&bytes).unwrap();
    assert_eq!(ver, 0);
    assert_eq!(decoded, v);
}

#[test]
fn encode_version_255_works() {
    let v = ExampleV1 { a: 255, b: "max".into() };
    let bytes = encode_versioned(255, &v).unwrap();
    assert_eq!(bytes[0], 255);
    let (ver, decoded) = decode_versioned::<ExampleV1>(&bytes).unwrap();
    assert_eq!(ver, 255);
    assert_eq!(decoded, v);
}

// ── Additional adversarial: single byte and 1MB junk for SessionRecord ────────

#[test]
fn single_byte_session_record_returns_error() {
    // Just a version byte, no payload — must return Err, never panic.
    let r: Result<(u8, SessionRecord), _> = decode_versioned(&[1u8]);
    assert!(r.is_err(), "single version byte must error for SessionRecord");
}

#[test]
fn one_mb_junk_blob_for_session_record_returns_error() {
    // 1 MB of junk bytes fed to decode_versioned::<SessionRecord>:
    // must never panic, must return Err.
    let mut junk = vec![1u8]; // version byte
    junk.extend(vec![0xFFu8; 1024 * 1024]); // 1 MB of junk payload
    let r: Result<(u8, SessionRecord), _> = decode_versioned(&junk);
    assert!(r.is_err(), "1MB junk blob must error for SessionRecord");
}

// ── Proptest property tests ───────────────────────────────────────────────────

// NOTE: The proptest below is the temporary stand-in for `cargo fuzz` on
// `sid_store::codec::decode_versioned`. It covers the same invariant (arbitrary
// bytes must never panic, must return Ok or Err deterministically). Real fuzzing
// with libFuzzer should be set up in a `fuzz/` crate once CI lands, targeting
// `decode_versioned::<SessionRecord>` specifically (see CLAUDE.md §cargo fuzz).

proptest! {
    #[test]
    fn proptest_round_trip_u32_string(a in 0u32..u32::MAX, b in ".*") {
        let v = ExampleV1 { a, b };
        let bytes = encode_versioned(1, &v).unwrap();
        let (ver, decoded) = decode_versioned::<ExampleV1>(&bytes).unwrap();
        prop_assert_eq!(ver, 1);
        prop_assert_eq!(decoded, v);
    }

    #[test]
    fn proptest_version_preserved(version in 0u8..=255u8, a in 0u32..1000u32) {
        let v = ExampleV1 { a, b: "test".into() };
        let bytes = encode_versioned(version, &v).unwrap();
        let (decoded_ver, decoded_val) = decode_versioned::<ExampleV1>(&bytes).unwrap();
        prop_assert_eq!(decoded_ver, version);
        prop_assert_eq!(decoded_val, v);
    }

    #[test]
    fn proptest_arbitrary_bytes_never_panics(bytes in proptest::collection::vec(0u8..=255u8, 0..512)) {
        // Decoding arbitrary bytes must never panic — only return Ok or Err.
        let _: Result<(u8, ExampleV1), _> = decode_versioned(&bytes);
    }

    #[test]
    fn proptest_arbitrary_bytes_blob_round_trip(data in proptest::collection::vec(0u8..=255u8, 0..4096)) {
        let v = SimpleBytes { data: data.clone() };
        let bytes = encode_versioned(1, &v).unwrap();
        let (_, decoded) = decode_versioned::<SimpleBytes>(&bytes).unwrap();
        prop_assert_eq!(decoded.data, data);
    }

    /// Proptest fuzz equivalent for decode_versioned::<SessionRecord>:
    /// feeds arbitrary Vec<u8> (0..=4096 bytes) and asserts:
    ///   1. No panic.
    ///   2. Results are deterministic: decoding the same bytes twice gives the
    ///      same Ok/Err variant and same value.
    #[test]
    fn proptest_session_record_decode_never_panics_and_is_deterministic(
        bytes in proptest::collection::vec(0u8..=255u8, 0..=4096)
    ) {
        let r1: Result<(u8, SessionRecord), _> = decode_versioned(&bytes);
        let r2: Result<(u8, SessionRecord), _> = decode_versioned(&bytes);
        // Both calls must agree on Ok vs Err and on the decoded value.
        match (r1, r2) {
            (Ok((v1, rec1)), Ok((v2, rec2))) => {
                prop_assert_eq!(v1, v2);
                prop_assert_eq!(rec1, rec2);
            }
            (Err(_), Err(_)) => {} // both errored — deterministic
            _ => prop_assert!(false, "decode_versioned must be deterministic"),
        }
    }
}
