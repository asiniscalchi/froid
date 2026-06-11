//! Shared helpers for the crate's error types.

/// Implements `From<$source>` for an error enum by rendering the source
/// through `to_string` into the given string-holding variant.
///
/// Most repositories and services collapse upstream failures into a single
/// string-carrying variant (usually `Storage`); this macro replaces the
/// hand-written `From` impls that all did exactly that.
macro_rules! from_error_string {
    ($target:ident :: $variant:ident, $($source:ty),+ $(,)?) => {
        $(
            impl From<$source> for $target {
                fn from(error: $source) -> Self {
                    Self::$variant(error.to_string())
                }
            }
        )+
    };
}

pub(crate) use from_error_string;
