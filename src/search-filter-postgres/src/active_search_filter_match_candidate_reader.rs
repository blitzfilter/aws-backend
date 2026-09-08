use crate::mapping::{name, product_search_from_json, user_search_filter_uuid};
use application::error::box_error;
use platform_postgres::SqlxTransaction;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::ports::{
    ActiveSearchFilterMatchCandidate, ActiveSearchFilterMatchCandidateReadError,
    ActiveSearchFilterMatchCandidateReader, ActiveSearchFilterMatchCandidateReaderFactory,
    SearchFilterMatchCandidate,
};
use sqlx::FromRow;
use std::collections::HashMap;
use user_core::user_id::UserId;

#[derive(Debug, Clone, Default)]
pub struct SqlxActiveSearchFilterMatchCandidateReaderFactory;

struct SqlxActiveSearchFilterMatchCandidateReader<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl ActiveSearchFilterMatchCandidateReaderFactory<SqlxTransaction>
    for SqlxActiveSearchFilterMatchCandidateReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ActiveSearchFilterMatchCandidateReader + 'tx {
        SqlxActiveSearchFilterMatchCandidateReader { tx }
    }
}

#[derive(Debug, FromRow)]
struct ActiveCandidateRow {
    user_id: uuid::Uuid,
    user_search_filter_id: uuid::Uuid,
    name: String,
    search: serde_json::Value,
    embedding: Option<Vec<f32>>,
}

#[async_trait::async_trait]
impl ActiveSearchFilterMatchCandidateReader for SqlxActiveSearchFilterMatchCandidateReader<'_> {
    async fn find_active(
        &mut self,
        candidates: &[SearchFilterMatchCandidate],
    ) -> Result<Vec<ActiveSearchFilterMatchCandidate>, ActiveSearchFilterMatchCandidateReadError>
    {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let user_ids = candidates
            .iter()
            .map(|candidate| uuid::Uuid::from(candidate.user_id))
            .collect::<Vec<_>>();
        let filter_ids = candidates
            .iter()
            .map(|candidate| user_search_filter_uuid(candidate.search_filter_id))
            .collect::<Result<Vec<_>, _>>()
            .map_err(
                |source| ActiveSearchFilterMatchCandidateReadError::ReadFailed {
                    source: box_error(source),
                },
            )?;
        let candidates_by_id = candidates
            .iter()
            .map(|candidate| ((candidate.user_id, candidate.search_filter_id), candidate))
            .collect::<HashMap<_, _>>();

        let rows = sqlx::query_as::<_, ActiveCandidateRow>(
            r#"
            SELECT
                filter.user_id,
                filter.user_search_filter_id,
                filter.name,
                filter.search,
                filter.embedding
            FROM search_filters filter
            WHERE filter.state = 'ACTIVE'
              AND EXISTS (
                  SELECT 1 FROM unnest($1::uuid[], $2::uuid[])
                      AS candidate(user_id, user_search_filter_id)
                  WHERE filter.user_id = candidate.user_id
                    AND filter.user_search_filter_id = candidate.user_search_filter_id
              )
            ORDER BY filter.user_search_filter_id
            FOR SHARE OF filter
            "#,
        )
        .bind(user_ids)
        .bind(filter_ids)
        .fetch_all(self.tx.connection())
        .await
        .map_err(
            |source| ActiveSearchFilterMatchCandidateReadError::ReadFailed {
                source: box_error(source),
            },
        )?;
        let mut active = Vec::with_capacity(rows.len());
        for row in rows {
            let user_id = UserId::from(row.user_id);
            let search_filter_id = UserSearchFilterId::from(row.user_search_filter_id);
            let search = product_search_from_json(row.search).map_err(|source| {
                ActiveSearchFilterMatchCandidateReadError::InvalidPersistedState {
                    source: box_error(source),
                }
            })?;
            let Some(candidate) = candidates_by_id.get(&(user_id, search_filter_id)) else {
                continue;
            };
            if search != candidate.expected_search || row.embedding != candidate.expected_embedding
            {
                continue;
            }
            active.push(ActiveSearchFilterMatchCandidate {
                user_id,
                search_filter_id,
                search_filter_name: name(row.name).map_err(|source| {
                    ActiveSearchFilterMatchCandidateReadError::InvalidPersistedState {
                        source: box_error(source),
                    }
                })?,
                price_match_valuation: candidate.price_match_valuation,
                enhanced_match_reason: candidate.enhanced_match_reason.clone(),
            });
        }
        Ok(active)
    }
}
