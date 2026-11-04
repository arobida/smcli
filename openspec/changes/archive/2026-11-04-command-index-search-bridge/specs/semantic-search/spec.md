# Spec Delta

## MODIFIED Requirements

### Requirement: Hybrid retrieval by query kind

Search SHALL retrieve candidates by query kind, drawing at least `max(4 × limit, 40)` from each leg. Literal queries SHALL combine a BM25 leg over shell terms and a nearest-neighbour leg over embeddings as a weighted sum of each leg's score over its best, with keyword 0.7 and dense 0.3. Intent queries SHALL retrieve by nearest neighbour over the embedding of the typo-corrected query, plus command-index recall when available. Candidates SHALL then be reranked as defined in history-search-ranking.

#### Scenario: Intent query

- **WHEN** history has `git log --oneline` and the user searches `show recent commits`
- **THEN** that command is returned although it shares no word with the query

#### Scenario: Literal query keeps exact hit on top

- **WHEN** a literal query has an exact keyword hit and a semantically closer partial match
- **THEN** the exact keyword hit scores higher before usage reranking

## ADDED Requirements

### Requirement: Command-index recall for intent queries

When the service answers an intent query and the command index for the current model and sources exists, search SHALL find the command-index examples whose descriptions are nearest the query above a similarity floor, and SHALL add history rows nearest each matched template (with placeholders removed) to the candidate pool. Only rows already in history SHALL be returned.

#### Scenario: English query recalls an unworded command

- **WHEN** history has `git reset --soft HEAD~1`, the command index has a git example described as undoing the last commit, and the user searches `undo last commit`
- **THEN** `git reset --soft HEAD~1` is among the results

#### Scenario: Templates are never results

- **WHEN** a matched template has no similar row in history
- **THEN** search returns no row for that template

#### Scenario: Weak matches are ignored

- **WHEN** no command-index description reaches the similarity floor
- **THEN** results are the same as without the command index

### Requirement: Command-index recall degrades silently

Search SHALL skip command-index recall without error when the command index is missing, still building, held by another process, or when the CLI answers in-process.

#### Scenario: Command index not built

- **WHEN** the command index file does not exist and the user searches
- **THEN** search returns the same results as before this change and reports no error
