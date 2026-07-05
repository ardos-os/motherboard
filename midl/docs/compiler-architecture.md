# MIDL Compiler Architecture

The prototype is organized as a direct compilation pipeline:

```text
source .midl
  -> tokenizer::tokenize
  -> parser::parse_tokens
  -> ast::Document
  -> codegen::rust::generate_rust_with_mode
  -> Rust bindings
```

## Responsibilities

`ast.rs` contains the language model: services, service items, stores,
functions, fields, user enums, type aliases, and type expressions. It also owns
`Diagnostic`, the shared error type with source location.

`tokenizer.rs` performs lexical analysis only. It strips regular comments,
preserves `///` documentation comments as doc tokens, identifies identifiers,
punctuation, arrows, and EOF, and attaches line/column positions.

`parser.rs` consumes tokens and builds `ast::Document`. It owns grammar rules
and syntax diagnostics. It does not generate Rust and does not know about the
motherboard runtime.

`codegen/rust.rs` consumes the AST and emits Rust. It owns Rust naming,
serialization shapes, client bindings, server traits, the shared server runtime,
anonymous-store helpers, and request/reply dispatch.

`lib.rs` is the public facade. `parse_document` wires tokenization and parsing
together for common use, while the individual pipeline stages remain public for
tests, debugging, and future compiler tooling.

## Design Rule

The tokenizer must not depend on AST types other than `Diagnostic`; the parser
must not depend on code generation; backends must consume AST and must not parse
source text themselves.
