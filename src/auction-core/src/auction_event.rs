use crate::{
    AuctionDescription, AuctionFormat, AuctionKey, AuctionName, AuctionReportedStatus,
    AuctionSchedule, ReportedCatalogueLotCount,
};
use localization::{Language, Localized};
use std::str::FromStr;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::EnumIter)]
pub enum AuctionEventType {
    Discovered,
    Changed,
}

impl AuctionEventType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "AUCTION_DISCOVERED",
            Self::Changed => "AUCTION_CHANGED",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid auction event type `{value}`")]
pub struct InvalidAuctionEventType {
    value: String,
}

impl FromStr for AuctionEventType {
    type Err = InvalidAuctionEventType;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        use strum::IntoEnumIterator;

        Self::iter()
            .find(|event_type| event_type.as_str() == value)
            .ok_or_else(|| InvalidAuctionEventType {
                value: value.to_owned(),
            })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AuctionEventPayload {
    Discovered(Box<AuctionDiscovered>),
    Changed(Box<AuctionChanged>),
}

impl AuctionEventPayload {
    pub const fn event_type(&self) -> AuctionEventType {
        match self {
            Self::Discovered(_) => AuctionEventType::Discovered,
            Self::Changed(_) => AuctionEventType::Changed,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuctionDiscovered {
    key: AuctionKey,
    name: Option<Localized<Language, AuctionName>>,
    description: Option<Localized<Language, AuctionDescription>>,
    catalogue_url: Option<Url>,
    format: Option<AuctionFormat>,
    schedule: AuctionSchedule,
    reported_status: Option<AuctionReportedStatus>,
    reported_lot_count: Option<ReportedCatalogueLotCount>,
}

impl AuctionDiscovered {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        key: AuctionKey,
        name: Option<Localized<Language, AuctionName>>,
        description: Option<Localized<Language, AuctionDescription>>,
        catalogue_url: Option<Url>,
        format: Option<AuctionFormat>,
        schedule: AuctionSchedule,
        reported_status: Option<AuctionReportedStatus>,
        reported_lot_count: Option<ReportedCatalogueLotCount>,
    ) -> Self {
        Self {
            key,
            name,
            description,
            catalogue_url,
            format,
            schedule,
            reported_status,
            reported_lot_count,
        }
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuctionValueChange<T> {
    previous: T,
    current: T,
}

impl<T> AuctionValueChange<T> {
    pub(crate) const fn new(previous: T, current: T) -> Self {
        Self { previous, current }
    }

    pub const fn previous(&self) -> &T {
        &self.previous
    }

    pub const fn current(&self) -> &T {
        &self.current
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AuctionChanged {
    name: Option<AuctionValueChange<Option<Localized<Language, AuctionName>>>>,
    description: Option<AuctionValueChange<Option<Localized<Language, AuctionDescription>>>>,
    catalogue_url: Option<AuctionValueChange<Option<Url>>>,
    format: Option<AuctionValueChange<Option<AuctionFormat>>>,
    schedule: Option<AuctionValueChange<AuctionSchedule>>,
    reported_status: Option<AuctionValueChange<Option<AuctionReportedStatus>>>,
    reported_lot_count: Option<AuctionValueChange<Option<ReportedCatalogueLotCount>>>,
}

impl AuctionChanged {
    pub fn name(&self) -> Option<&AuctionValueChange<Option<Localized<Language, AuctionName>>>> {
        self.name.as_ref()
    }

    pub fn description(
        &self,
    ) -> Option<&AuctionValueChange<Option<Localized<Language, AuctionDescription>>>> {
        self.description.as_ref()
    }

    pub fn catalogue_url(&self) -> Option<&AuctionValueChange<Option<Url>>> {
        self.catalogue_url.as_ref()
    }

    pub const fn format(&self) -> Option<&AuctionValueChange<Option<AuctionFormat>>> {
        self.format.as_ref()
    }

    pub fn schedule(&self) -> Option<&AuctionValueChange<AuctionSchedule>> {
        self.schedule.as_ref()
    }

    pub const fn reported_status(
        &self,
    ) -> Option<&AuctionValueChange<Option<AuctionReportedStatus>>> {
        self.reported_status.as_ref()
    }

    pub const fn reported_lot_count(
        &self,
    ) -> Option<&AuctionValueChange<Option<ReportedCatalogueLotCount>>> {
        self.reported_lot_count.as_ref()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.description.is_none()
            && self.catalogue_url.is_none()
            && self.format.is_none()
            && self.schedule.is_none()
            && self.reported_status.is_none()
            && self.reported_lot_count.is_none()
    }

    pub(crate) fn change_name(
        &mut self,
        previous: Option<Localized<Language, AuctionName>>,
        current: Option<Localized<Language, AuctionName>>,
    ) {
        coalesce_value_change(&mut self.name, previous, current);
    }

    pub(crate) fn change_description(
        &mut self,
        previous: Option<Localized<Language, AuctionDescription>>,
        current: Option<Localized<Language, AuctionDescription>>,
    ) {
        coalesce_value_change(&mut self.description, previous, current);
    }

    pub(crate) fn change_catalogue_url(&mut self, previous: Option<Url>, current: Option<Url>) {
        coalesce_value_change(&mut self.catalogue_url, previous, current);
    }

    pub(crate) fn change_format(
        &mut self,
        previous: Option<AuctionFormat>,
        current: Option<AuctionFormat>,
    ) {
        coalesce_value_change(&mut self.format, previous, current);
    }

    pub(crate) fn change_schedule(&mut self, previous: AuctionSchedule, current: AuctionSchedule) {
        coalesce_value_change(&mut self.schedule, previous, current);
    }

    pub(crate) fn change_reported_status(
        &mut self,
        previous: Option<AuctionReportedStatus>,
        current: Option<AuctionReportedStatus>,
    ) {
        coalesce_value_change(&mut self.reported_status, previous, current);
    }

    pub(crate) fn change_reported_lot_count(
        &mut self,
        previous: Option<ReportedCatalogueLotCount>,
        current: Option<ReportedCatalogueLotCount>,
    ) {
        coalesce_value_change(&mut self.reported_lot_count, previous, current);
    }
}

fn coalesce_value_change<T: PartialEq>(
    change: &mut Option<AuctionValueChange<T>>,
    previous: T,
    current: T,
) {
    let first_previous = change.take().map_or(previous, |existing| existing.previous);
    *change =
        (first_previous != current).then_some(AuctionValueChange::new(first_previous, current));
}

#[cfg(test)]
mod tests {
    use super::AuctionEventType;
    use std::collections::HashSet;
    use strum::IntoEnumIterator;

    #[test]
    fn should_use_exact_unique_auction_event_type_codes() {
        let event_types = AuctionEventType::iter().collect::<Vec<_>>();

        assert_eq!(
            event_types.len(),
            event_types
                .iter()
                .map(|event_type| event_type.as_str())
                .collect::<HashSet<_>>()
                .len()
        );
        for event_type in event_types {
            assert_eq!(Ok(event_type), event_type.as_str().parse());
        }
        assert!("AUCTION_UPDATED".parse::<AuctionEventType>().is_err());
    }
}
