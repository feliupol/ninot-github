//! Tokens in the `Authorization` header.

use super::*;

/// Extracts the token from `token X`, `Bearer X` or Basic auth with the token as password.
pub(crate) fn token_from_authorization(headers: &HeaderMap) -> Option<String> {
    if let Some(token) = headers.typed_get::<Authorization<Token>>() {
        return Some(token.0.0);
    }
    if let Some(token) = headers.typed_get::<Authorization<Bearer>>() {
        return Some(token.0.token().to_owned());
    }
    Some(
        headers
            .typed_get::<Authorization<Basic>>()?
            .0
            .password()
            .to_owned(),
    )
}

/// GitHub's own `token X` authorization scheme, which `headers` has no type for.
struct Token(String);

impl Credentials for Token {
    const SCHEME: &'static str = "token";

    fn decode(value: &HeaderValue) -> Option<Self> {
        let token = std::str::from_utf8(value.as_bytes().get(Self::SCHEME.len() + 1..)?).ok()?;
        Some(Token(token.trim().to_owned()))
    }

    fn encode(&self) -> HeaderValue {
        let value = format!("{} {}", Self::SCHEME, self.0);
        // Tokens are printable ASCII; one the fake cannot encode as a header value it does
        // not send.
        HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static(Self::SCHEME))
    }
}
