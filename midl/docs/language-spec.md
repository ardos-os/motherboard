# MIDL Language Specification

MIDL describes motherboard service interfaces. It is an interface language, not
an implementation language: it names services, retained stores, RPC functions,
domain types, and typed errors. Runtime authorization remains implemented by
the service.

## Lexical Model

MIDL source is UTF-8 text.

Whitespace is insignificant except inside string literals, which are currently
reserved for future attributes and default values. `//` starts a line comment.
`///` starts a documentation comment and attaches to the next declaration or
enum variant.

Identifiers start with `A..Z`, `a..z`, or `_`, followed by letters, digits, or
`_`. Keywords are reserved and cannot be used as identifiers:

```text
service store public type enum fn error void
```

Semicolons and commas are both accepted as declaration separators in service
bodies. Commas are accepted after enum variants and function parameters.

## Top-Level Declarations

A file contains one or more service declarations:

```text
service Auth {
    type AuthSessionToken = string;

    enum AuthSessionStatus {
        Pending,
        LoggedIn,
        Canceled
    }

    store totalComponents: string[];

    fn start_user_auth(user: string)
        -> AuthSessionToken?(error { UserNotFound, Conflict });
}
```

Service names use upper camel case by convention. The compiler does not enforce
that convention in the prototype.

## Documentation Comments

`///` comments immediately before a declaration are preserved as Rust doc
comments by the Rust backend.

```text
/// Starts a login session.
fn start_user_auth(user: string) -> AuthSessionToken;
```

Blank lines are allowed inside a documentation block. A normal `//` comment
breaks attachment.

## Type Aliases

Aliases name reusable service-local types:

```text
type AuthSessionToken = string;
type PID = u64;
```

Aliases are scoped to the containing service in the language model. Backends may
emit them inside a service module to avoid cross-service collisions.

## Enums

Enums support unit variants and record variants:

```text
enum FarProcessState {
    Running,
    ExittedNormally,
    Crashed { logs: string }
}
```

Record variant fields use `name: Type` syntax.

## Stores

Stores are service-owned retained values.

```text
store logs: string[];
public store theme: string;
```

Private stores require the server to authorize subscriptions. Public stores can
be subscribed by clients directly. If the modifier is omitted, the store is
private.

## Functions

Functions are asynchronous motherboard RPC methods:

```text
fn login_with_password(sessionId: AuthSessionToken, password: string)
    -> bool?(error { InvalidSessionId });
```

The return type is required in this prototype. Use `void` for an empty
successful payload:

```text
fn cancel_login(sessionId: AuthSessionToken) -> void?(error { NotFound });
```

Errors are declared inline with `?(error { ... })`. A function without an error
set cannot return a typed service error, though the transport can still fail.

## Types

Primitive types:

```text
u8 u16 u32 u64 i8 i16 i32 i64 string bool char f32 f64 void
```

Compound types:

```text
T[]                  // dynamic array
(A, B, C)            // tuple
AnonymousStore<T>    // service-created anonymous store id carrying values of T
```

`AnonymousStore<T>` is serialized in normal function payloads as a
kernel-issued `AnonymousStoreId`. A client that receives the id may request a
subscription, but the service owner must still accept or reject the request.

## Anonymous Store Lifetime

MIDL does not change the kernel semantics of anonymous stores:

- stores are created explicitly by a service;
- ids are opaque and kernel-owned;
- any client with an id can attempt to subscribe;
- the service always authorizes the subscription;
- after the store has had at least one accepted subscription, it is cleaned up
  once accepted and pending subscription counts both reach zero.

## Prototype Limitations

This prototype deliberately excludes imports, constants, attributes, maps,
generic user-defined types, versioning rules, and schema evolution. Those can be
added after the first generated bindings are exercised against real services.
