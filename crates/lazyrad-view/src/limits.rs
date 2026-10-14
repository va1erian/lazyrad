//! The resource limits a binding is evaluated under.
//!
//! **This is the one place the numbers are stated.** The README and the other
//! module docs refer here instead of repeating them, and
//! `tests/limits_docs.rs` fails if a table below disagrees with a constant, or
//! if prose elsewhere states one of these numbers.
//!
//! The limits are sized for *view* expressions, not data processing. Every one
//! ends an expression with a [`BindError::Limit`](crate::BindError::Limit) or,
//! for nesting, a syntax error; none of them panics or aborts.
//!
//! # The limits
//!
//! | Constant | Value | What it bounds |
//! |---|---|---|
//! | [`MAX_ARRAY_SIZE`] | 512 | array elements in one value, nested ones summed; also the longest array literal and the most iterations of a `map` |
//! | [`MAX_MAP_SIZE`] | 512 | map entries in one value, nested ones summed: one per field, in a list of rows too |
//! | [`MAX_STRING_SIZE`] | 32768 | bytes of text in one value, nested ones summed |
//! | [`MAX_OPERATIONS`] | 32768 | operations in one evaluation, closures included |
//! | [`MAX_CALL_LEVELS`] | 4 | closures calling closures |
//! | [`MAX_EXPR_DEPTHS`] | 16, 10 | expression nesting, then function-body nesting |
//! | [`PEAK_MEMORY_CEILING`] | 192 MiB | transient memory of the worst shape |
//!
//! Rhai counts one per element or entry, so the numbers above are enforced as
//! stated, for a value an expression builds and for one it reads from `state`
//! alike. What follows from them:
//!
//! | You have | It holds | Because |
//! |---|---|---|
//! | a list of numbers or names | 512 | the array limit |
//! | a list of 4-field rows | 127 rows | the map limit, less the entry that holds the list, divided by 4 |
//! | a list of 5-field rows | 102 rows | likewise, divided by 5 |
//! | a list of 10-field rows | 51 rows | likewise, divided by 10 |
//! | a list of 20-field rows | 25 rows | likewise, divided by 20 |
//! | five-operation steps over a full array | 12 | the operation limit over five per element |
//!
//! Text counts only inside an array or a map that is inside one (a list of 400
//! titles of 100 bytes is 40000 bytes, over the text limit). A long text that
//! is only *read* (`bind text: state.doc`) is fine at any length; a method call
//! on one longer than the limit is a diagnostic that names it.
//!
//! A value that does not fit is hidden from bindings (see
//! [`crate::bindings`]), and the diagnostic names it.
//!
//! # Motion
//!
//! Animations are not bindings: they cost nothing in the operation budget, and
//! none of the numbers above bounds them. They have two limits of their own.
//!
//! | Constant | Value | What it bounds |
//! |---|---|---|
//! | [`MAX_RUNNING_ANIMATIONS`] | 256 | animations in flight in one session, one per node and property |
//! | [`MAX_ANIMATION_MS`] | 5000 | the duration, and the delay, of one animation, in milliseconds |
//!
//! Past the first, an animation is not started: the change is applied at once
//! and counted in [`MotionStats::degraded`](crate::MotionStats::degraded). Past
//! the second, a duration or delay is cut to it (and a motion that declares
//! more is a [`TreeError`](crate::TreeError)).
//!
//! # Native stack
//!
//! The worst expressions the nesting limits allow need about 350 KiB of native
//! stack in a debug build, and the suite runs them on a 1 MiB thread. A call
//! level costs about 60 KiB, and a level of expression nesting 7 to 25 KiB.
//!
//! # Memory
//!
//! See [`PEAK_MEMORY_CEILING`] for what the limits cost.

/// The most operations one evaluation may execute, **including** those run by
/// closures that array methods such as `map` call back into.
///
/// Rhai restarts its own counter inside such a callback, so the engine's
/// `max_operations` alone does not bound `array.map(...)`; a shared counter in
/// the progress hook does.
///
/// A `map`, `filter` or `reduce` step costs three to five operations per
/// element, so a pipeline of `k` such steps over `n` elements needs about
/// `4 * k * n` of them. The nesting limit of ten chained calls is reached long
/// before a chain of full-size steps could exhaust the budget.
pub const MAX_OPERATIONS: u64 = 32_768;

/// The deepest call nesting an expression may reach (closures calling closures).
pub const MAX_CALL_LEVELS: usize = 4;

/// The deepest expression nesting the parser accepts, in the expression and in
/// a function body.
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
/// only *read* is fine.
pub const MAX_STRING_SIZE: usize = 32 * 1024;

/// The most array elements a value may hold, summed over a nested value.
///
/// Rhai counts one per element and adds the elements of nested arrays. It is
/// also the longest array literal (one fewer, in practice) and, because every
/// array a native can iterate is at most this long, the most iterations of one
/// `map`: the memory figure in [`PEAK_MEMORY_CEILING`] is proportional to it.
pub const MAX_ARRAY_SIZE: usize = 512;

/// The most map entries a value may hold, summed over a nested value.
///
/// Rhai counts one per entry and adds the entries of nested maps, including
/// the maps inside an array: a list of rows with five fields each has five
/// entries per row. It equals [`MAX_ARRAY_SIZE`] so one number describes both.
pub const MAX_MAP_SIZE: usize = 512;

/// The most memory, in bytes, that one evaluation can transiently allocate in
/// the worst shape these limits permit. **Measured.**
///
/// One native call keeps at most [`MAX_ARRAY_SIZE`] results, each a copy of a
/// value that fits the limits. The expensive shape is many small maps: a map
/// with one entry still allocates a node of about 540 bytes, so a list of 510
/// one-field rows is about 280 KiB, and a `map` over a 511-element list whose
/// closure returns that list keeps 511 copies of it.
///
/// Measured on a debug build (peak working set; an idle process is 8 MiB):
///
/// | Shape | Peak |
/// |---|---|
/// | **510 one-field rows returned on each of 511 iterations**, plus a 31 KiB text built on each (the worst) | **154 MiB** |
/// | 102 five-field rows, 511 iterations | 52 MiB |
/// | 510 numbers, 500 iterations | 29 MiB |
/// | 400 iterations returning a 100 000-element list from `state` (hidden from bindings) | 5 MiB |
///
/// For comparison, the same worst shape under the earlier settings: 545 MiB at
/// 1024 elements and 1024 entries; 217 MiB at 512 elements and 1024 entries;
/// 98 MiB at 256 and 1024.
///
/// What lowers it further, in order of effect: [`MAX_ARRAY_SIZE`] (the number
/// of iterations, linearly), then [`MAX_MAP_SIZE`] (the size of each copy). An
/// exact cure needs Rhai to size-check a closure's result before `map` keeps
/// it, which it does not do.
///
/// This is a known, accepted cost and not a bound the evaluator enforces: no
/// guard stops an expression from allocating up to it. The cure would shadow
/// the array methods that keep closure results (`map`, `zip`) with versions
/// that size-check each result as it is produced; it is deferred (see the
/// README).
pub const PEAK_MEMORY_CEILING: u64 = 192 * 1024 * 1024;

/// The most animations a session lets run at once, counted per node and
/// property (a `Slide` is two: its opacity and its translation).
///
/// **Why this number.** The cost of an animation is a tween per frame and, on
/// the software compositor, a repaint of the node's region, so the cost is in
/// how many nodes move together, not in the tree's size. A preset is at most
/// two animations per node, so the limit is 128 nodes entering at once, which
/// is more than a phone screen of rows and than a desktop form of controls. A
/// page that animates more than that in one frame is repainting most of the
/// window anyway, and an instant change is better than a janky one.
///
/// **What happens past it.** The session does not start the animation: the
/// change is applied at once (an `enter` simply shows the node, an `exit`
/// removes it, a `change` is a `Set`), nothing panics and the next animation
/// that finds room runs normally. The count is
/// [`MotionStats::degraded`](crate::MotionStats::degraded).
///
/// An animation counts from the patch until the renderer reports
/// [`Session::finished`](crate::Session::finished) for it, or a newer patch
/// replaces it.
pub const MAX_RUNNING_ANIMATIONS: usize = 256;

/// The longest duration, and the longest delay, of one animation, in
/// milliseconds.
///
/// Nothing in a user interface should move for five seconds, and the limit
/// makes sure an `exit` cannot hold its node for ever: an infinite or enormous
/// duration would. A longer value in a [`Motion`](crate::motion::Motion) is a
/// [`TreeError`](crate::TreeError); one that reaches the sampler or the
/// planner another way is cut to this.
pub const MAX_ANIMATION_MS: f64 = 5_000.0;
