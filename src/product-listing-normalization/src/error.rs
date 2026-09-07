use product_listing_core::source_listing_id::InvalidSourceListingId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormalizationFailureScope {
    CandidateData,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceField {
    Price,
    EstimateMin,
    EstimateMax,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateTimeField {
    AuctionStart,
    AuctionEnd,
}

#[derive(Debug, thiserror::Error)]
pub enum NormalizationError {
    #[error("source listing ID is empty after trimming")]
    SourceListingIdEmpty,
    #[error("source listing ID is invalid")]
    SourceListingIdInvalid(#[source] InvalidSourceListingId),
    #[error("title is empty after trimming")]
    TitleEmpty,
    #[error("title language could not be detected")]
    TitleUnknownLanguage { text: String },
    #[error("description language could not be detected")]
    DescriptionUnknownLanguage { text: String },
    #[error("price currency could not be detected")]
    PriceUnknownCurrency { field: PriceField },
    #[error("price amount could not be parsed")]
    PriceParseError { field: PriceField },
    #[error("image URL is invalid")]
    InvalidImageUrl(#[source] url::ParseError),
    #[error("no valid images remained after validating candidates")]
    NoValidImages { candidates: usize },
    #[error("auction date-time could not be parsed")]
    DateTimeParseError { field: DateTimeField },
    #[error("availability input exceeds the maximum length")]
    AvailabilityTextTooLong { len: usize, max: usize },
    #[error("availability input contains an embedded NUL")]
    AvailabilityTextEmbeddedNul,
    #[error("availability regex set configuration is invalid")]
    AvailabilityRegexSetCompilationFailed,
}

impl NormalizationError {
    pub const fn failure_reason(&self) -> &'static str {
        match self {
            Self::SourceListingIdEmpty => "source_listing_id_empty",
            Self::SourceListingIdInvalid(_) => "source_listing_id_invalid",
            Self::TitleEmpty => "title_empty",
            Self::TitleUnknownLanguage { .. } => "title_unknown_language",
            Self::DescriptionUnknownLanguage { .. } => "description_unknown_language",
            Self::PriceUnknownCurrency {
                field: PriceField::Price,
            } => "price_unknown_currency",
            Self::PriceUnknownCurrency {
                field: PriceField::EstimateMin,
            } => "price_estimate_min_unknown_currency",
            Self::PriceUnknownCurrency {
                field: PriceField::EstimateMax,
            } => "price_estimate_max_unknown_currency",
            Self::PriceParseError {
                field: PriceField::Price,
            } => "price_parse_error",
            Self::PriceParseError {
                field: PriceField::EstimateMin,
            } => "price_estimate_min_parse_error",
            Self::PriceParseError {
                field: PriceField::EstimateMax,
            } => "price_estimate_max_parse_error",
            Self::InvalidImageUrl(_) => "invalid_image_url",
            Self::NoValidImages { .. } => "no_valid_images",
            Self::DateTimeParseError {
                field: DateTimeField::AuctionStart,
            } => "auction_start_parse_error",
            Self::DateTimeParseError {
                field: DateTimeField::AuctionEnd,
            } => "auction_end_parse_error",
            Self::AvailabilityTextTooLong { .. } => "state_text_too_long",
            Self::AvailabilityTextEmbeddedNul => "state_text_embedded_nul",
            Self::AvailabilityRegexSetCompilationFailed => {
                "availability_regex_set_compilation_failed"
            }
        }
    }

    pub const fn failure_scope(&self) -> NormalizationFailureScope {
        match self {
            Self::AvailabilityRegexSetCompilationFailed => NormalizationFailureScope::System,
            _ => NormalizationFailureScope::CandidateData,
        }
    }
}
