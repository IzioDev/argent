# Body lowering decisions

## Actor-function `self`

Actor helpers lower `self.<storage_field>` to contract fields. `state(self)` is
entry-only. Reconstructing expanded state inside a helper would require extra
arguments carrying validated expansion preimages; the contract stores only
their digests.

Decide whether to keep authored state explicit or pass expansion arguments
through helper calls to support `state(self)`.

## Entry branch and terminator syntax

Entry parsing accepts unbraced `if` branches, but lowering requires blocks
except for `else if`. Single-route `become` makes the semicolon optional;
ordinary Sil statements require it.

Decide the accepted forms and enforce them in source parsing.

## Empty transitions

`become {}` is accepted for `emits none` and emits no output validations. It is
terminal: following statements are rejected.

Decide whether this is valid termination syntax or an empty-route error.

## Sil expression coverage

Argent rejects Sil ternary expressions. This predates the AST-to-AST refactor.

Decide whether Argent supports the pinned Sil expression syntax in full or an
explicit subset. Align parser compatibility tests with that boundary.
