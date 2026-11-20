//! Cursor, node-id and validation-error grammars the resolvers parse.

use std::str::FromStr;

use winnow::ascii::digit1;
use winnow::combinator::opt;
use winnow::error::{ContextError, ErrMode, ModalResult};
use winnow::prelude::*;
use winnow::token::{literal, take, take_until, take_while};

fn decimal<T: FromStr>(input: &mut &str) -> ModalResult<T> {
    let (_, digits) = (opt('+'), digit1).parse_next(input)?;
    digits
        .parse()
        .map_err(|_| ErrMode::<ContextError>::Backtrack(ContextError::new()))
}

/// The offset of a `cursor:<offset>` pagination cursor.
pub fn cursor(input: &mut &str) -> ModalResult<usize> {
    literal("cursor:").void().parse_next(input)?;
    decimal.parse_next(input)
}

/// The id in the `04:User42` payload of a node id made by [`node_id`](super::node_id).
pub fn node_payload(input: &mut &str) -> ModalResult<u64> {
    let kind_length: usize = decimal.parse_next(input)?;
    (':'.void(), take(kind_length).void()).parse_next(input)?;
    decimal.parse_next(input)
}

/// The `Type` and `field` of a `Type.field` key, both GraphQL names.
pub fn field_key<'s>(input: &mut &'s str) -> ModalResult<(&'s str, &'s str)> {
    let type_name = take_until(0.., '.').parse_next(input)?;
    '.'.void().parse_next(input)?;
    let field = take_while(1.., ('a'..='z', 'A'..='Z', '0'..='9', '_')).parse_next(input)?;
    Ok((type_name, field))
}

/// The type and field of an `Unknown field "field" on type "Type".` validation error,
/// leaving the "Did you mean" suggestions async-graphql may append unparsed.
pub fn unknown_field<'s>(input: &mut &'s str) -> ModalResult<(&'s str, &'s str)> {
    (
        (
            literal("Unknown field \"").void(),
            take_until(0.., '"'),
            '"'.void(),
        ),
        (
            literal(" on type \"").void(),
            take_until(0.., '"'),
            '"'.void(),
        ),
    )
        .map(|((_, field, _), (_, type_name, _))| (type_name, field))
        .parse_next(input)
}
