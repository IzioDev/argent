# Sil compatibility corpus

Run both layers with:

```sh
cargo test --lib compatibility_tests
cargo test --test sil_compatibility
```

Both also run under `./check.sh`. They use the Sil version pinned by Cargo,
not a local checkout, a saved compiler binary, or the current Git branch name.

## Parser layer

`src/compiler/syntax/parser/compatibility_tests.rs` compares ordinary Sil
expression and statement ASTs with upstream Sil's parser. Expressions run in
constants, entries, global helpers, and actor helpers. Statements run in all
three body contexts. Source offsets differ between wrappers, so comparisons
ignore offset metadata but also check authored expression and statement text,
name roles, and the text covered by name spans. Block spans allow the two
parsers' brace-inclusion conventions.

Cases cover precedence, token adjacency, parentheses, literals, casts, arrays,
structs, destructuring, control flow, introspection, native calls, and malformed
syntax. This layer tests syntax, not type correctness or name resolution.

## End-to-end layer

`tests/sil_compatibility.rs` places each ordinary-code case in an entry, a
global helper, and an actor helper. Each is compared with independently written
Sil using upstream parsing and compilation. The complete contract ABI includes
compiled bytecode, dispatch tags, runtime state, and template metadata; struct
records are compared too. The reference does not reuse Argent's emitted source
or lowering passes.

Build outputs use auto-cleaned temporary directories, including failure paths.

Each case executes valid result claims and rejects incorrect claims through
the runtime builder. Branch and loop cases include multiple paths and zero
iterations. An incorrect claim must fail during script execution, not during
argument construction or artifact lookup.

Intentional Argent restrictions have separate diagnostic tests. Do not add a
valid upstream construct to a rejection list merely to make the corpus pass.
For an intentional surface extension, provide its explicit Sil equivalent.

## Adding coverage

Add a small named syntax case or end-to-end case at the layer it exercises.
Prefer several paths or boundary values to a single example. Keep expected
Sil independent of the implementation being checked. Extend existing tests
for routes, state representations, imports, ranges, and security invariants
when those features require richer application setup; this corpus complements
those tests rather than duplicating their fixtures.

This is a maintained compatibility sample, not a complete Sil grammar proof
or a replacement for compiler and security review.
