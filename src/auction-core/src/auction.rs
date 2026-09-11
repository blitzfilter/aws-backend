use crate::{
    AuctionChanged, AuctionDescription, AuctionDiscovered, AuctionEventPayload, AuctionFormat,
    AuctionId, AuctionKey, AuctionName, AuctionReportedStatus, AuctionSchedule,
    InvalidAuctionSchedule, ReportedCatalogueLotCount,
};
use domain_primitives::change_outcome::ChangeOutcome;
use localization::{Language, Localized};
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct NewAuction {
    pub id: AuctionId,
    pub key: AuctionKey,
    pub name: Option<Localized<Language, AuctionName>>,
    pub description: Option<Localized<Language, AuctionDescription>>,
    pub catalogue_url: Option<Url>,
    pub format: Option<AuctionFormat>,
    pub schedule: AuctionSchedule,
    pub reported_status: Option<AuctionReportedStatus>,
    pub reported_lot_count: Option<ReportedCatalogueLotCount>,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedAuctionState {
    pub id: AuctionId,
    pub key: AuctionKey,
    pub name: Option<Localized<Language, AuctionName>>,
    pub description: Option<Localized<Language, AuctionDescription>>,
    pub catalogue_url: Option<Url>,
    pub format: Option<AuctionFormat>,
    pub schedule: AuctionSchedule,
    pub reported_status: Option<AuctionReportedStatus>,
    pub reported_lot_count: Option<ReportedCatalogueLotCount>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RehydrateAuctionError {
    #[error("invalid persisted auction schedule")]
    InvalidSchedule(#[source] InvalidAuctionSchedule),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReplaceAuctionScheduleError {
    #[error("invalid auction schedule")]
    InvalidSchedule(#[source] InvalidAuctionSchedule),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Auction {
    id: AuctionId,
    key: AuctionKey,
    name: Option<Localized<Language, AuctionName>>,
    description: Option<Localized<Language, AuctionDescription>>,
    catalogue_url: Option<Url>,
    format: Option<AuctionFormat>,
    schedule: AuctionSchedule,
    reported_status: Option<AuctionReportedStatus>,
    reported_lot_count: Option<ReportedCatalogueLotCount>,
    pending_event_payload: Option<AuctionEventPayload>,
}

impl Auction {
    pub fn create(input: NewAuction) -> Result<Self, RehydrateAuctionError> {
        let mut auction = Self::rehydrate(RehydratedAuctionState {
            id: input.id,
            key: input.key,
            name: input.name,
            description: input.description,
            catalogue_url: input.catalogue_url,
            format: input.format,
            schedule: input.schedule,
            reported_status: input.reported_status,
            reported_lot_count: input.reported_lot_count,
        })?;
        auction.pending_event_payload = Some(AuctionEventPayload::Discovered(Box::new(
            auction.discovered_event(),
        )));
        Ok(auction)
    }

    #[doc(hidden)]
    pub fn rehydrate(state: RehydratedAuctionState) -> Result<Self, RehydrateAuctionError> {
        AuctionSchedule::new(
            state.schedule.bidding_opens().cloned(),
            state.schedule.live_starts().cloned(),
            state.schedule.lots_begin_closing().cloned(),
            state.schedule.scheduled_end().cloned(),
        )
        .map_err(RehydrateAuctionError::InvalidSchedule)?;

        Ok(Self {
            id: state.id,
            key: state.key,
            name: state.name,
            description: state.description,
            catalogue_url: state.catalogue_url,
            format: state.format,
            schedule: state.schedule,
            reported_status: state.reported_status,
            reported_lot_count: state.reported_lot_count,
            pending_event_payload: None,
        })
    }

    pub const fn id(&self) -> AuctionId {
        self.id
    }

    pub fn key(&self) -> &AuctionKey {
        &self.key
    }

    pub fn name(&self) -> Option<&Localized<Language, AuctionName>> {
        self.name.as_ref()
    }

    pub fn description(&self) -> Option<&Localized<Language, AuctionDescription>> {
        self.description.as_ref()
    }

    pub fn catalogue_url(&self) -> Option<&Url> {
        self.catalogue_url.as_ref()
    }

    pub const fn format(&self) -> Option<AuctionFormat> {
        self.format
    }

    pub fn schedule(&self) -> &AuctionSchedule {
        &self.schedule
    }

    pub const fn reported_status(&self) -> Option<AuctionReportedStatus> {
        self.reported_status
    }

    pub const fn reported_lot_count(&self) -> Option<ReportedCatalogueLotCount> {
        self.reported_lot_count
    }

    pub fn rename(&mut self, name: Localized<Language, AuctionName>) -> ChangeOutcome {
        let previous = self.name.clone();
        let current = Some(name);
        if previous == current {
            return ChangeOutcome::Unchanged;
        }
        self.name = current.clone();
        self.coalesce_pending_change(|changed| changed.change_name(previous, current));
        ChangeOutcome::Changed
    }

    pub fn clear_name(&mut self) -> ChangeOutcome {
        let previous = self.name.clone();
        if previous.is_none() {
            return ChangeOutcome::Unchanged;
        }
        self.name = None;
        self.coalesce_pending_change(|changed| changed.change_name(previous, None));
        ChangeOutcome::Changed
    }

    pub fn replace_description(
        &mut self,
        description: Localized<Language, AuctionDescription>,
    ) -> ChangeOutcome {
        let previous = self.description.clone();
        let current = Some(description);
        if previous == current {
            return ChangeOutcome::Unchanged;
        }
        self.description = current.clone();
        self.coalesce_pending_change(|changed| changed.change_description(previous, current));
        ChangeOutcome::Changed
    }

    pub fn clear_description(&mut self) -> ChangeOutcome {
        let previous = self.description.clone();
        if previous.is_none() {
            return ChangeOutcome::Unchanged;
        }
        self.description = None;
        self.coalesce_pending_change(|changed| changed.change_description(previous, None));
        ChangeOutcome::Changed
    }

    pub fn replace_catalogue_url(&mut self, catalogue_url: Url) -> ChangeOutcome {
        let previous = self.catalogue_url.clone();
        let current = Some(catalogue_url);
        if previous == current {
            return ChangeOutcome::Unchanged;
        }
        self.catalogue_url = current.clone();
        self.coalesce_pending_change(|changed| changed.change_catalogue_url(previous, current));
        ChangeOutcome::Changed
    }

    pub fn clear_catalogue_url(&mut self) -> ChangeOutcome {
        let previous = self.catalogue_url.clone();
        if previous.is_none() {
            return ChangeOutcome::Unchanged;
        }
        self.catalogue_url = None;
        self.coalesce_pending_change(|changed| changed.change_catalogue_url(previous, None));
        ChangeOutcome::Changed
    }

    pub fn set_format(&mut self, format: AuctionFormat) -> ChangeOutcome {
        let previous = self.format;
        let current = Some(format);
        if previous == current {
            return ChangeOutcome::Unchanged;
        }
        self.format = current;
        self.coalesce_pending_change(|changed| changed.change_format(previous, current));
        ChangeOutcome::Changed
    }

    pub fn clear_format(&mut self) -> ChangeOutcome {
        let previous = self.format;
        if previous.is_none() {
            return ChangeOutcome::Unchanged;
        }
        self.format = None;
        self.coalesce_pending_change(|changed| changed.change_format(previous, None));
        ChangeOutcome::Changed
    }

    pub fn replace_schedule(
        &mut self,
        schedule: AuctionSchedule,
    ) -> Result<ChangeOutcome, ReplaceAuctionScheduleError> {
        AuctionSchedule::new(
            schedule.bidding_opens().cloned(),
            schedule.live_starts().cloned(),
            schedule.lots_begin_closing().cloned(),
            schedule.scheduled_end().cloned(),
        )
        .map_err(ReplaceAuctionScheduleError::InvalidSchedule)?;
        let previous = self.schedule.clone();
        if previous == schedule {
            return Ok(ChangeOutcome::Unchanged);
        }
        self.schedule = schedule.clone();
        self.coalesce_pending_change(|changed| changed.change_schedule(previous, schedule));
        Ok(ChangeOutcome::Changed)
    }

    pub fn set_reported_status(&mut self, status: AuctionReportedStatus) -> ChangeOutcome {
        let previous = self.reported_status;
        let current = Some(status);
        if previous == current {
            return ChangeOutcome::Unchanged;
        }
        self.reported_status = current;
        self.coalesce_pending_change(|changed| changed.change_reported_status(previous, current));
        ChangeOutcome::Changed
    }

    pub fn clear_reported_status(&mut self) -> ChangeOutcome {
        let previous = self.reported_status;
        if previous.is_none() {
            return ChangeOutcome::Unchanged;
        }
        self.reported_status = None;
        self.coalesce_pending_change(|changed| changed.change_reported_status(previous, None));
        ChangeOutcome::Changed
    }

    pub fn set_reported_lot_count(&mut self, count: ReportedCatalogueLotCount) -> ChangeOutcome {
        let previous = self.reported_lot_count;
        let current = Some(count);
        if previous == current {
            return ChangeOutcome::Unchanged;
        }
        self.reported_lot_count = current;
        self.coalesce_pending_change(|changed| {
            changed.change_reported_lot_count(previous, current)
        });
        ChangeOutcome::Changed
    }

    pub fn clear_reported_lot_count(&mut self) -> ChangeOutcome {
        let previous = self.reported_lot_count;
        if previous.is_none() {
            return ChangeOutcome::Unchanged;
        }
        self.reported_lot_count = None;
        self.coalesce_pending_change(|changed| changed.change_reported_lot_count(previous, None));
        ChangeOutcome::Changed
    }

    pub fn take_pending_event_payload(&mut self) -> Option<AuctionEventPayload> {
        self.pending_event_payload.take()
    }

    fn discovered_event(&self) -> AuctionDiscovered {
        AuctionDiscovered::new(
            self.key.clone(),
            self.name.clone(),
            self.description.clone(),
            self.catalogue_url.clone(),
            self.format,
            self.schedule.clone(),
            self.reported_status,
            self.reported_lot_count,
        )
    }

    fn coalesce_pending_change(&mut self, change: impl FnOnce(&mut AuctionChanged)) {
        if matches!(
            self.pending_event_payload,
            Some(AuctionEventPayload::Discovered(_))
        ) {
            self.pending_event_payload = Some(AuctionEventPayload::Discovered(Box::new(
                self.discovered_event(),
            )));
            return;
        }

        let mut changed = match self.pending_event_payload.take() {
            Some(AuctionEventPayload::Changed(changed)) => *changed,
            None | Some(AuctionEventPayload::Discovered(_)) => AuctionChanged::default(),
        };
        change(&mut changed);
        self.pending_event_payload =
            (!changed.is_empty()).then_some(AuctionEventPayload::Changed(Box::new(changed)));
    }
}

#[cfg(test)]
mod tests {
    use super::{Auction, NewAuction, RehydratedAuctionState};
    use crate::{
        AuctionDescription, AuctionEventPayload, AuctionFormat, AuctionId, AuctionKey, AuctionName,
        AuctionReportedStatus, AuctionSchedule, ReportedCatalogueLotCount, SourceAuctionId,
    };
    use domain_primitives::change_outcome::ChangeOutcome;
    use listing_source_core::ListingSourceId;
    use localization::{Language, Localized};
    use url::Url;

    fn key() -> AuctionKey {
        AuctionKey::new(
            ListingSourceId::new(),
            SourceAuctionId::try_from("catalogue-2026-0042")
                .unwrap_or_else(|error| panic!("valid source auction ID: {error}")),
        )
    }

    fn name(value: &str) -> Localized<Language, AuctionName> {
        Localized::new(
            Language::En,
            AuctionName::try_from(value)
                .unwrap_or_else(|error| panic!("valid auction name: {error}")),
        )
    }

    fn new_auction() -> NewAuction {
        NewAuction {
            id: AuctionId::new(),
            key: key(),
            name: None,
            description: None,
            catalogue_url: None,
            format: None,
            schedule: AuctionSchedule::default(),
            reported_status: None,
            reported_lot_count: None,
        }
    }

    #[test]
    fn should_create_id_only_auction_with_one_discovery_event() {
        let mut auction =
            Auction::create(new_auction()).unwrap_or_else(|error| panic!("valid auction: {error}"));

        assert_eq!(None, auction.name());
        assert_eq!(None, auction.reported_lot_count());
        assert!(matches!(
            auction.take_pending_event_payload(),
            Some(AuctionEventPayload::Discovered(_))
        ));
        assert_eq!(None, auction.take_pending_event_payload());
    }

    #[test]
    fn should_keep_identity_and_key_stable_when_renamed_and_rescheduled() {
        let mut auction =
            Auction::create(new_auction()).unwrap_or_else(|error| panic!("valid auction: {error}"));
        let id = auction.id();
        let key = auction.key().clone();
        let _ = auction.take_pending_event_payload();

        assert_eq!(ChangeOutcome::Changed, auction.rename(name("New name")));
        assert_eq!(
            Ok(ChangeOutcome::Unchanged),
            auction.replace_schedule(AuctionSchedule::default())
        );

        assert_eq!(id, auction.id());
        assert_eq!(&key, auction.key());
    }

    #[test]
    fn should_coalesce_changes_and_drop_net_zero_event() {
        let mut auction =
            Auction::create(new_auction()).unwrap_or_else(|error| panic!("valid auction: {error}"));
        let _ = auction.take_pending_event_payload();

        assert_eq!(ChangeOutcome::Changed, auction.rename(name("Autumn sale")));
        assert_eq!(ChangeOutcome::Changed, auction.clear_name());

        assert_eq!(None, auction.take_pending_event_payload());
    }

    #[test]
    fn should_fold_mutations_during_creation_into_discovery() {
        let mut auction =
            Auction::create(new_auction()).unwrap_or_else(|error| panic!("valid auction: {error}"));

        let _ = auction.rename(name("Autumn sale"));
        let _ = auction.set_format(AuctionFormat::Timed);
        let _ = auction.set_reported_status(AuctionReportedStatus::Scheduled);
        let _ = auction.set_reported_lot_count(ReportedCatalogueLotCount::new(0));
        let _ = auction.replace_description(Localized::new(
            Language::En,
            AuctionDescription::try_from("Fine objects")
                .unwrap_or_else(|error| panic!("valid auction description: {error}")),
        ));
        let _ = auction.replace_catalogue_url(
            Url::parse("https://example.test/catalogue")
                .unwrap_or_else(|error| panic!("valid test URL: {error}")),
        );

        let Some(AuctionEventPayload::Discovered(discovered)) =
            auction.take_pending_event_payload()
        else {
            panic!("expected discovery event");
        };
        assert_eq!(Some(&name("Autumn sale")), discovered.name());
        assert_eq!(Some(AuctionFormat::Timed), discovered.format());
        assert_eq!(
            Some(AuctionReportedStatus::Scheduled),
            discovered.reported_status()
        );
        assert_eq!(
            Some(ReportedCatalogueLotCount::new(0)),
            discovered.reported_lot_count()
        );
    }

    #[test]
    fn should_rehydrate_without_emitting_an_event() {
        let auction = Auction::rehydrate(RehydratedAuctionState {
            id: AuctionId::new(),
            key: key(),
            name: None,
            description: None,
            catalogue_url: None,
            format: None,
            schedule: AuctionSchedule::default(),
            reported_status: None,
            reported_lot_count: None,
        })
        .unwrap_or_else(|error| panic!("valid rehydrated auction: {error}"));

        assert_eq!(None, auction.pending_event_payload);
    }
}
