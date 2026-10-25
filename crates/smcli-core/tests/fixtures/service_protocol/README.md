# ServiceProtocol wire fixtures

These fixtures were generated from the Swift DTOs before the Swift sources were removed, and are now the frozen wire format: a protocol change edits them by hand and bumps the protocol version.

- `requests.jsonl`: frozen protocol-v1 requests for all ten operations, Unicode/escaped newlines, and finite double extremes.
- `alias-records-v2.jsonl`: record-time expansion requests, including unsafe input, UUID retry and historical redefinition.
- `alias-results-v2.jsonl`: explicit expansion provenance in suggestions/search and distinct `recentEntries` variants.
- `requests-v2.jsonl`: protocol-v2 search requests with present, absent and empty `shellPath`, plus request-scoped `rankingOff` ablation. Neither field is persisted.
- `responses.jsonl`: all response fields, nested suggestion/search/context/GitStatus/file DTOs, empty collections, omitted nils, and open-ended failure codes.
- `decoding.jsonl`: Swift JSONDecoder's acceptance of missing/null/unknown fields, required nested keys (including initializer-defaulted properties), malformed JSON, version mismatches, and numeric boundaries.

`make test` runs these with the rest of the workspace.

## Deliberate boundaries

The codec emits LF; decoding takes one JSON payload, optionally LF-terminated, as Swift's socket line reader does. Reading a stream, enforcing byte/deadline/connection limits, rejecting a mismatched version in dispatch, and binary identity checks are **not** implemented here. A mismatched version decodes successfully so a future service can return the `version` failure. `build` is only an optional opaque string.

Swift `Int` is represented by `i64`, `pid` by `i32`, and `Double` by `f64`. Integer-token minimum/maximum values remain exact. Ordinary integral floating-point tokens are accepted only within the exact f64 integer range (absolute value at most 2^53) and destination bounds; fractional and larger floating-point values are rejected. As approved for this seam, Foundation's noncanonical decimal-number quirks are not emulated (for example, its acceptance of `9007199254740993.1` as an Int). Canonical Swift-generated integer tokens are the compatibility target. Nonfinite doubles cannot be encoded.

These are the only definitions of the wire types: `entry`, `context` and `history` re-export them and add behavior. Socket location, binary identity and `FileHit` paths live in `service_location.rs`.
