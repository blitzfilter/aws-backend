#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum_macros::EnumIter)]
pub enum PartnershipLifecycle {
    Active,
    Dissolved,
}

impl PartnershipLifecycle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Dissolved => "DISSOLVED",
        }
    }

    pub fn from_code(value: &str) -> Option<Self> {
        use strum::IntoEnumIterator;

        Self::iter().find(|lifecycle| lifecycle.as_str() == value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use strum::IntoEnumIterator;

    #[test]
    fn should_use_exact_lifecycle_codes() {
        assert_eq!("ACTIVE", PartnershipLifecycle::Active.as_str());
        assert_eq!("DISSOLVED", PartnershipLifecycle::Dissolved.as_str());
    }

    #[test]
    fn should_parse_each_exact_lifecycle_code() {
        for lifecycle in PartnershipLifecycle::iter() {
            assert_eq!(
                Some(lifecycle),
                PartnershipLifecycle::from_code(lifecycle.as_str())
            );
        }

        assert_eq!(None, PartnershipLifecycle::from_code("active"));
        assert_eq!(None, PartnershipLifecycle::from_code("RETIRED"));
    }

    #[test]
    fn should_use_unique_lifecycle_codes() {
        let lifecycles = PartnershipLifecycle::iter().collect::<Vec<_>>();

        assert_eq!(
            lifecycles.len(),
            lifecycles
                .iter()
                .map(|lifecycle| lifecycle.as_str())
                .collect::<HashSet<_>>()
                .len()
        );
    }
}
