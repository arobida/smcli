# semantic-search Specification

## Purpose

Defines the embedding leg of history search: which model embeds commands, how vectors are stored and kept in sync, how the dense and keyword legs combine, and what happens when the semantic leg is unavailable. Lexical ordering and quality floors are defined in the history-search-ranking capability.

## Requirements

### Requirement: Local static embedding model

Search SHALL embed queries and commands locally with one static model and SHALL NOT use the network or a subprocess. The default model SHALL be the bundled `potion-shell-16m` folder found beside the executable, falling back to the checkout's `Models/` for dev builds; a configured `potionPath` SHALL replace it.

#### Scenario: Missing bundled model

- **WHEN** no `potion-shell-16m` folder with `model.safetensors` exists beside the executable or in the checkout and no `potionPath` is set
- **THEN** search fails with an error naming the model and suggesting `smcli init --potion DIR`

#### Scenario: Query with no known tokens

- **WHEN** a query or command has no tokens in the model vocabulary
- **THEN** it gets a fixed uniform unit vector, so its order is decided by the keyword leg

### Requirement: Index pinned to model identity

The index name SHALL encode the document format version, model name, model identity and vector dimension. A model with a different identity or dimension SHALL NOT reuse vectors from another model's index.

#### Scenario: Model switch

- **WHEN** the user runs `smcli init --potion DIR` with a model at a different path
- **THEN** the next search builds a new index under a new name and never reads the old vectors

#### Scenario: Stored dimension

- **WHEN** commands are indexed
- **THEN** every stored embedding has the loaded model's dimension

### Requirement: Incremental sync

Before each indexed search, the index SHALL match the corpus: commands no longer in the corpus are deleted, new or changed commands are embedded in batches, and usage-only changes update metadata without re-embedding.

#### Scenario: Usage changes

- **WHEN** a command's run count changes but its text and intent do not
- **THEN** its stored vector is kept and only its metadata is updated

#### Scenario: Shared long prefix

- **WHEN** two commands share a prefix longer than the embedding text cap
- **THEN** both are stored as separate rows, keyed by the full command

### Requirement: Hybrid retrieval by query kind

Search SHALL retrieve candidates from both a BM25 leg over shell terms and a nearest-neighbour leg over embeddings, drawing at least `max(4 × limit, 40)` from each. Literal queries SHALL combine the legs as a weighted sum of each leg's score over its best, with keyword 0.7 and dense 0.3. Intent queries SHALL use the index's hybrid search with the legs weighted equally. Candidates SHALL then be reranked as defined in history-search-ranking.

#### Scenario: Intent query

- **WHEN** history has `git log --oneline` and the user searches `show recent commits`
- **THEN** that command is returned although it shares no word with the query

#### Scenario: Literal query keeps exact hit on top

- **WHEN** a literal query has an exact keyword hit and a semantically closer partial match
- **THEN** the exact keyword hit scores higher before usage reranking

### Requirement: Text fallback without the index

When another process holds the index lock, search SHALL skip the semantic leg and answer by word matching over the corpus, using the same marker parsing and literal/intent classification. A corrupt or incompatible index cache SHALL be discarded and rebuilt; other errors SHALL keep it.

#### Scenario: Index locked

- **WHEN** the service holds the index lock and the CLI searches in-process
- **THEN** the CLI returns text-matched results instead of failing

#### Scenario: Corrupt cache

- **WHEN** the index file is corrupt
- **THEN** the next search deletes and rebuilds it

### Requirement: One model loaded at a time

A process SHALL NOT hold two embedding models at once. Model changes through `smcli init` and `smcli reindex` SHALL stop the service before loading the new model, and the service SHALL drop its old engine before building a new one.

#### Scenario: Service rebuilds its engine

- **WHEN** the service sees the configured model change
- **THEN** it releases the old model before loading the new one

### Requirement: Private index

The index directory SHALL be owned by the current user with mode `0700`, because it stores command text.

#### Scenario: New index directory

- **WHEN** search creates `<history-db-path>.search/`
- **THEN** its mode is `0700`
