use application::error::{BoxError, box_error};
use auction_core::{AuctionEventPayload, AuctionTime};
use localization::{Language, Localized};
use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;

pub(crate) const AUCTION_EVENT_SCHEMA_VERSION: i16 = 1;

#[derive(Debug, thiserror::Error)]
pub(crate) enum AuctionEventCodecError {
    #[error("auction event time serialization failed")]
    Time(#[source] time::error::Format),
}

pub(crate) fn encode(payload: &AuctionEventPayload) -> Result<Value, AuctionEventCodecError> {
    match payload {
        AuctionEventPayload::Discovered(discovered) => Ok(json!({
            "listingSourceId": discovered.key().listing_source_id().as_uuid().to_string(),
            "sourceAuctionId": discovered.key().source_auction_id().as_ref(),
            "name": discovered.name().map(localized_name),
            "description": discovered.description().map(localized_description),
            "catalogueUrl": discovered.catalogue_url().map(url::Url::as_str),
            "format": discovered.format().map(|value| value.as_str()),
            "schedule": schedule(discovered.schedule())?,
            "reportedStatus": discovered.reported_status().map(|value| value.as_str()),
            "reportedLotCount": discovered.reported_lot_count().map(|value| value.value()),
        })),
        AuctionEventPayload::Changed(changed) => Ok(json!({
            "name": changed.name().map(|change| value_change(localized_name_option(change.previous()), localized_name_option(change.current()))),
            "description": changed.description().map(|change| value_change(localized_description_option(change.previous()), localized_description_option(change.current()))),
            "catalogueUrl": changed.catalogue_url().map(|change| value_change(json!(change.previous().as_ref().map(url::Url::as_str)), json!(change.current().as_ref().map(url::Url::as_str)))),
            "format": changed.format().map(|change| value_change(json!(change.previous().map(|value| value.as_str())), json!(change.current().map(|value| value.as_str())))),
            "schedule": changed.schedule().map(|change| Ok::<_, AuctionEventCodecError>(value_change(schedule(change.previous())?, schedule(change.current())?))).transpose()?,
            "reportedStatus": changed.reported_status().map(|change| value_change(json!(change.previous().map(|value| value.as_str())), json!(change.current().map(|value| value.as_str())))),
            "reportedLotCount": changed.reported_lot_count().map(|change| value_change(json!(change.previous().map(|value| value.value())), json!(change.current().map(|value| value.value())))),
        })),
    }
}

fn value_change(previous: Value, current: Value) -> Value {
    json!({ "previous": previous, "current": current })
}

fn localized_name(value: &Localized<Language, auction_core::AuctionName>) -> Value {
    json!({"language": value.localization.as_str(), "text": value.payload.as_ref()})
}
fn localized_name_option(value: &Option<Localized<Language, auction_core::AuctionName>>) -> Value {
    value.as_ref().map(localized_name).unwrap_or(Value::Null)
}
fn localized_description(value: &Localized<Language, auction_core::AuctionDescription>) -> Value {
    json!({"language": value.localization.as_str(), "text": value.payload.as_ref()})
}
fn localized_description_option(
    value: &Option<Localized<Language, auction_core::AuctionDescription>>,
) -> Value {
    value
        .as_ref()
        .map(localized_description)
        .unwrap_or(Value::Null)
}

fn schedule(schedule: &auction_core::AuctionSchedule) -> Result<Value, AuctionEventCodecError> {
    Ok(json!({
        "biddingOpens": schedule.bidding_opens().map(time).transpose()?,
        "liveStarts": schedule.live_starts().map(time).transpose()?,
        "lotsBeginClosing": schedule.lots_begin_closing().map(time).transpose()?,
        "scheduledEnd": schedule.scheduled_end().map(time).transpose()?,
    }))
}

fn time(value: &AuctionTime) -> Result<Value, AuctionEventCodecError> {
    match value {
        AuctionTime::Instant {
            at,
            source_timezone,
        } => Ok(json!({
            "precision": "INSTANT", "at": at.format(&Rfc3339).map_err(AuctionEventCodecError::Time)?,
            "sourceTimezone": source_timezone.as_ref().map(|value| value.as_str()),
        })),
        AuctionTime::Date {
            on,
            source_timezone,
        } => Ok(json!({
            "precision": "DATE", "on": on.to_string(),
            "sourceTimezone": source_timezone.as_ref().map(|value| value.as_str()),
        })),
    }
}

pub(crate) fn boxed(error: AuctionEventCodecError) -> BoxError {
    box_error(error)
}
