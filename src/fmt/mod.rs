//! Stateless formatting primitives: colors, tables, text wrapping, number formatting.

mod format;
pub mod style;
pub(crate) mod table;
pub(crate) mod wrap;

pub use format::{
    format_bytes, format_count, format_elapsed, format_version, mb_to_bytes, to_json_pretty,
};
pub use wrap::cap_output;
pub(crate) use wrap::truncate_body;
pub use wrap::visible_width;
pub use wrap::write_wrapped;

/// Sealed helper for [`plural`] -- implemented for common integer types.
#[allow(clippy::wrong_self_convention)]
pub trait IsOne {
    fn is_one(self) -> bool;
}
macro_rules! impl_is_one {
    ($($t:ty),*) => { $(impl IsOne for $t { fn is_one(self) -> bool { self == 1 } })* };
}
impl_is_one!(usize, u64, u32, i64, i32);

/// Returns `""` if count is 1, `"s"` otherwise.
pub fn plural(n: impl IsOne) -> &'static str {
    if n.is_one() { "" } else { "s" }
}
