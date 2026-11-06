//! Matches request paths against the generated operation table, with
//! [`ninot_common::router`].

pub(crate) use ninot_common::router::Router;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::OPERATIONS;

    #[test]
    fn every_operation_of_the_pinned_spec_routes_to_itself() {
        let router = Router::new(OPERATIONS);
        for info in OPERATIONS {
            let path = info
                .path
                .split('/')
                .map(|part| if part.starts_with('{') { "x" } else { part })
                .collect::<Vec<_>>()
                .join("/");
            let found = router.find(info.method, &path).unwrap();
            assert_eq!(
                found.info.path,
                info.path,
                "{} {} routed to {}",
                info.method.as_str(),
                info.path,
                found.info.id
            );
        }
    }
}
