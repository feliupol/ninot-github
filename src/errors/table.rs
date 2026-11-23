//! The macro that defines [`super::GithubError`] from one table.

/// Defines `GithubError`, its `status()` and `message()`, and the test-only `samples()`.
///
/// `custom` holds variants whose status or message is computed, passed through verbatim as
/// enum variants and as match arms of `status()` and `message()`. Each following block names
/// a [`http::StatusCode`] constant and lists its rows as `Variant => "message"` or
/// `Variant { field: Type } => "message with {field}"`.
macro_rules! github_errors {
    (
        custom {
            variants { $($custom_variant:tt)* }
            status { $($custom_status:tt)* }
            message { $($custom_message:tt)* }
        }
        $(
            $status:ident {
                $(
                    $(#[$meta:meta])*
                    $name:ident $({ $($field:ident : $ty:ty),* $(,)? })? => $message:literal
                ),* $(,)?
            }
        )*
    ) => {
        /// One GitHub REST error.
        #[derive(Clone, Debug)]
        pub(crate) enum GithubError {
            $($(
                $(#[$meta])*
                $name $({ $($field: $ty),* })?,
            )*)*
            $($custom_variant)*
        }

        impl GithubError {
            /// The HTTP status this error answers with.
            ///
            /// [`Self::Injected`] uses the rule's raw code; this returns a stand-in when that
            /// code is not a real status.
            pub(crate) fn status(&self) -> StatusCode {
                match self {
                    $($(
                        Self::$name $({ $($field: _),* })? => StatusCode::$status,
                    )*)*
                    $($custom_status)*
                }
            }

            /// GitHub's message for this error.
            pub(crate) fn message(&self) -> Cow<'_, str> {
                match self {
                    $($(
                        Self::$name $({ $($field),* })? => {
                            github_errors!(@message $message $($(, $field)*)?)
                        }
                    )*)*
                    $($custom_message)*
                }
            }

            /// One value of each table row, with every field set to its own name.
            #[cfg(test)]
            fn samples() -> Vec<Self> {
                vec![$($(
                    Self::$name $({ $($field: stringify!($field).into()),* })?,
                )*)*]
            }
        }
    };
    (@message $message:literal) => {
        Cow::Borrowed($message)
    };
    (@message $message:literal $(, $field:ident)+) => {
        Cow::Owned(format!($message))
    };
}
