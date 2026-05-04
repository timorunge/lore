# Ownership in Rust

Rust's ownership system is the language's most distinctive feature. It enables
memory safety without garbage collection by enforcing three rules at compile
time:

1. Each value has exactly one owner.
2. When the owner goes out of scope, the value is dropped.
3. Ownership can be transferred (moved) or temporarily lent (borrowed).

## Move Semantics

When you assign a variable to another, the original binding becomes invalid:

```rust
let s1 = String::from("hello");
let s2 = s1; // s1 is moved to s2
// println!("{s1}"); // compile error: value used after move
```

Types that implement `Copy` (integers, booleans, references) are duplicated
instead of moved.

## Borrowing

References let you use a value without taking ownership:

- `&T` -- shared (immutable) reference. Multiple allowed simultaneously.
- `&mut T` -- exclusive (mutable) reference. Only one at a time.

The borrow checker ensures references never outlive the data they point to,
preventing dangling pointers at compile time.

## Lifetimes

When the compiler cannot infer how long a reference lives, you annotate it
with a lifetime parameter:

```rust
fn longest<'a>(x: &'a str, y: &'a str) -> &'a str {
    if x.len() > y.len() { x } else { y }
}
```

Lifetimes are purely a compile-time concept -- they produce no runtime cost.
