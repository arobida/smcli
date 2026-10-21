use serde::Deserialize;
use serde_json::{Value, json};
use smcli_core::service_protocol::*;

const REQUESTS: &str = include_str!("fixtures/service_protocol/requests.jsonl");
const RESPONSES: &str = include_str!("fixtures/service_protocol/responses.jsonl");
const DECODING: &str = include_str!("fixtures/service_protocol/decoding.jsonl");

// JSONEncoder prints whole doubles as integers; serde_json may print a decimal point.
fn assert_wire_value(actual: &Value, expected: &Value) {
    match (actual, expected) {
        (Value::Number(a), Value::Number(b)) => assert_eq!(a.as_f64(), b.as_f64()),
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len());
            for (a, b) in a.iter().zip(b) {
                assert_wire_value(a, b);
            }
        }
        (Value::Object(a), Value::Object(b)) => {
            // Key order is not part of the contract (and serde_json's `preserve_order` may be on via other crates).
            let sorted = |m: &serde_json::Map<String, Value>| m.keys().cloned().collect::<std::collections::BTreeSet<_>>();
            assert_eq!(sorted(a), sorted(b));
            for (key, value) in a {
                assert_wire_value(value, &b[key]);
            }
        }
        _ => assert_eq!(actual, expected),
    }
}

fn round_trip<T: serde::Serialize + serde::de::DeserializeOwned>(fixture: &str, name: &str) {
    let mut output = Vec::new();
    for line in fixture.lines() {
        let message: T = decode_line(line.as_bytes()).unwrap();
        let encoded = encode_line(&message).unwrap();
        assert_eq!(encoded.iter().filter(|&&b| b == b'\n').count(), 1);
        assert_eq!(encoded.last(), Some(&b'\n'));
        assert_wire_value(
            &serde_json::from_slice(&encoded).unwrap(),
            &serde_json::from_str(line).unwrap(),
        );
        output.extend(encoded);
    }
    // Optional test artifact for independent decoding by the Swift fixture generator.
    if let Some(directory) = std::env::var_os("SMCLI_PROTOCOL_ROUNDTRIP_DIR") {
        std::fs::write(std::path::Path::new(&directory).join(name), output).unwrap();
    }
}

#[test]
fn swift_service_protocol_fixtures_round_trip() {
    round_trip::<ServiceRequest>(REQUESTS, "requests.jsonl");
    round_trip::<ServiceResponse>(RESPONSES, "responses.jsonl");
    let operations: Vec<_> = REQUESTS
        .lines()
        .map(|line| decode_line::<ServiceRequest>(line.as_bytes()).unwrap().op)
        .collect();
    for op in [
        Operation::Ping,
        Operation::Record,
        Operation::Suggest,
        Operation::Pick,
        Operation::Search,
        Operation::Context,
        Operation::Files,
        Operation::Grep,
        Operation::Select,
        Operation::Shutdown,
    ] {
        assert!(operations.contains(&op));
    }
}

#[derive(Deserialize)]
struct DecodingCase {
    name: String,
    kind: String,
    json: String,
    valid: bool,
}

#[test]
fn swift_service_protocol_decoding_contract() {
    for line in DECODING.lines() {
        let case: DecodingCase = serde_json::from_str(line).unwrap();
        let valid = match case.kind.as_str() {
            "request" => decode_line::<ServiceRequest>(case.json.as_bytes()).is_ok(),
            "response" => decode_line::<ServiceResponse>(case.json.as_bytes()).is_ok(),
            _ => panic!("unexpected fixture kind"),
        };
        assert_eq!(valid, case.valid, "{}: {}", case.name, case.json);
    }
}

#[test]
fn service_protocol_defaults_omissions_and_versions() {
    assert_eq!(VERSION, 1);
    assert_eq!(MAX_MESSAGE_BYTES, 1 << 20);
    assert_eq!(MAX_RESPONSE_BYTES, 64 << 20);
    assert_eq!(IO_TIMEOUT, 2.0);
    assert_eq!(MAX_CONNECTIONS, 64);
    assert_eq!(
        encode_line(&ServiceRequest::new(Operation::Ping)).unwrap(),
        b"{\"v\":1,\"op\":\"ping\"}\n"
    );
    assert_eq!(
        encode_line(&ServiceResponse::new(true)).unwrap(),
        b"{\"v\":1,\"ok\":true}\n"
    );
    let request: ServiceRequest =
        decode_line(b"{\"v\":1,\"op\":\"ping\",\"command\":null}\n").unwrap();
    assert_eq!(request, ServiceRequest::new(Operation::Ping));
    assert_eq!(
        serde_json::to_value(SearchFilter::default()).unwrap(),
        json!({})
    );

    // Version mismatch is not malformed JSON: future dispatch must issue the version failure.
    let request: ServiceRequest = decode_line(b"{\"v\":2,\"op\":\"ping\"}").unwrap();
    assert_eq!(request.v, 2);
    let response = ServiceResponse::failure("version", "Protocol version 1 required.");
    assert_eq!(
        serde_json::to_value(response).unwrap(),
        json!({
            "v": 1, "ok": false, "error": {"code": "version", "message": "Protocol version 1 required."}
        })
    );
}

#[test]
fn service_protocol_canonical_swift_double_preserves_bits() {
    let request: ServiceRequest =
        decode_line(br#"{"v":1,"op":"record","startedAt":979267932.8934171}"#).unwrap();
    assert_eq!(request.started_at.unwrap().to_bits(), 0x41cd2f38ae725b7e);
}

#[test]
fn service_protocol_integer_boundaries_are_exact() {
    let request: ServiceRequest = decode_line(b"{\"v\":1,\"op\":\"record\",\"limit\":9223372036854775807,\"exitCode\":-9223372036854775808,\"pipestatus\":[9007199254740993]}").unwrap();
    assert_eq!(request.limit, Some(i64::MAX));
    assert_eq!(request.exit_code, Some(i64::MIN));
    assert_eq!(request.pipestatus, Some(vec![9_007_199_254_740_993]));
    let encoded = encode_line(&request).unwrap();
    let value: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(value["limit"].as_i64(), Some(i64::MAX));
    assert_eq!(value["exitCode"].as_i64(), Some(i64::MIN));
    assert_eq!(value["pipestatus"][0].as_i64(), Some(9_007_199_254_740_993));

    for number in [
        "9007199254740994.0",
        "-9007199254740994.0",
        "1e20",
        "-1e20",
        "1.5",
    ] {
        let json = format!("{{\"v\":1,\"op\":\"pick\",\"limit\":{number}}}");
        assert!(
            decode_line::<ServiceRequest>(json.as_bytes()).is_err(),
            "{number}"
        );
    }
    for number in ["1.0", "-1e2", "9007199254740992.0"] {
        let json = format!("{{\"v\":1,\"op\":\"pick\",\"limit\":{number}}}");
        assert!(
            decode_line::<ServiceRequest>(json.as_bytes()).is_ok(),
            "{number}"
        );
    }
}

#[test]
fn service_protocol_nonfinite_doubles_cannot_be_encoded() {
    for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut request = ServiceRequest::new(Operation::Record);
        request.started_at = Some(number);
        assert!(encode_line(&request).is_err());
        assert!(serde_json::to_vec(&request).is_err());
        request.started_at = None;
        request.ended_at = Some(number);
        assert!(encode_line(&request).is_err());
        request.ended_at = None;
        request.filter = Some(SearchFilter {
            since: Some(number),
            ..SearchFilter::default()
        });
        assert!(encode_line(&request).is_err());
        request.filter = Some(SearchFilter {
            before: Some(number),
            ..SearchFilter::default()
        });
        assert!(encode_line(&request).is_err());

        let mut response: ServiceResponse =
            decode_line(RESPONSES.lines().nth(3).unwrap().as_bytes()).unwrap();
        response.suggestions.as_mut().unwrap()[0].score = number;
        assert!(encode_line(&response).is_err());
        let mut response: ServiceResponse =
            decode_line(RESPONSES.lines().nth(4).unwrap().as_bytes()).unwrap();
        response.hits.as_mut().unwrap()[0].frecency = number;
        assert!(encode_line(&response).is_err());
        let mut response: ServiceResponse =
            decode_line(RESPONSES.lines().nth(5).unwrap().as_bytes()).unwrap();
        response
            .context
            .as_mut()
            .unwrap()
            .status
            .as_mut()
            .unwrap()
            .fetched_at = number;
        assert!(encode_line(&response).is_err());
    }
}
