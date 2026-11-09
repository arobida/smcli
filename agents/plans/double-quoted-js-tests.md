# Discover double-quoted JS and TS tests

**Status:** Replaced by Slice 3C of the [audit fixes plan](audit-fixes-2026-11-08.md#slice-3c-ranking-details-26-27). The tasks below are historical; implementation and verification are tracked there.

## Outcome

Project test suggestions include JS and TS tests written with double quotes, such as `it("name")` and `test("name")`, not only single-quoted ones.

## Acceptance checks

- `test_names` returns valid names from double-quoted `it` and `test` calls and still rejects hostile ones — unit: tests_discovery `#double-quoted-names`
- A project whose only tests are double-quoted yields `npm test -- -t <name>` candidates — unit: test_candidates `#double-quoted-candidates`

## Approach

Single quotes are hard-coded in two places, and both need the double-quoted form.

- `crates/smcli-core/src/tests_discovery.rs`: `test_names` strips only the `it('` and `test('` prefixes. Match the double-quoted prefixes too and end the name at the matching quote.
- `crates/smcli-service/src/test_candidates.rs`: the grep pattern list only finds files containing `it('` or `test('`. Add `it("` and `test("`.

## Decisions

- Keep the existing `valid` check, so double-quoted names get the same shell-safety filter.
- Backtick template names and other runners stay out of scope, because nobody asked for them.

## Tasks

### Plan

- [x] Read the test discovery and candidate search code `#read-code`

### Build

- [ ] Parse double-quoted names in `test_names` `#parse-names`
- [ ] Add the double-quoted grep patterns `#grep-patterns`

### Test

- [ ] Unit test for double-quoted and hostile names `#names-test`
- [ ] Unit test for double-quoted candidates `#candidates-test`
- [ ] Kimi team runs the modular tests for `smcli-core` and `smcli-service` `#run-modular`
