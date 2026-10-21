# ServiceProtocol wire fixtures

`generate.py` extracts the current Swift DTO declarations from `Sources/SmcliCore` and runs the Swift interpreter with `generate.swift`. It keeps Codable declarations/initializers, not runtime implementations. No Swift package build, service, user history, or Phase 2 code is needed.

- `requests.jsonl`: all ten operations, every request field, Unicode/escaped newlines, and finite double extremes.
- `responses.jsonl`: all response fields, nested suggestion/search/context/GitStatus/file DTOs, empty collections, omitted nils, and open-ended failure codes.
- `decoding.jsonl`: Swift JSONDecoder's acceptance of missing/null/unknown fields, required nested keys (including initializer-defaulted properties), malformed JSON, version mismatches, and numeric boundaries.

Regenerate from the worktree root (requires Swift):

```sh
python3 crates/smcli-core/tests/fixtures/service_protocol/generate.py
```

For a bidirectional check, Rust writes re-encoded fixtures to an existing temporary directory, then Swift decodes them and asserts equality with its original values:

```sh
mkdir -p /tmp/smcli-rust-protocol-roundtrip
CARGO_TARGET_DIR=/tmp/smcli-rust-protocol-target \
SMCLI_PROTOCOL_ROUNDTRIP_DIR=/tmp/smcli-rust-protocol-roundtrip \
cargo test -p smcli-core --test service_protocol
python3 crates/smcli-core/tests/fixtures/service_protocol/generate.py /tmp/smcli-rust-protocol-roundtrip
```

`make rs-test` runs these with the rest of the workspace.

## Deliberate boundaries

The codec emits LF; decoding takes one JSON payload, optionally LF-terminated, as Swift's socket line reader does. Reading a stream, enforcing byte/deadline/connection limits, rejecting a mismatched version in dispatch, and binary identity checks are **not** implemented here. A mismatched version decodes successfully so a future service can return the `version` failure. `build` is only an optional opaque string.

Swift `Int` is represented by `i64`, `pid` by `i32`, and `Double` by `f64`. Integer-token minimum/maximum values remain exact. Ordinary integral floating-point tokens are accepted only within the exact f64 integer range (absolute value at most 2^53) and destination bounds; fractional and larger floating-point values are rejected. As approved for this seam, Foundation's noncanonical decimal-number quirks are not emulated (for example, its acceptance of `9007199254740993.1` as an Int). Canonical Swift-generated integer tokens are the compatibility target. Nonfinite doubles cannot be encoded.

These are the only definitions of the wire types: `entry`, `context` and `history` re-export them and add behavior. Socket location, binary identity and `FileHit` paths live in `service_location.rs`.
