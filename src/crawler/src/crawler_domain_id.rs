domain_primitives::object_id_newtype!(CrawlerDomainId, "cd");
domain_primitives::object_id_newtype!(CrawlerReviewId, "cr");
domain_primitives::object_id_newtype!(CrawlerReviewPageId, "crp");
domain_primitives::object_id_newtype!(CrawlerReviewUrlId, "cru");

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn should_generate_canonical_crawler_object_ids() {
        for (prefix, value) in [
            (CrawlerDomainId::PREFIX, CrawlerDomainId::new().to_string()),
            (CrawlerReviewId::PREFIX, CrawlerReviewId::new().to_string()),
            (
                CrawlerReviewPageId::PREFIX,
                CrawlerReviewPageId::new().to_string(),
            ),
            (
                CrawlerReviewUrlId::PREFIX,
                CrawlerReviewUrlId::new().to_string(),
            ),
        ] {
            assert!(value.starts_with(&format!("{prefix}_")));
        }
    }

    #[test]
    fn should_reject_bare_uuid_and_wrong_prefix() {
        let id = CrawlerDomainId::new();
        let suffix = id.to_string();
        let suffix = suffix.strip_prefix("cd_").expect("domain TypeID prefix");

        assert!(CrawlerDomainId::from_str(id.as_uuid().to_string().as_str()).is_err());
        assert!(CrawlerDomainId::from_str(&format!("cr_{suffix}")).is_err());
    }

    #[test]
    fn should_roundtrip_crawler_object_ids_through_uuid_storage() {
        let domain_id = CrawlerDomainId::new();
        let review_id = CrawlerReviewId::new();
        let review_page_id = CrawlerReviewPageId::new();
        let review_url_id = CrawlerReviewUrlId::new();

        assert_eq!(
            CrawlerDomainId::try_from(*domain_id.as_uuid()).expect("valid domain UUIDv7"),
            domain_id
        );
        assert_eq!(
            CrawlerReviewId::try_from(*review_id.as_uuid()).expect("valid review UUIDv7"),
            review_id
        );
        assert_eq!(
            CrawlerReviewPageId::try_from(*review_page_id.as_uuid())
                .expect("valid review-page UUIDv7"),
            review_page_id
        );
        assert_eq!(
            CrawlerReviewUrlId::try_from(*review_url_id.as_uuid())
                .expect("valid review-URL UUIDv7"),
            review_url_id
        );
    }
}
