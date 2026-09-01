# Repository Intelligence Phase 1A

Phase 1A provides one trusted, deterministic repository knowledge graph. It
parses supported source files locally with Tree-sitter and stores compact
content-addressed `FileFactPack` records. Source bytes remain authoritative;
facts and graph edges are context, not permission or execution authority.

`ContentIdentity` is SHA-256 over exact file bytes. `AnalysisIdentity` also
binds extractor/schema, language grammar identity, and analysis configuration,
so incompatible cached facts are rejected. Normal mode may persist packs in a
trusted cache outside the repository. Private mode uses memory/task-owned
storage and does not persist repository-derived metadata.

The canonical graph is derived only from the canonical workspace. A candidate
view is rebuilt from candidate bytes and shadows canonical facts for modified
files; added files exist only in the overlay and deleted files are absent from
the active view. Candidate `.git`, hooks, filters, and text conversion are not
used. Discard removes the overlay.

Phase 1A populates syntax facts and unresolved import declarations. Future
LSP/SCIP, build metadata, Git/history, coverage, schema, and configuration
analyzers enrich this same graph rather than creating separate graphs. Future
AI notes must be advisory and AnalysisIdentity-bound.

The crate performs no network access, compiler/build execution, project hook
execution, Git filters, or language-server execution. It is not connected to
prompts, tools, ContextSearch, providers, validation, review, or Apply yet.
