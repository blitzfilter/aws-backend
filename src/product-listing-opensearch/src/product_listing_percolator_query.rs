use crate::product_listing_document::ProductListingDocumentSerdeField;
use crate::product_listing_search_reader::build_common_filter_clauses;
use domain_primitives::query::text_query::TextQuery;
use localization::Language;
use money::Currency;
use product_listing_core::product_listing_search::ProductListingSearch;
use serde_json::json;

/// Builds saved-filter percolator JSON from authoritative SearchFilter state.
///
/// Price clauses target the private all-currency temporary ProductListing document;
/// they never depend on an FX snapshot or persistent ProductListing index fields.
pub fn build_percolator_query(
    search: &ProductListingSearch,
) -> Result<serde_json::Value, serde_json::Error> {
    let mut must = Vec::with_capacity(1);
    if let Some(product_listing_query_clause) = build_product_listing_query_clause(
        &search.product_listing_query,
        title_field(&search.language),
    ) {
        must.push(product_listing_query_clause);
    }
    let (must_not, mut filter) = build_common_filter_clauses(search)?;
    if let Some(price_clause) = build_percolator_price_clause(search) {
        filter.push(price_clause);
    }

    Ok(json!({
        "bool": {
            "must": must,
            "must_not": must_not,
            "filter": filter
        }
    }))
}

fn build_percolator_price_clause(search: &ProductListingSearch) -> Option<serde_json::Value> {
    let range = search.price_query?;
    let mut bounds = serde_json::Map::new();
    if let Some(minimum) = range.min {
        bounds.insert("gte".to_owned(), json!(u64::from(minimum)));
    }
    if let Some(maximum) = range.max {
        bounds.insert("lte".to_owned(), json!(u64::from(maximum)));
    }

    Some(json!({
        "range": {
            percolation_price_field(search.currency): bounds
        }
    }))
}

fn percolation_price_field(currency: Currency) -> &'static str {
    match currency {
        Currency::Eur => "priceByCurrency.eur",
        Currency::Gbp => "priceByCurrency.gbp",
        Currency::Usd => "priceByCurrency.usd",
        Currency::Aud => "priceByCurrency.aud",
        Currency::Cad => "priceByCurrency.cad",
        Currency::Nzd => "priceByCurrency.nzd",
        Currency::Cny => "priceByCurrency.cny",
        Currency::Brl => "priceByCurrency.brl",
        Currency::Pln => "priceByCurrency.pln",
        Currency::Try => "priceByCurrency.try",
        Currency::Jpy => "priceByCurrency.jpy",
        Currency::Czk => "priceByCurrency.czk",
        Currency::Rub => "priceByCurrency.rub",
        Currency::Aed => "priceByCurrency.aed",
        Currency::Sar => "priceByCurrency.sar",
        Currency::Hkd => "priceByCurrency.hkd",
        Currency::Sgd => "priceByCurrency.sgd",
        Currency::Chf => "priceByCurrency.chf",
        Currency::Zar => "priceByCurrency.zar",
    }
}

fn title_field(language: &Language) -> ProductListingDocumentSerdeField {
    match language {
        Language::De => ProductListingDocumentSerdeField::TitleDe,
        Language::En => ProductListingDocumentSerdeField::TitleEn,
        Language::Fr => ProductListingDocumentSerdeField::TitleFr,
        Language::Es => ProductListingDocumentSerdeField::TitleEs,
        Language::It => ProductListingDocumentSerdeField::TitleIt,
        _ => ProductListingDocumentSerdeField::TitleEn,
    }
}

fn build_product_listing_query_clause(
    product_queries: &[TextQuery<1>],
    title_field: ProductListingDocumentSerdeField,
) -> Option<serde_json::Value> {
    match product_queries {
        [] => None,
        [product_listing_query] => Some(build_text_match_clause(
            product_listing_query.as_ref(),
            title_field,
        )),
        product_queries => Some(json!({
            "bool": {
                "should": product_queries
                    .iter()
                    .map(|product_listing_query| build_text_match_clause(product_listing_query.as_ref(), title_field))
                    .collect::<Vec<_>>(),
                "minimum_should_match": 1
            }
        })),
    }
}

fn build_text_match_clause(
    product_listing_query: &str,
    title_field: ProductListingDocumentSerdeField,
) -> serde_json::Value {
    json!({
        "multi_match": {
            "query": product_listing_query,
            "fields": [title_field.as_str(), "title.text"],
            "type": "best_fields",
            "operator": "or",
            "minimum_should_match": "4<80%"
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain_primitives::query::range_query::RangeQuery;
    use money::MonetaryAmount;

    #[test]
    fn should_preserve_usd_price_bounds_without_fx_conversion()
    -> Result<(), Box<dyn std::error::Error>> {
        let search =
            ProductListingSearch::new(Language::En, Currency::Usd).with_price_query(RangeQuery {
                min: Some(MonetaryAmount::from(10_000_u64)),
                max: Some(MonetaryAmount::from(50_000_u64)),
            });

        let query = build_percolator_query(&search)?;

        assert_eq!(
            Some(&json!({ "gte": 10_000, "lte": 50_000 })),
            query.pointer("/bool/filter/0/range/priceByCurrency.usd")
        );
        assert!(query.to_string().contains("priceByCurrency.usd"));
        Ok(())
    }

    #[test]
    fn should_preserve_jpy_minor_units_without_fx_conversion()
    -> Result<(), Box<dyn std::error::Error>> {
        let search =
            ProductListingSearch::new(Language::En, Currency::Jpy).with_price_query(RangeQuery {
                min: Some(MonetaryAmount::from(7_u64)),
                max: Some(MonetaryAmount::from(11_u64)),
            });

        let query = build_percolator_query(&search)?;

        assert_eq!(
            Some(&json!({ "gte": 7, "lte": 11 })),
            query.pointer("/bool/filter/0/range/priceByCurrency.jpy")
        );
        Ok(())
    }

    #[test]
    fn should_render_open_price_bounds_without_inventing_values()
    -> Result<(), Box<dyn std::error::Error>> {
        let max_only =
            ProductListingSearch::new(Language::En, Currency::Usd).with_price_query(RangeQuery {
                min: None,
                max: Some(MonetaryAmount::from(50_000_u64)),
            });
        let min_only =
            ProductListingSearch::new(Language::En, Currency::Usd).with_price_query(RangeQuery {
                min: Some(MonetaryAmount::from(10_000_u64)),
                max: None,
            });

        assert_eq!(
            Some(&json!({ "lte": 50_000 })),
            build_percolator_query(&max_only)?.pointer("/bool/filter/0/range/priceByCurrency.usd")
        );
        assert_eq!(
            Some(&json!({ "gte": 10_000 })),
            build_percolator_query(&min_only)?.pointer("/bool/filter/0/range/priceByCurrency.usd")
        );
        Ok(())
    }

    #[test]
    fn should_omit_price_clause_when_search_has_no_price_range()
    -> Result<(), Box<dyn std::error::Error>> {
        let query =
            build_percolator_query(&ProductListingSearch::new(Language::En, Currency::Usd))?;

        assert!(!query.to_string().contains("priceByCurrency"));
        Ok(())
    }
}
