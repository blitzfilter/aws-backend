use super::product_listing_details_reader::{
    DEFAULT_NOTIFICATION_STATES, ProductListingDetailsRow, ProductListingDetailsRowMappingError,
    product_details_select,
};
use application::{
    error::box_error,
    pagination::{Cursor, CursoredResult},
};
use platform_postgres::SqlxTransaction;
use product_listing_service::ports::{
    AuctionCatalogueCursor, AuctionCataloguePage, AuctionCatalogueReadError,
    AuctionCatalogueReadRequest, AuctionCatalogueReader, AuctionCatalogueReaderFactory,
};
use sqlx::AssertSqlSafe;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxAuctionCatalogueReaderFactory;

struct SqlxAuctionCatalogueReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

impl SqlxAuctionCatalogueReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl AuctionCatalogueReaderFactory<SqlxTransaction> for SqlxAuctionCatalogueReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        transaction: &'tx mut SqlxTransaction,
    ) -> impl AuctionCatalogueReader + 'tx {
        SqlxAuctionCatalogueReader {
            connection: transaction.connection(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Auction catalogue SQL query failed")]
struct AuctionCatalogueQueryError(#[source] sqlx::Error);

#[derive(Debug, thiserror::Error)]
#[error("Auction catalogue row could not map to the read model")]
struct AuctionCatalogueMappingError {
    #[source]
    source: ProductListingDetailsRowMappingError,
}

#[async_trait::async_trait]
impl AuctionCatalogueReader for SqlxAuctionCatalogueReader<'_> {
    async fn list(
        &mut self,
        request: &AuctionCatalogueReadRequest,
    ) -> Result<AuctionCataloguePage, AuctionCatalogueReadError> {
        let size = request.cursor.size.clamp(1, 100);
        let size_usize = usize::try_from(size).map_err(invalid)?;
        let limit = i64::try_from(size + 1).map_err(invalid)?;
        let requested_language = request.language.as_str();
        let user_id = request.user_id.map(|id| id.into_uuid());

        let mut select = product_details_select(DEFAULT_NOTIFICATION_STATES);
        select.push_str(" WHERE auction_context.auction_id = $3 AND p.lifecycle = 'ACTIVE'");
        let rows = match request.cursor.search_after {
            None => {
                select.push_str(
                    " ORDER BY auction_context.catalogue_position ASC NULLS LAST, p.product_listing_id ASC LIMIT $4",
                );
                sqlx::query_as::<_, ProductListingDetailsRow>(AssertSqlSafe(select))
                    .bind(requested_language)
                    .bind(user_id)
                    .bind(request.auction_id.as_uuid())
                    .bind(limit)
                    .fetch_all(&mut *self.connection)
                    .await
            }
            Some(cursor) if cursor.catalogue_position.is_some() => {
                select.push_str(
                    " AND (auction_context.catalogue_position IS NULL OR auction_context.catalogue_position > $4 OR (auction_context.catalogue_position = $4 AND p.product_listing_id > $5)) ORDER BY auction_context.catalogue_position ASC NULLS LAST, p.product_listing_id ASC LIMIT $6",
                );
                sqlx::query_as::<_, ProductListingDetailsRow>(AssertSqlSafe(select))
                    .bind(requested_language)
                    .bind(user_id)
                    .bind(request.auction_id.as_uuid())
                    .bind(i64::from(cursor.catalogue_position.unwrap_or_default()))
                    .bind(cursor.product_listing_id.as_uuid())
                    .bind(limit)
                    .fetch_all(&mut *self.connection)
                    .await
            }
            Some(cursor) => {
                select.push_str(
                    " AND auction_context.catalogue_position IS NULL AND p.product_listing_id > $4 ORDER BY auction_context.catalogue_position ASC NULLS LAST, p.product_listing_id ASC LIMIT $5",
                );
                sqlx::query_as::<_, ProductListingDetailsRow>(AssertSqlSafe(select))
                    .bind(requested_language)
                    .bind(user_id)
                    .bind(request.auction_id.as_uuid())
                    .bind(cursor.product_listing_id.as_uuid())
                    .bind(limit)
                    .fetch_all(&mut *self.connection)
                    .await
            }
        }
        .map_err(|source| AuctionCatalogueReadError::QueryFailed {
            source: box_error(AuctionCatalogueQueryError(source)),
        })?;

        let has_more = rows.len() > size_usize;
        let items = rows
            .into_iter()
            .take(size_usize)
            .map(|row| {
                product_listing_service::ports::PersonalizedProductListingDetailsReadModel::try_from(
                    row,
                )
                .map_err(|source| {
                    AuctionCatalogueReadError::InvalidReadModel {
                        source: box_error(AuctionCatalogueMappingError { source }),
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let search_after = if has_more {
            items
                .last()
                .map(|item| cursor_for_item(request.auction_id, item))
                .transpose()?
        } else {
            None
        };

        Ok(CursoredResult {
            items,
            cursor: Cursor { size, search_after },
            total: None,
        })
    }
}

fn cursor_for_item(
    auction_id: auction_core::AuctionId,
    item: &product_listing_service::ports::PersonalizedProductListingDetailsReadModel,
) -> Result<AuctionCatalogueCursor, AuctionCatalogueReadError> {
    let context = item.item.auction.as_ref().ok_or_else(|| {
        invalid(std::io::Error::other(
            "Auction catalogue item has no Auction context",
        ))
    })?;
    let membership = context.membership().ok_or_else(|| {
        invalid(std::io::Error::other(
            "Auction catalogue item has unresolved Auction context",
        ))
    })?;
    if membership.auction_id() != auction_id {
        return Err(invalid(std::io::Error::other(
            "Auction catalogue item has another Auction membership",
        )));
    }
    Ok(AuctionCatalogueCursor {
        auction_id,
        catalogue_position: context
            .catalogue_position()
            .map(|position| position.value()),
        product_listing_id: item.item.product_listing_id,
    })
}

fn invalid(error: impl std::error::Error + Send + Sync + 'static) -> AuctionCatalogueReadError {
    AuctionCatalogueReadError::InvalidReadModel {
        source: box_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_expose_a_transaction_bound_catalogue_factory() {
        let _ = SqlxAuctionCatalogueReaderFactory::new();
    }
}
