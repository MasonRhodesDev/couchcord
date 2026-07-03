//! Boundary error types. Each domain surfaces a small, domain-shaped error so
//! callers never match on a foreign error variant.

use std::fmt;

macro_rules! simple_error {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(msg: impl Into<String>) -> Self {
                Self(msg.into())
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
        impl std::error::Error for $name {}
    };
}

simple_error!(
    RpcError,
    "An error from the Discord RPC boundary (`cc-discord`)."
);
simple_error!(InputError, "An error from the input boundary (`cc-input`).");
simple_error!(
    RenderError,
    "An error from the render boundary (`cc-render`)."
);
simple_error!(
    ConfigError,
    "An error loading/validating config (`cc-config`)."
);
