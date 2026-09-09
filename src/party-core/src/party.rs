use crate::{
    party_id::PartyId,
    party_name::PartyName,
    party_slug_id::{InvalidPartySlugId, PartySlugId},
};
use domain_primitives::change_outcome::ChangeOutcome;
use serde_email::Email;

#[derive(Debug, Clone, PartialEq)]
pub struct Party {
    id: PartyId,
    slug_id: PartySlugId,
    name: PartyName,
    contact: PartyContact,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PartyContact {
    pub phone: Option<String>,
    pub email: Option<Email>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewParty {
    pub id: PartyId,
    pub name: PartyName,
    pub contact: PartyContact,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedPartyState {
    pub id: PartyId,
    pub slug_id: String,
    pub name: PartyName,
    pub contact: PartyContact,
}

#[derive(Debug, thiserror::Error)]
pub enum RehydratePartyError {
    #[error("invalid persisted party slug")]
    InvalidSlug(#[source] InvalidPartySlugId),
}

impl Party {
    pub fn create(input: NewParty) -> Self {
        let slug_id = PartySlugId::derive(input.name.as_ref(), input.id);
        Self {
            id: input.id,
            slug_id,
            name: input.name,
            contact: input.contact,
        }
    }

    #[doc(hidden)]
    pub fn rehydrate(state: RehydratedPartyState) -> Result<Self, RehydratePartyError> {
        Ok(Self {
            id: state.id,
            slug_id: PartySlugId::raw(state.slug_id).map_err(RehydratePartyError::InvalidSlug)?,
            name: state.name,
            contact: state.contact,
        })
    }

    pub fn rename(&mut self, name: PartyName) -> ChangeOutcome {
        replace_if_changed(&mut self.name, name)
    }

    pub fn replace_contact(&mut self, contact: PartyContact) -> ChangeOutcome {
        replace_if_changed(&mut self.contact, contact)
    }

    pub fn id(&self) -> PartyId {
        self.id
    }

    pub fn slug_id(&self) -> &PartySlugId {
        &self.slug_id
    }

    pub fn name(&self) -> &PartyName {
        &self.name
    }

    pub fn contact(&self) -> &PartyContact {
        &self.contact
    }
}

fn replace_if_changed<T: PartialEq>(current: &mut T, replacement: T) -> ChangeOutcome {
    if *current == replacement {
        ChangeOutcome::Unchanged
    } else {
        *current = replacement;
        ChangeOutcome::Changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn party_name(value: &str) -> PartyName {
        PartyName::try_from(value)
            .unwrap_or_else(|error| panic!("invalid test party name: {error}"))
    }

    fn party_id() -> Result<PartyId, domain_primitives::object_id::ObjectIdError> {
        PartyId::try_from("pty_01h455vb4pex5vy7enb1p677vn")
    }

    #[test]
    fn should_always_disambiguate_slug_when_creating_party()
    -> Result<(), domain_primitives::object_id::ObjectIdError> {
        let party_id = party_id()?;
        let party = Party::create(NewParty {
            id: party_id,
            name: party_name("Antik und Stil"),
            contact: PartyContact::default(),
        });

        assert_eq!(
            "antik-und-stil-01890a5d-ac96-774b-bf1d-d5586c639f75",
            party.slug_id().as_ref()
        );
        Ok(())
    }

    #[test]
    fn should_disambiguate_fallback_slug_when_name_has_no_slug_characters()
    -> Result<(), domain_primitives::object_id::ObjectIdError> {
        let party_id = party_id()?;
        let party = Party::create(NewParty {
            id: party_id,
            name: party_name("\u{10FFFF}"),
            contact: PartyContact::default(),
        });

        assert_eq!(
            "party-01890a5d-ac96-774b-bf1d-d5586c639f75",
            party.slug_id().as_ref()
        );
        Ok(())
    }

    #[test]
    fn should_preserve_slug_when_renaming_party() {
        let mut party = Party::create(NewParty {
            id: PartyId::new(),
            name: party_name("Antik und Stil"),
            contact: PartyContact::default(),
        });

        assert!(party.rename(party_name("Neue Identität")).changed());

        assert_eq!(
            format!("antik-und-stil-{}", party.id().as_uuid()),
            party.slug_id().as_ref()
        );
        assert_eq!("Neue Identität", party.name().as_ref());
    }

    #[test]
    fn should_rehydrate_exact_valid_persisted_slug_without_comparing_name() {
        let party = Party::rehydrate(RehydratedPartyState {
            id: PartyId::new(),
            slug_id: "historic-party-slug".to_owned(),
            name: party_name("Renamed Party"),
            contact: PartyContact::default(),
        });

        let party = party.unwrap_or_else(|error| panic!("failed to rehydrate party: {error}"));

        assert_eq!("historic-party-slug", party.slug_id().as_ref());
    }

    #[test]
    fn should_reject_invalid_persisted_slug_when_rehydrating_party() {
        let party = Party::rehydrate(RehydratedPartyState {
            id: PartyId::new(),
            slug_id: "historic--party".to_owned(),
            name: party_name("Party"),
            contact: PartyContact::default(),
        });

        assert!(matches!(party, Err(RehydratePartyError::InvalidSlug(_))));
    }
}
