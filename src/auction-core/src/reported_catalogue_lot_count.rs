#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReportedCatalogueLotCount(u32);

impl ReportedCatalogueLotCount {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u32 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::ReportedCatalogueLotCount;

    #[test]
    fn should_retain_an_explicit_empty_catalogue_report() {
        assert_eq!(0, ReportedCatalogueLotCount::new(0).value());
    }
}
