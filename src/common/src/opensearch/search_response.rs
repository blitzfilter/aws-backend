use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct SearchResponse<T> {
    pub took: u64,
    pub timed_out: bool,

    #[serde(rename = "_shards")]
    pub shards: ShardStats,
    pub hits: HitsMetadata<T>,
}

#[derive(Debug, Deserialize, Clone, Copy)]
pub struct ShardStats {
    pub total: u64,
    pub successful: u64,
    pub skipped: u64,
    pub failed: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HitsMetadata<T> {
    pub total: TotalHits,
    pub max_score: Option<f64>,
    pub hits: Vec<SearchHit<T>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TotalHits {
    pub value: u64,
    pub relation: String, // usually "eq" or "gte"
}

#[derive(Debug, Deserialize, Clone)]
pub struct SearchHit<T> {
    #[serde(rename = "_index")]
    pub index: String,

    #[serde(rename = "_id")]
    pub id: String,

    #[serde(rename = "_score")]
    pub score: Option<f64>,

    #[serde(rename = "_source")]
    pub source: T,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize, PartialEq, Clone)]
    struct MyDoc {
        field1: String,
        field2: i32,
    }

    #[test]
    fn should_deserialize_hits_when_multiple_documents_present() {
        let json = r#"
        {
            "took": 10,
            "timed_out": false,
            "_shards": {
                "total": 1,
                "successful": 1,
                "skipped": 0,
                "failed": 0
            },
            "hits": {
                "total": { "value": 2, "relation": "eq" },
                "max_score": 1.0,
                "hits": [
                    {
                        "_index": "test",
                        "_id": "1",
                        "_score": 1.0,
                        "_source": { "field1": "foo", "field2": 42 }
                    },
                    {
                        "_index": "test",
                        "_id": "2",
                        "_score": 0.9,
                        "_source": { "field1": "bar", "field2": 7 }
                    }
                ]
            }
        }
        "#;

        let resp: SearchResponse<MyDoc> = serde_json::from_str(json).unwrap();

        assert_eq!(resp.took, 10);
        assert!(!resp.timed_out);
        assert_eq!(resp.shards.successful, 1);
        assert_eq!(resp.hits.total.value, 2);
        assert_eq!(resp.hits.hits.len(), 2);
    }

    #[test]
    fn should_return_empty_hits_when_no_documents_found() {
        let json = r#"
        {
            "took": 3,
            "timed_out": false,
            "_shards": {
                "total": 1,
                "successful": 1,
                "skipped": 0,
                "failed": 0
            },
            "hits": {
                "total": { "value": 0, "relation": "eq" },
                "max_score": null,
                "hits": []
            }
        }
        "#;

        let resp: SearchResponse<MyDoc> = serde_json::from_str(json).unwrap();

        assert_eq!(resp.took, 3);
        assert_eq!(resp.hits.total.value, 0);
        assert!(resp.hits.hits.is_empty());
    }

    #[test]
    fn should_handle_null_max_score_when_no_relevance_available() {
        let json = r#"
        {
            "took": 5,
            "timed_out": false,
            "_shards": {
                "total": 2,
                "successful": 2,
                "skipped": 0,
                "failed": 0
            },
            "hits": {
                "total": { "value": 1, "relation": "eq" },
                "max_score": null,
                "hits": [
                    {
                        "_index": "docs",
                        "_id": "abc",
                        "_score": null,
                        "_source": { "field1": "baz", "field2": 99 }
                    }
                ]
            }
        }
        "#;

        let resp: SearchResponse<MyDoc> = serde_json::from_str(json).unwrap();

        assert!(resp.hits.max_score.is_none());
        assert_eq!(resp.hits.hits[0].score, None);
    }

    #[test]
    fn should_ignore_extra_fields_when_present_in_hit_source() {
        #[derive(Debug, Deserialize, PartialEq, Clone)]
        struct PartialDoc {
            field1: String,
        }

        let json = r#"
        {
            "took": 7,
            "timed_out": false,
            "_shards": {
                "total": 1,
                "successful": 1,
                "skipped": 0,
                "failed": 0
            },
            "hits": {
                "total": { "value": 1, "relation": "eq" },
                "max_score": 1.0,
                "hits": [
                    {
                        "_index": "test",
                        "_id": "3",
                        "_score": 1.0,
                        "_source": { "field1": "hello", "field2": 123, "extra": "ignored" }
                    }
                ]
            }
        }
        "#;

        let resp: SearchResponse<PartialDoc> = serde_json::from_str(json).unwrap();

        assert_eq!(
            resp.hits.hits[0].source,
            PartialDoc {
                field1: "hello".into()
            }
        );
    }

    #[test]
    fn should_deserialize_total_hits_relation_when_gte_provided() {
        let json = r#"
        {
            "took": 2,
            "timed_out": false,
            "_shards": {
                "total": 1,
                "successful": 1,
                "skipped": 0,
                "failed": 0
            },
            "hits": {
                "total": { "value": 10000, "relation": "gte" },
                "max_score": 0.5,
                "hits": []
            }
        }
        "#;

        let resp: SearchResponse<MyDoc> = serde_json::from_str(json).unwrap();

        assert_eq!(resp.hits.total.relation, "gte");
        assert_eq!(resp.hits.total.value, 10000);
    }

    #[test]
    fn should_mark_response_as_timed_out_when_timeout_flag_is_true() {
        let json = r#"
           {
               "took": 100,
               "timed_out": true,
               "_shards": { "total": 5, "successful": 4, "skipped": 0, "failed": 1 },
               "hits": { "total": { "value": 0, "relation": "eq" },
                         "max_score": null, "hits": [] }
           }
           "#;

        let resp: SearchResponse<MyDoc> = serde_json::from_str(json).unwrap();

        assert!(resp.timed_out);
        assert_eq!(resp.shards.failed, 1);
        assert_eq!(resp.shards.successful, 4);
    }

    #[test]
    fn should_track_failed_shards_when_partial_results_returned() {
        let json = r#"
           {
               "took": 12,
               "timed_out": false,
               "_shards": { "total": 3, "successful": 2, "skipped": 0, "failed": 1 },
               "hits": { "total": { "value": 1, "relation": "eq" },
                         "max_score": 0.8,
                         "hits": [
                           { "_index": "partial", "_id": "xyz", "_score": 0.8,
                             "_source": { "field1": "ok", "field2": 5 } }
                         ] }
           }
           "#;

        let resp: SearchResponse<MyDoc> = serde_json::from_str(json).unwrap();

        assert_eq!(resp.shards.total, 3);
        assert_eq!(resp.shards.successful, 2);
        assert_eq!(resp.shards.failed, 1);
        assert_eq!(resp.hits.hits.len(), 1);
    }
}
