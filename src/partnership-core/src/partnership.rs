use crate::{partnership_id::PartnershipId, partnership_lifecycle::PartnershipLifecycle};
use party_core::party_id::PartyId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partnership {
    id: PartnershipId,
    party_id: PartyId,
    lifecycle: PartnershipLifecycle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPartnership {
    pub id: PartnershipId,
    pub party_id: PartyId,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RehydratedPartnershipState {
    pub id: PartnershipId,
    pub party_id: PartyId,
    pub lifecycle: PartnershipLifecycle,
}

impl Partnership {
    pub fn create(input: NewPartnership) -> Self {
        Self {
            id: input.id,
            party_id: input.party_id,
            lifecycle: PartnershipLifecycle::Active,
        }
    }

    #[doc(hidden)]
    pub fn rehydrate(state: RehydratedPartnershipState) -> Self {
        Self {
            id: state.id,
            party_id: state.party_id,
            lifecycle: state.lifecycle,
        }
    }
    pub fn id(&self) -> PartnershipId {
        self.id
    }
    pub fn party_id(&self) -> PartyId {
        self.party_id
    }

    pub fn lifecycle(&self) -> PartnershipLifecycle {
        self.lifecycle
    }

    pub fn dissolve(&mut self) -> bool {
        if self.lifecycle == PartnershipLifecycle::Dissolved {
            return false;
        }

        self.lifecycle = PartnershipLifecycle::Dissolved;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn partnership() -> Partnership {
        Partnership::create(NewPartnership {
            id: PartnershipId::new(),
            party_id: PartyId::new(),
        })
    }

    #[test]
    fn should_create_active_partnership() {
        assert_eq!(PartnershipLifecycle::Active, partnership().lifecycle());
    }

    #[test]
    fn should_dissolve_active_partnership_once() {
        let mut partnership = partnership();

        assert!(partnership.dissolve());
        assert_eq!(PartnershipLifecycle::Dissolved, partnership.lifecycle());
        assert!(!partnership.dissolve());
        assert_eq!(PartnershipLifecycle::Dissolved, partnership.lifecycle());
    }

    #[test]
    fn should_preserve_lifecycle_when_rehydrating() {
        let partnership = Partnership::rehydrate(RehydratedPartnershipState {
            id: PartnershipId::new(),
            party_id: PartyId::new(),
            lifecycle: PartnershipLifecycle::Dissolved,
        });

        assert_eq!(PartnershipLifecycle::Dissolved, partnership.lifecycle());
    }
}
