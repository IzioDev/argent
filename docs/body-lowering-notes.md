# Body lowering notes

Small implementation hints to revisit during the current compiler work.

- Parse state constructor fields through the Sil AST. Accept comments in valid constructors and reject empty or repeated comma components; generated comments do not need to be preserved.
- Define actor-function `self` semantics. Actor functions currently access contract fields by bare Sil names and do not receive entry `self.*` lowering; if that syntax is added, reserve `self` as a function binding at the same time.
- Accept Sil ternaries and bitwise XOR in the shared lexer.
- Attach body-local selector metadata to parsed binding identities instead of keeping the analyzed selector catalog entry-wide and keyed by name.
- Decide whether unbraced `if` bodies pass through or are rejected by the source parser.
- Keep statement terminator policy in source parsing for both ordinary Sil statements and `become`.
- Reject or define empty `become {}`; it currently becomes a no-op for `emits none`.
- Retain each entry body's file offset so body diagnostics use source-file locations.
- Match route-target delimiter kinds so malformed expressions receive structural errors.
- Split `body.rs` and `emitter.rs` after their authored-value proof, state
  conversion, naming, and rendering interfaces become stable.
