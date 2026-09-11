use crate::ports::AuctionMetadataField;
use auction_core::{
    Auction, AuctionDescription, AuctionFormat, AuctionName, AuctionReportedStatus,
    AuctionSchedule, AuctionTime, ReplaceAuctionScheduleError, ReportedCatalogueLotCount,
};
use domain_primitives::change_outcome::ChangeOutcome;
use localization::{Language, Localized};
use std::collections::{BTreeMap, BTreeSet};
use url::Url;

/// Listing-embedded metadata can only fill an absent, unprotected Auction field.
/// This policy has no authority to clear or replace shared facts.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EmbeddedAuctionMetadata {
    pub name: Option<Localized<Language, AuctionName>>,
    pub description: Option<Localized<Language, AuctionDescription>>,
    pub catalogue_url: Option<Url>,
    pub format: Option<AuctionFormat>,
    pub reported_status: Option<AuctionReportedStatus>,
    pub reported_lot_count: Option<ReportedCatalogueLotCount>,
    pub bidding_opens: Option<AuctionTime>,
    pub live_starts: Option<AuctionTime>,
    pub lots_begin_closing: Option<AuctionTime>,
    pub scheduled_end: Option<AuctionTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedAuctionMetadataOutcome {
    Unasserted,
    Filled,
    Equal,
    Protected,
    Conflict,
    InvalidSchedule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedAuctionMetadataAcceptance {
    pub fields: BTreeMap<AuctionMetadataField, EmbeddedAuctionMetadataOutcome>,
    pub change: ChangeOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum ApplyEmbeddedAuctionMetadataError {
    #[error("embedded auction metadata produced an invalid schedule")]
    InvalidSchedule(#[source] ReplaceAuctionScheduleError),
}

pub fn apply_embedded_auction_metadata(
    auction: &mut Auction,
    protected_fields: &BTreeSet<AuctionMetadataField>,
    candidate: &EmbeddedAuctionMetadata,
) -> Result<EmbeddedAuctionMetadataAcceptance, ApplyEmbeddedAuctionMetadataError> {
    let mut acceptance = EmbeddedAuctionMetadataAcceptance {
        fields: BTreeMap::new(),
        change: ChangeOutcome::Unchanged,
    };
    let current_name = auction.name().cloned();
    let name = classify(
        AuctionMetadataField::Name,
        protected_fields,
        candidate.name.as_ref(),
        current_name.as_ref(),
    );
    acceptance.fields.insert(AuctionMetadataField::Name, name);
    if name == EmbeddedAuctionMetadataOutcome::Filled
        && let Some(value) = candidate.name.clone()
    {
        acceptance.change = acceptance.change.combine(auction.rename(value));
    }
    let current_description = auction.description().cloned();
    let description = classify(
        AuctionMetadataField::Description,
        protected_fields,
        candidate.description.as_ref(),
        current_description.as_ref(),
    );
    acceptance
        .fields
        .insert(AuctionMetadataField::Description, description);
    if description == EmbeddedAuctionMetadataOutcome::Filled
        && let Some(value) = candidate.description.clone()
    {
        acceptance.change = acceptance
            .change
            .combine(auction.replace_description(value));
    }
    let current_url = auction.catalogue_url().cloned();
    let catalogue_url = classify(
        AuctionMetadataField::CatalogueUrl,
        protected_fields,
        candidate.catalogue_url.as_ref(),
        current_url.as_ref(),
    );
    acceptance
        .fields
        .insert(AuctionMetadataField::CatalogueUrl, catalogue_url);
    if catalogue_url == EmbeddedAuctionMetadataOutcome::Filled
        && let Some(value) = candidate.catalogue_url.clone()
    {
        acceptance.change = acceptance
            .change
            .combine(auction.replace_catalogue_url(value));
    }
    let current_format = auction.format();
    let format = classify(
        AuctionMetadataField::Format,
        protected_fields,
        candidate.format.as_ref(),
        current_format.as_ref(),
    );
    acceptance
        .fields
        .insert(AuctionMetadataField::Format, format);
    if format == EmbeddedAuctionMetadataOutcome::Filled
        && let Some(value) = candidate.format
    {
        acceptance.change = acceptance.change.combine(auction.set_format(value));
    }
    let current_status = auction.reported_status();
    let status = classify(
        AuctionMetadataField::ReportedStatus,
        protected_fields,
        candidate.reported_status.as_ref(),
        current_status.as_ref(),
    );
    acceptance
        .fields
        .insert(AuctionMetadataField::ReportedStatus, status);
    if status == EmbeddedAuctionMetadataOutcome::Filled
        && let Some(value) = candidate.reported_status
    {
        acceptance.change = acceptance
            .change
            .combine(auction.set_reported_status(value));
    }
    let current_count = auction.reported_lot_count();
    let count = classify(
        AuctionMetadataField::ReportedLotCount,
        protected_fields,
        candidate.reported_lot_count.as_ref(),
        current_count.as_ref(),
    );
    acceptance
        .fields
        .insert(AuctionMetadataField::ReportedLotCount, count);
    if count == EmbeddedAuctionMetadataOutcome::Filled
        && let Some(value) = candidate.reported_lot_count
    {
        acceptance.change = acceptance
            .change
            .combine(auction.set_reported_lot_count(value));
    }

    let before = auction.schedule().clone();
    let proposed = AuctionSchedule::new(
        select_schedule_value(
            &mut acceptance.fields,
            AuctionMetadataField::BiddingOpens,
            protected_fields,
            candidate.bidding_opens.as_ref(),
            before.bidding_opens(),
        ),
        select_schedule_value(
            &mut acceptance.fields,
            AuctionMetadataField::LiveStarts,
            protected_fields,
            candidate.live_starts.as_ref(),
            before.live_starts(),
        ),
        select_schedule_value(
            &mut acceptance.fields,
            AuctionMetadataField::LotsBeginClosing,
            protected_fields,
            candidate.lots_begin_closing.as_ref(),
            before.lots_begin_closing(),
        ),
        select_schedule_value(
            &mut acceptance.fields,
            AuctionMetadataField::ScheduledEnd,
            protected_fields,
            candidate.scheduled_end.as_ref(),
            before.scheduled_end(),
        ),
    );
    match proposed {
        Ok(proposed) => {
            let has_fill = acceptance
                .fields
                .values()
                .any(|outcome| *outcome == EmbeddedAuctionMetadataOutcome::Filled);
            if has_fill && proposed != before {
                match auction.replace_schedule(proposed) {
                    Ok(change) => acceptance.change = acceptance.change.combine(change),
                    Err(error) => {
                        return Err(ApplyEmbeddedAuctionMetadataError::InvalidSchedule(error));
                    }
                }
            }
        }
        Err(_) => {
            for field in [
                AuctionMetadataField::BiddingOpens,
                AuctionMetadataField::LiveStarts,
                AuctionMetadataField::LotsBeginClosing,
                AuctionMetadataField::ScheduledEnd,
            ] {
                if acceptance.fields.get(&field) == Some(&EmbeddedAuctionMetadataOutcome::Filled) {
                    acceptance
                        .fields
                        .insert(field, EmbeddedAuctionMetadataOutcome::InvalidSchedule);
                }
            }
        }
    }
    Ok(acceptance)
}

fn classify<T: PartialEq>(
    field: AuctionMetadataField,
    protected: &BTreeSet<AuctionMetadataField>,
    incoming: Option<&T>,
    current: Option<&T>,
) -> EmbeddedAuctionMetadataOutcome {
    match incoming {
        None => EmbeddedAuctionMetadataOutcome::Unasserted,
        Some(_) if protected.contains(&field) => EmbeddedAuctionMetadataOutcome::Protected,
        Some(value) if current == Some(value) => EmbeddedAuctionMetadataOutcome::Equal,
        Some(_) if current.is_some() => EmbeddedAuctionMetadataOutcome::Conflict,
        Some(_) => EmbeddedAuctionMetadataOutcome::Filled,
    }
}

fn select_schedule_value(
    outcomes: &mut BTreeMap<AuctionMetadataField, EmbeddedAuctionMetadataOutcome>,
    field: AuctionMetadataField,
    protected: &BTreeSet<AuctionMetadataField>,
    incoming: Option<&AuctionTime>,
    current: Option<&AuctionTime>,
) -> Option<AuctionTime> {
    match incoming {
        None => {
            outcomes.insert(field, EmbeddedAuctionMetadataOutcome::Unasserted);
            current.cloned()
        }
        Some(_) if protected.contains(&field) => {
            outcomes.insert(field, EmbeddedAuctionMetadataOutcome::Protected);
            current.cloned()
        }
        Some(value) if current == Some(value) => {
            outcomes.insert(field, EmbeddedAuctionMetadataOutcome::Equal);
            current.cloned()
        }
        Some(_) if current.is_some() => {
            outcomes.insert(field, EmbeddedAuctionMetadataOutcome::Conflict);
            current.cloned()
        }
        Some(value) => {
            outcomes.insert(field, EmbeddedAuctionMetadataOutcome::Filled);
            Some(value.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auction_core::{AuctionId, AuctionKey, NewAuction, SourceAuctionId};
    use listing_source_core::ListingSourceId;

    fn auction() -> Auction {
        Auction::create(NewAuction {
            id: AuctionId::new(),
            key: AuctionKey::new(
                ListingSourceId::new(),
                SourceAuctionId::try_from("sale-1")
                    .unwrap_or_else(|error| panic!("valid ID: {error}")),
            ),
            name: None,
            description: None,
            catalogue_url: None,
            format: None,
            schedule: AuctionSchedule::default(),
            reported_status: None,
            reported_lot_count: None,
        })
        .unwrap_or_else(|error| panic!("valid auction: {error}"))
    }

    #[test]
    fn should_fill_absent_embedded_metadata_once() {
        let mut auction = auction();
        let candidate = EmbeddedAuctionMetadata {
            format: Some(AuctionFormat::Timed),
            ..Default::default()
        };
        let accepted = apply_embedded_auction_metadata(&mut auction, &BTreeSet::new(), &candidate)
            .unwrap_or_else(|error| panic!("valid fill: {error}"));
        assert_eq!(Some(AuctionFormat::Timed), auction.format());
        assert_eq!(
            Some(&EmbeddedAuctionMetadataOutcome::Filled),
            accepted.fields.get(&AuctionMetadataField::Format)
        );

        let conflict = EmbeddedAuctionMetadata {
            format: Some(AuctionFormat::Live),
            ..Default::default()
        };
        let outcome = apply_embedded_auction_metadata(&mut auction, &BTreeSet::new(), &conflict)
            .unwrap_or_else(|error| panic!("valid conflict result: {error}"));
        assert_eq!(Some(AuctionFormat::Timed), auction.format());
        assert_eq!(
            Some(&EmbeddedAuctionMetadataOutcome::Conflict),
            outcome.fields.get(&AuctionMetadataField::Format)
        );
    }

    #[test]
    fn should_preserve_protected_absence_from_embedded_fill() {
        let mut auction = auction();
        let protected = BTreeSet::from([AuctionMetadataField::LiveStarts]);
        let candidate = EmbeddedAuctionMetadata {
            live_starts: Some(AuctionTime::instant(
                time::macros::datetime!(2026-10-18 16:00 UTC),
                None,
            )),
            ..Default::default()
        };
        let outcome = apply_embedded_auction_metadata(&mut auction, &protected, &candidate)
            .unwrap_or_else(|error| panic!("valid protected result: {error}"));
        assert_eq!(None, auction.schedule().live_starts());
        assert_eq!(
            Some(&EmbeddedAuctionMetadataOutcome::Protected),
            outcome.fields.get(&AuctionMetadataField::LiveStarts)
        );
    }
}
