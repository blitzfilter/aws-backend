use credential_core::oauth_client_id::OAuthClientId;
use domain_primitives::query::text_query::TextQuery;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OAuthClientSearch {
    pub client_id: Option<OAuthClientId>,
    pub name_query: Option<TextQuery<0>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_default_oauth_client_search_to_empty_filters() {
        let search = OAuthClientSearch::default();

        assert_eq!(None, search.client_id);
        assert_eq!(None, search.name_query);
    }
}
