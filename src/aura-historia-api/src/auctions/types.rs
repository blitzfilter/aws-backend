use crate::{
    error::{ApiError, BAD_BODY_VALUE},
    patch_value::{PatchValue, clearable},
    values::LocalizedTextData,
    wire::parse_body_object_id,
};
use application::patch_field::PatchField;
use auction_core::{
    AuctionDescription, AuctionFormat, AuctionId, AuctionName, AuctionReportedStatus,
    AuctionSchedule, AuctionTime, AuctionTimeZone, ReportedCatalogueLotCount, SourceAuctionId,
};
use auction_service::{
    ports::{AuctionMetadataField, AuctionStorageVersion},
    use_cases::{
        commands::{
            create_auction::CreateAuctionCommand,
            update_auction::{AuctionSchedulePatch, UpdateAuctionCommand},
        },
        queries::get_auction::AuctionAdminDetailsView,
    },
};
use listing_source_core::ListingSourceId;
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime, format_description::well_known::Iso8601};
use url::Url;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CreateAuctionData {
    listing_source_id: String,
    source_auction_id: String,
    #[serde(default)]
    name: Option<LocalizedTextData>,
    #[serde(default)]
    description: Option<LocalizedTextData>,
    #[serde(default)]
    catalogue_url: Option<Url>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    schedule: AuctionScheduleData,
    #[serde(default)]
    reported_status: Option<String>,
    #[serde(default)]
    reported_lot_count: Option<u32>,
}

impl TryFrom<CreateAuctionData> for CreateAuctionCommand {
    type Error = ApiError;

    fn try_from(value: CreateAuctionData) -> Result<Self, Self::Error> {
        Ok(Self {
            listing_source_id: parse_body_object_id(
                &value.listing_source_id,
                "listingSourceId",
                "ListingSource",
            )?,
            source_auction_id: source_auction_id(value.source_auction_id)?,
            name: value.name.map(auction_name).transpose()?,
            description: value.description.map(auction_description).transpose()?,
            catalogue_url: value.catalogue_url,
            format: value.format.map(auction_format).transpose()?,
            schedule: value.schedule.into_schedule()?,
            reported_status: value.reported_status.map(auction_status).transpose()?,
            reported_lot_count: value.reported_lot_count.map(ReportedCatalogueLotCount::new),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UpdateAuctionData {
    expected_version: u64,
    #[serde(default)]
    name: PatchValue<LocalizedTextData>,
    #[serde(default)]
    description: PatchValue<LocalizedTextData>,
    #[serde(default)]
    catalogue_url: PatchValue<Url>,
    #[serde(default)]
    format: PatchValue<String>,
    #[serde(default)]
    schedule: AuctionSchedulePatchData,
    #[serde(default)]
    reported_status: PatchValue<String>,
    #[serde(default)]
    reported_lot_count: PatchValue<u32>,
}

impl UpdateAuctionData {
    pub(super) fn into_command(
        self,
        auction_id: AuctionId,
    ) -> Result<UpdateAuctionCommand, ApiError> {
        let expected_version = AuctionStorageVersion::try_from(self.expected_version)
            .map_err(|_| invalid_body("expectedVersion must be a positive integer."))?;
        Ok(UpdateAuctionCommand {
            auction_id,
            expected_version,
            name: map_patch(self.name, auction_name)?,
            description: map_patch(self.description, auction_description)?,
            catalogue_url: clearable(self.catalogue_url),
            format: map_patch(self.format, auction_format)?,
            schedule: self.schedule.into_patch()?,
            reported_status: map_patch(self.reported_status, auction_status)?,
            reported_lot_count: map_patch(self.reported_lot_count, |value| {
                Ok(ReportedCatalogueLotCount::new(value))
            })?,
        })
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuctionScheduleData {
    #[serde(default)]
    bidding_opens: Option<AuctionTimeData>,
    #[serde(default)]
    live_starts: Option<AuctionTimeData>,
    #[serde(default)]
    lots_begin_closing: Option<AuctionTimeData>,
    #[serde(default)]
    scheduled_end: Option<AuctionTimeData>,
}

impl AuctionScheduleData {
    fn into_schedule(self) -> Result<AuctionSchedule, ApiError> {
        AuctionSchedule::new(
            self.bidding_opens
                .map(AuctionTimeData::into_core)
                .transpose()?,
            self.live_starts
                .map(AuctionTimeData::into_core)
                .transpose()?,
            self.lots_begin_closing
                .map(AuctionTimeData::into_core)
                .transpose()?,
            self.scheduled_end
                .map(AuctionTimeData::into_core)
                .transpose()?,
        )
        .map_err(|_| invalid_body("schedule has invalid comparable bounds."))
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuctionSchedulePatchData {
    #[serde(default)]
    bidding_opens: PatchValue<AuctionTimeData>,
    #[serde(default)]
    live_starts: PatchValue<AuctionTimeData>,
    #[serde(default)]
    lots_begin_closing: PatchValue<AuctionTimeData>,
    #[serde(default)]
    scheduled_end: PatchValue<AuctionTimeData>,
}

impl AuctionSchedulePatchData {
    fn into_patch(self) -> Result<AuctionSchedulePatch, ApiError> {
        Ok(AuctionSchedulePatch {
            bidding_opens: map_patch(self.bidding_opens, AuctionTimeData::into_core)?,
            live_starts: map_patch(self.live_starts, AuctionTimeData::into_core)?,
            lots_begin_closing: map_patch(self.lots_begin_closing, AuctionTimeData::into_core)?,
            scheduled_end: map_patch(self.scheduled_end, AuctionTimeData::into_core)?,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "precision",
    rename_all = "SCREAMING_SNAKE_CASE",
    deny_unknown_fields
)]
enum AuctionTimeData {
    Instant {
        #[serde(with = "time::serde::rfc3339")]
        at: OffsetDateTime,
        #[serde(default, rename = "sourceTimezone")]
        source_timezone: Option<String>,
    },
    Date {
        on: String,
        #[serde(default, rename = "sourceTimezone")]
        source_timezone: Option<String>,
    },
}

impl AuctionTimeData {
    fn into_core(self) -> Result<AuctionTime, ApiError> {
        match self {
            Self::Instant {
                at,
                source_timezone,
            } => Ok(AuctionTime::instant(
                at,
                source_timezone.map(timezone).transpose()?,
            )),
            Self::Date {
                on,
                source_timezone,
            } => {
                let on = Date::parse(&on, &Iso8601::DATE)
                    .map_err(|_| invalid_body("schedule date must use YYYY-MM-DD."))?;
                Ok(AuctionTime::date(
                    on,
                    source_timezone.map(timezone).transpose()?,
                ))
            }
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuctionAdminData {
    auction_id: AuctionId,
    listing_source_id: ListingSourceId,
    source_auction_id: String,
    name: Option<LocalizedTextData>,
    description: Option<LocalizedTextData>,
    catalogue_url: Option<Url>,
    format: Option<&'static str>,
    schedule: AuctionScheduleResponseData,
    reported_status: Option<&'static str>,
    reported_lot_count: Option<u32>,
    expected_version: u64,
    protected_fields: Vec<&'static str>,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
}

impl From<AuctionAdminDetailsView> for AuctionAdminData {
    fn from(value: AuctionAdminDetailsView) -> Self {
        Self {
            auction_id: value.auction_id,
            listing_source_id: value.key.listing_source_id(),
            source_auction_id: value.key.source_auction_id().to_string(),
            name: value.name.map(LocalizedTextData::from),
            description: value.description.map(LocalizedTextData::from),
            catalogue_url: value.catalogue_url,
            format: value.format.map(AuctionFormat::as_str),
            schedule: AuctionScheduleResponseData::from(value.schedule),
            reported_status: value.reported_status.map(AuctionReportedStatus::as_str),
            reported_lot_count: value
                .reported_lot_count
                .map(ReportedCatalogueLotCount::value),
            expected_version: value.version.into_inner(),
            protected_fields: value
                .protected_fields
                .into_iter()
                .map(AuctionMetadataField::as_str)
                .collect(),
            created: value.created,
            updated: value.updated,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuctionScheduleResponseData {
    bidding_opens: Option<AuctionTimeResponseData>,
    live_starts: Option<AuctionTimeResponseData>,
    lots_begin_closing: Option<AuctionTimeResponseData>,
    scheduled_end: Option<AuctionTimeResponseData>,
}

impl From<AuctionSchedule> for AuctionScheduleResponseData {
    fn from(value: AuctionSchedule) -> Self {
        Self {
            bidding_opens: value
                .bidding_opens()
                .cloned()
                .map(AuctionTimeResponseData::from),
            live_starts: value
                .live_starts()
                .cloned()
                .map(AuctionTimeResponseData::from),
            lots_begin_closing: value
                .lots_begin_closing()
                .cloned()
                .map(AuctionTimeResponseData::from),
            scheduled_end: value
                .scheduled_end()
                .cloned()
                .map(AuctionTimeResponseData::from),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "precision", rename_all = "SCREAMING_SNAKE_CASE")]
enum AuctionTimeResponseData {
    Instant {
        #[serde(with = "time::serde::rfc3339")]
        at: OffsetDateTime,
        #[serde(rename = "sourceTimezone", skip_serializing_if = "Option::is_none")]
        source_timezone: Option<String>,
    },
    Date {
        on: String,
        #[serde(rename = "sourceTimezone", skip_serializing_if = "Option::is_none")]
        source_timezone: Option<String>,
    },
}

impl From<AuctionTime> for AuctionTimeResponseData {
    fn from(value: AuctionTime) -> Self {
        match value {
            AuctionTime::Instant {
                at,
                source_timezone,
            } => Self::Instant {
                at,
                source_timezone: source_timezone.map(String::from),
            },
            AuctionTime::Date {
                on,
                source_timezone,
            } => Self::Date {
                on: on.to_string(),
                source_timezone: source_timezone.map(String::from),
            },
        }
    }
}

fn source_auction_id(value: String) -> Result<SourceAuctionId, ApiError> {
    SourceAuctionId::try_from(value).map_err(|_| {
        invalid_body("sourceAuctionId must be nonblank, NUL-free, and at most 512 UTF-8 bytes.")
    })
}

fn auction_name(
    value: LocalizedTextData,
) -> Result<localization::Localized<localization::Language, AuctionName>, ApiError> {
    AuctionName::try_from(value.text)
        .map(|payload| localization::Localized::new(value.language, payload))
        .map_err(|_| {
            invalid_body("name.text must be nonblank, NUL-free, and at most 512 UTF-8 bytes.")
        })
}

fn auction_description(
    value: LocalizedTextData,
) -> Result<localization::Localized<localization::Language, AuctionDescription>, ApiError> {
    AuctionDescription::try_from(value.text)
        .map(|payload| localization::Localized::new(value.language, payload))
        .map_err(|_| invalid_body("description.text must be valid nonblank sanitized text of at most 65536 UTF-8 bytes."))
}

fn auction_format(value: String) -> Result<AuctionFormat, ApiError> {
    value
        .parse()
        .map_err(|_| invalid_body("format must be LIVE or TIMED."))
}

fn auction_status(value: String) -> Result<AuctionReportedStatus, ApiError> {
    value.parse().map_err(|_| {
        invalid_body(
            "reportedStatus must be SCHEDULED, IN_PROGRESS, ENDED, POSTPONED, or CANCELLED.",
        )
    })
}

fn timezone(value: String) -> Result<AuctionTimeZone, ApiError> {
    AuctionTimeZone::try_from(value)
        .map_err(|_| invalid_body("sourceTimezone must be a valid IANA timezone identifier."))
}

fn map_patch<T, U>(
    value: PatchValue<T>,
    map: impl Fn(T) -> Result<U, ApiError>,
) -> Result<PatchField<U>, ApiError> {
    match value {
        PatchValue::Omitted => Ok(PatchField::Unchanged),
        PatchValue::Null => Ok(PatchField::Clear),
        PatchValue::Value(value) => map(value).map(PatchField::Set),
    }
}

fn invalid_body(detail: &str) -> ApiError {
    ApiError::bad_request(BAD_BODY_VALUE).with_detail(detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_map_create_payload_with_precise_schedule() -> Result<(), ApiError> {
        let listing_source_id = ListingSourceId::new();
        let payload = serde_json::json!({
            "listingSourceId": listing_source_id,
            "sourceAuctionId": " sale / 42 ",
            "format": "TIMED",
            "schedule": {
                "lotsBeginClosing": {
                    "precision": "INSTANT",
                    "at": "2026-10-18T16:03:00Z",
                    "sourceTimezone": "Europe/Berlin"
                }
            }
        });
        let command = CreateAuctionCommand::try_from(
            serde_json::from_value::<CreateAuctionData>(payload)
                .map_err(|_| invalid_body("invalid test payload"))?,
        )?;

        assert_eq!("sale / 42", command.source_auction_id.as_ref());
        assert_eq!(Some(AuctionFormat::Timed), command.format);
        assert!(command.schedule.lots_begin_closing().is_some());
        Ok(())
    }

    #[test]
    fn should_reject_noncanonical_auction_codes_and_unknown_members() {
        for body in [
            r#"{"listingSourceId":"ls_01jgfjjz4ne2g0000000000000","sourceAuctionId":"sale","format":"timed"}"#,
            r#"{"listingSourceId":"ls_01jgfjjz4ne2g0000000000000","sourceAuctionId":"sale","auctionId":"auc_01jgfjjz4ne2g0000000000000"}"#,
        ] {
            let result = serde_json::from_str::<CreateAuctionData>(body)
                .map_err(|_| invalid_body("invalid body"))
                .and_then(CreateAuctionCommand::try_from);
            assert!(result.is_err());
        }
    }

    #[test]
    fn should_distinguish_patch_omission_from_clear() -> Result<(), ApiError> {
        let auction_id = AuctionId::new();
        let omitted: UpdateAuctionData = serde_json::from_str(r#"{"expectedVersion":1}"#)
            .map_err(|_| invalid_body("invalid test payload"))?;
        let cleared: UpdateAuctionData =
            serde_json::from_str(r#"{"expectedVersion":1,"name":null}"#)
                .map_err(|_| invalid_body("invalid test payload"))?;

        assert!(matches!(
            omitted.into_command(auction_id)?.name,
            PatchField::Unchanged
        ));
        assert!(matches!(
            cleared.into_command(auction_id)?.name,
            PatchField::Clear
        ));
        Ok(())
    }
}
