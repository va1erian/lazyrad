//! The resource limits a binding is evaluated under.
//!
//! They are sized for *view* expressions, not data processing, and the rest of
//! the crate (the visible view of `state`, the tests that pin the memory and
//! stack claims) is written against these numbers. Every limit ends an
//! expression with a [`BindError::Limit`](crate::BindError::Limit) or, for
//! nesting, a syntax error; none of them panics or aborts.
//!
//! # The number: 1024
//!
//! A binding can see, and can build, at most **1024 array elements** and
//! **1024 map entries** in one value, and 32 KiB of text. These are Rhai's own
//! counts (one per element or entry, summed over everything nested), so the
//! advertised number is the number that is enforced, for a value an expression
//! builds and for one it reads from `state` alike:
//!
//! | You have | It holds | Because |
//! |---|---|---|
//! | a list of numbers or names | at most 1024 | the array limit |
//! | a list of 5-field rows | at most 204 rows | 1024 entries, 5 per row |
//! | a list of 4-field rows | at most 255 rows | likewise; the entry that holds the list costs one |
//! | a list of 10-field rows | at most 102 rows | |
//! | a list of 300 titles of 100 bytes | refused | 30 000 bytes of text are counted in the list; 32 KiB is the limit |
//!
//! A value that does not fit is hidden from bindings (see
//! [`crate::bindings`]), and the diagnostic names it.
//!
//! # What 1024 costs in memory
//!
//! Read [`PEAK_MEMORY_CEILING`] before raising anything: with these numbers a
//! pathological expression can transiently allocate hundreds of MiB.

/// The most operations one evaluation may execute, **including** those run by
/// closures that array methods such as `map` call back into.
///
/// Rhai restarts its own counter inside such a callback, so the engine's
/// `max_operations` alone does not bound `array.map(...)`; a shared counter in
/// the progress hook does.
///
/// A `map`, `filter` or `reduce` step costs three to five operations per
/// element, so a pipeline of `k` such steps over `n` elements needs about
/// `4 * k * n` of them: six five-operation steps over a full-size array
/// ([`MAX_ARRAY_SIZE`] elements) fit.
pub const MAX_OPERATIONS: u64 = 32_768;

/// The deepest call nesting an expression may reach (closures calling closures).
/// A level costs about 60 KiB of native stack in a debug build.
pub const MAX_CALL_LEVELS: usize = 4;

/// The deepest expression nesting the parser accepts, in the expression and in
/// a function body. Parsing and evaluating cost 7 to 25 KiB of native stack per
/// level in a debug build.
///
/// Every binary operator in a flat chain counts as a level, so `a + b + ...`
/// and `a && b && ...` take at most 16 terms, an `else if` chain at most 12
/// branches and a method chain (`.map(..).filter(..)...`) at most 10 calls.
/// Split a longer one across bindings.
pub const MAX_EXPR_DEPTHS: (usize, usize) = (16, 10);

/// The most bytes of text a value may hold, summed over a nested value.
///
/// Rhai refuses to call a method on a longer string (even `.len()`), and
/// counts the strings inside an array or map toward it. A long string that is
/// only *read* (`bind text: state.doc`) is fine.
pub const MAX_STRING_SIZE: usize = 32 * 1024;

/// The most array elements a value may hold, summed over a nested value.
///
/// Rhai counts one per element and adds the elements of nested arrays. It is
/// also the longest array literal and, because every array a native can
/// iterate is at most this long, the most iterations of one `map`: the memory
/// figure in [`PEAK_MEMORY_CEILING`] is proportional to it.
pub const MAX_ARRAY_SIZE: usize = 1024;

/// The most map entries a value may hold, summed over a nested value.
///
/// Rhai counts one per entry and adds the entries of nested maps, including
/// the maps inside an array: a list of rows with five fields each has five
/// entries per row. It equals [`MAX_ARRAY_SIZE`] so one number describes both.
pub const MAX_MAP_SIZE: usize = 1024;

/// The most memory, in bytes, that one evaluation can transiently allocate in
/// the worst shape these limits permit. **Measured, and large.**
///
/// One native call keeps at most [`MAX_ARRAY_SIZE`] results, each a copy of a
/// value that fits the limits. The expensive shape is many small maps: a map
/// with one entry still allocates a node of about 540 bytes, so a list of 1020
/// one-field rows is about 550 KiB, and a `map` over a 1020-element list whose
/// closure returns that list keeps 1020 copies of it.
///
/// Measured on a debug build (peak working set, an idle process is 7 MiB), for
/// that shape with a 31 KiB text built on each iteration as well:
///
/// | `MAX_ARRAY_SIZE` | iterations x rows | peak |
/// |---|---|---|
/// | **1024 (current)** | 1020 x 1020 | **545 MiB** |
/// | 512 | 500 x 500 | 217 MiB |
/// | 256 | 250 x 250 | 98 MiB |
///
/// Typical shapes are far smaller: 204 five-field rows returned on each of
/// 1000 iterations peak at 144 MiB, and 1000 numbers cost 16 KiB a copy.
///
/// What would lower it, in order of effect: [`MAX_ARRAY_SIZE`] (the number of
/// iterations, linearly: the table), then [`MAX_MAP_SIZE`] (the size of each
/// copy). Neither is changed here because the visible budget is deliberately
/// 1024; the figures are given so the trade-off can be made knowingly. An
/// exact cure needs Rhai to size-check a closure's result before `map` keeps
/// it, which it does not do.
pub const PEAK_MEMORY_CEILING: u64 = 640 * 1024 * 1024;
