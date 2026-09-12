CREATE TABLE users (
    user_id uuid PRIMARY KEY,
    email text NOT NULL UNIQUE,
    first_name text,
    last_name text,
    language text,
    currency text,
    measurement_unit text,
    show_unassessed_or_sensitive_content boolean NOT NULL DEFAULT false,
    suspended boolean NOT NULL DEFAULT false,
    tier text NOT NULL,
    role text NOT NULL,
    stripe_customer_id text UNIQUE,
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT users_language_check CHECK (language IS NULL OR language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar')),
    CONSTRAINT users_currency_check CHECK (currency IS NULL OR currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR')),
    CONSTRAINT users_measurement_unit_check CHECK (measurement_unit IS NULL OR measurement_unit IN ('METRIC', 'IMPERIAL')),
    CONSTRAINT users_tier_check CHECK (tier IN ('FREE', 'PRO', 'ULTIMATE')),
    CONSTRAINT users_role_check CHECK (role IN ('USER', 'ADMIN')),
    CONSTRAINT users_version_positive CHECK (version >= 1)
);

CREATE INDEX users_created_idx ON users (created DESC);

CREATE TABLE user_cognito_identities (
    issuer text NOT NULL,
    subject text NOT NULL,
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    created timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (issuer, subject),
    CONSTRAINT user_cognito_identities_user_unique UNIQUE (user_id),
    CONSTRAINT user_cognito_identities_issuer_length CHECK (octet_length(issuer) BETWEEN 1 AND 2048),
    CONSTRAINT user_cognito_identities_subject_length CHECK (octet_length(subject) BETWEEN 1 AND 2048),
    CONSTRAINT user_cognito_identities_issuer_no_control CHECK (issuer !~ '[[:cntrl:]]'),
    CONSTRAINT user_cognito_identities_subject_no_control CHECK (subject !~ '[[:cntrl:]]')
);

CREATE TABLE parties (
    party_id uuid PRIMARY KEY,
    party_slug_id text NOT NULL,
    CONSTRAINT parties_slug_unique UNIQUE (party_slug_id),
    name text NOT NULL,
    phone text,
    email text,
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT parties_party_slug_id_format CHECK (party_slug_id ~ '^[a-z0-9]+(-[a-z0-9]+)*$'),
    CONSTRAINT parties_name_nonblank CHECK (length(trim(name)) > 0),
    CONSTRAINT parties_name_max_bytes CHECK (octet_length(name) <= 255),
    CONSTRAINT parties_version_positive CHECK (version >= 1)
);

CREATE TABLE listing_sources (
    listing_source_id uuid PRIMARY KEY,
    listing_source_slug_id text NOT NULL,
    CONSTRAINT listing_sources_slug_unique UNIQUE (listing_source_slug_id),
    name text NOT NULL,
    operator_party_id uuid NOT NULL REFERENCES parties(party_id) ON DELETE RESTRICT,
    url text,
    image text,
    referral_configuration jsonb,
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT listing_sources_listing_source_slug_id_format CHECK (listing_source_slug_id ~ '^[a-z0-9]+(-[a-z0-9]+)*$'),
    CONSTRAINT listing_sources_name_nonblank CHECK (length(trim(name)) > 0),
    CONSTRAINT listing_sources_name_max_bytes CHECK (octet_length(name) <= 255),
    CONSTRAINT listing_sources_referral_configuration_object CHECK (referral_configuration IS NULL OR jsonb_typeof(referral_configuration) = 'object'),
    CONSTRAINT listing_sources_version_positive CHECK (version >= 1)
);

CREATE TABLE listing_source_ingestion_methods (
    listing_source_id uuid NOT NULL REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    ingestion_method text NOT NULL,
    PRIMARY KEY (listing_source_id, ingestion_method),
    CONSTRAINT listing_source_ingestion_methods_check CHECK (ingestion_method IN ('WEB_CRAWL', 'SHOPIFY', 'WOOCOMMERCE', 'PARTNER_API'))
);

CREATE TABLE listing_source_web_crawl_ingestion_configurations (
    listing_source_id uuid PRIMARY KEY REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    fallback_currency text,
    CONSTRAINT listing_source_web_crawl_fallback_currency_check CHECK (fallback_currency IS NULL OR fallback_currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR'))
);

CREATE TABLE listing_source_shopify_ingestion_configurations (
    listing_source_id uuid PRIMARY KEY REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    domain text NOT NULL,
    CONSTRAINT listing_source_shopify_domain_unique UNIQUE (domain),
    currency text,
    language text,
    CONSTRAINT listing_source_shopify_currency_check CHECK (currency IS NULL OR currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR')),
    CONSTRAINT listing_source_shopify_language_check CHECK (language IS NULL OR language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar'))
);

CREATE TABLE listing_source_woocommerce_ingestion_configurations (
    listing_source_id uuid PRIMARY KEY REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    webhook_secret text,
    currency text,
    language text,
    CONSTRAINT listing_source_woocommerce_currency_check CHECK (currency IS NULL OR currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR')),
    CONSTRAINT listing_source_woocommerce_language_check CHECK (language IS NULL OR language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar'))
);

CREATE INDEX listing_sources_operator_party_id_idx ON listing_sources (operator_party_id);
CREATE INDEX listing_source_ingestion_methods_method_idx ON listing_source_ingestion_methods (ingestion_method, listing_source_id);

-- Auction is source-scoped. The source key is immutable; metadata and schedule are optional.
CREATE TABLE auctions (
    auction_id uuid PRIMARY KEY,
    listing_source_id uuid NOT NULL
        REFERENCES listing_sources(listing_source_id) ON DELETE RESTRICT,
    source_auction_id text NOT NULL,
    name_text text,
    name_language text,
    description_text text,
    description_language text,
    catalogue_url text,
    format text,
    reported_status text,
    reported_lot_count bigint,
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT auctions_source_key_unique UNIQUE (listing_source_id, source_auction_id),
    CONSTRAINT auctions_id_source_unique UNIQUE (auction_id, listing_source_id),
    CONSTRAINT auctions_source_auction_id_length_check
        CHECK (octet_length(source_auction_id) BETWEEN 1 AND 512),
    CONSTRAINT auctions_format_check CHECK (format IS NULL OR format IN ('LIVE', 'TIMED')),
    CONSTRAINT auctions_reported_status_check CHECK (reported_status IS NULL OR reported_status IN (
        'SCHEDULED', 'IN_PROGRESS', 'ENDED', 'POSTPONED', 'CANCELLED'
    )),
    CONSTRAINT auctions_reported_lot_count_check CHECK (reported_lot_count IS NULL OR reported_lot_count BETWEEN 0 AND 4294967295),
    CONSTRAINT auctions_version_positive CHECK (version >= 1),
    CONSTRAINT auctions_name_localization_shape_check CHECK ((name_text IS NULL) = (name_language IS NULL)),
    CONSTRAINT auctions_description_localization_shape_check CHECK ((description_text IS NULL) = (description_language IS NULL))
);

CREATE TABLE auction_schedule_points (
    auction_id uuid NOT NULL REFERENCES auctions(auction_id) ON DELETE CASCADE,
    role text NOT NULL,
    precision text NOT NULL,
    instant_at timestamptz,
    date_on date,
    source_timezone text,
    PRIMARY KEY (auction_id, role),
    CONSTRAINT auction_schedule_points_role_check CHECK (role IN (
        'BIDDING_OPENS', 'LIVE_STARTS', 'LOTS_BEGIN_CLOSING', 'SCHEDULED_END'
    )),
    CONSTRAINT auction_schedule_points_precision_check CHECK (precision IN ('INSTANT', 'DATE')),
    CONSTRAINT auction_schedule_points_precision_shape_check CHECK (
        (precision = 'INSTANT' AND instant_at IS NOT NULL AND date_on IS NULL)
        OR (precision = 'DATE' AND date_on IS NOT NULL AND instant_at IS NULL)
    )
);

CREATE INDEX auction_schedule_points_role_instant_idx
    ON auction_schedule_points (role, instant_at) WHERE precision = 'INSTANT';

CREATE TABLE auction_events (
    event_id uuid PRIMARY KEY,
    auction_id uuid NOT NULL REFERENCES auctions(auction_id) ON DELETE CASCADE,
    event_type text NOT NULL,
    event_type_schema_version smallint NOT NULL,
    payload jsonb NOT NULL,
    event_time timestamptz NOT NULL,
    CONSTRAINT auction_events_type_check CHECK (event_type IN ('AUCTION_DISCOVERED', 'AUCTION_CHANGED')),
    CONSTRAINT auction_events_schema_version_check CHECK (event_type_schema_version = 1),
    CONSTRAINT auction_events_payload_object_check CHECK (jsonb_typeof(payload) = 'object')
);

CREATE INDEX auction_events_auction_time_idx ON auction_events (auction_id, event_time, event_id);

CREATE TABLE auction_metadata_policy_audits (
    audit_id uuid PRIMARY KEY,
    auction_id uuid NOT NULL REFERENCES auctions(auction_id) ON DELETE CASCADE,
    actor_label text NOT NULL,
    recorded_at timestamptz NOT NULL,
    CONSTRAINT auction_metadata_policy_audits_actor_label_length_check
        CHECK (octet_length(actor_label) BETWEEN 1 AND 512)
);

CREATE TABLE auction_metadata_field_protections (
    auction_id uuid NOT NULL REFERENCES auctions(auction_id) ON DELETE CASCADE,
    field_code text NOT NULL,
    latest_audit_id uuid NOT NULL REFERENCES auction_metadata_policy_audits(audit_id) ON DELETE RESTRICT,
    PRIMARY KEY (auction_id, field_code),
    CONSTRAINT auction_metadata_field_protections_field_code_check CHECK (field_code IN (
        'NAME', 'DESCRIPTION', 'CATALOGUE_URL', 'FORMAT', 'REPORTED_STATUS',
        'REPORTED_LOT_COUNT', 'BIDDING_OPENS', 'LIVE_STARTS',
        'LOTS_BEGIN_CLOSING', 'SCHEDULED_END'
    ))
);

CREATE TABLE product_listing_raw_streams (
    product_listing_raw_stream_id uuid PRIMARY KEY,
    listing_source_id uuid NOT NULL REFERENCES listing_sources(listing_source_id) ON DELETE RESTRICT,
    ingestion_method text NOT NULL,
    source_record_key text NOT NULL,
    source_record_key_sha256 bytea NOT NULL,
    latest_revision bigint NOT NULL,
    latest_input_sha256 bytea,
    latest_provider_source_epoch_seconds bigint,
    latest_provider_source_nanoseconds integer,
    latest_provider_source_operation text,
    latest_provider_source_ordering_state text NOT NULL DEFAULT 'NO_ORDERING',
    latest_provider_source_observation_sha256 bytea,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT product_listing_raw_streams_ingestion_method_check
        CHECK (ingestion_method IN ('WEB_CRAWL', 'SHOPIFY', 'WOOCOMMERCE')),
    CONSTRAINT product_listing_raw_streams_source_record_key_max_bytes_check
        CHECK (octet_length(source_record_key) <= 4096),
    CONSTRAINT product_listing_raw_streams_source_record_key_sha256_length_check
        CHECK (octet_length(source_record_key_sha256) = 32),
    CONSTRAINT product_listing_raw_streams_latest_revision_nonnegative_check
        CHECK (latest_revision >= 0),
    CONSTRAINT product_listing_raw_streams_latest_input_sha256_length_check
        CHECK (latest_input_sha256 IS NULL OR octet_length(latest_input_sha256) = 32),
    CONSTRAINT product_listing_raw_streams_provider_source_ordering_shape_check
        CHECK (
            (
                latest_provider_source_ordering_state = 'NO_ORDERING'
                AND latest_provider_source_epoch_seconds IS NULL
                AND latest_provider_source_nanoseconds IS NULL
                AND latest_provider_source_operation IS NULL
                AND latest_provider_source_observation_sha256 IS NULL
            )
            OR (
                latest_provider_source_ordering_state = 'KNOWN'
                AND latest_provider_source_epoch_seconds IS NOT NULL
                AND latest_provider_source_nanoseconds IS NOT NULL
                AND latest_provider_source_nanoseconds BETWEEN 0 AND 999999999
                AND latest_provider_source_operation IS NOT NULL
                AND latest_provider_source_operation IN ('UPSERT', 'DELETE')
                AND latest_provider_source_observation_sha256 IS NOT NULL
                AND octet_length(latest_provider_source_observation_sha256) = 32
            )
            OR (
                latest_provider_source_ordering_state = 'UNKNOWN_DELETE'
                AND latest_provider_source_epoch_seconds IS NULL
                AND latest_provider_source_nanoseconds IS NULL
                AND latest_provider_source_operation IS NULL
                AND latest_provider_source_observation_sha256 IS NOT NULL
                AND octet_length(latest_provider_source_observation_sha256) = 32
            )
        ),
    CONSTRAINT product_listing_raw_streams_identity_unique
        UNIQUE (listing_source_id, ingestion_method, source_record_key_sha256)
);

-- Operational provider-delivery idempotency state. It is deliberately separate
-- from product_listing_raw_revisions, the sole raw-normalization CDC source.
CREATE TABLE product_listing_raw_provider_observation_receipts (
    product_listing_raw_stream_id uuid NOT NULL
        REFERENCES product_listing_raw_streams(product_listing_raw_stream_id) ON DELETE CASCADE,
    provider_scope text NOT NULL,
    provider_delivery_id text NOT NULL,
    observation_sha256 bytea NOT NULL,
    expires_at timestamptz NOT NULL DEFAULT now() + interval '90 days',
    created timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (
        product_listing_raw_stream_id,
        provider_scope,
        provider_delivery_id
    ),
    CONSTRAINT product_listing_raw_provider_observation_receipts_provider_scope_check
        CHECK (octet_length(provider_scope) BETWEEN 1 AND 128),
    CONSTRAINT product_listing_raw_provider_observation_receipts_provider_delivery_id_check
        CHECK (octet_length(provider_delivery_id) BETWEEN 1 AND 512),
    CONSTRAINT product_listing_raw_provider_observation_receipts_observation_sha256_length_check
        CHECK (octet_length(observation_sha256) = 32)
);

CREATE TABLE product_listing_raw_revisions (
    product_listing_raw_revision_id uuid PRIMARY KEY,
    generation bigint GENERATED ALWAYS AS IDENTITY UNIQUE,
    product_listing_raw_stream_id uuid NOT NULL
        REFERENCES product_listing_raw_streams(product_listing_raw_stream_id) ON DELETE CASCADE,
    revision bigint NOT NULL,
    operation text NOT NULL,
    payload_format text NOT NULL,
    payload_schema_version smallint NOT NULL,
    raw_values_schema_version smallint NOT NULL,
    source_payload jsonb NOT NULL,
    raw_values jsonb NOT NULL,
    normalization_context jsonb NOT NULL,
    provenance jsonb NOT NULL,
    input_sha256 bytea NOT NULL,
    source_event_id text,
    source_occurred_at timestamptz,
    captured_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT product_listing_raw_revisions_stream_revision_unique
        UNIQUE (product_listing_raw_stream_id, revision),
    CONSTRAINT product_listing_raw_revisions_operation_check
        CHECK (operation IN ('UPSERT', 'DELETE')),
    CONSTRAINT product_listing_raw_revisions_payload_format_check
        CHECK (payload_format IN ('CRAWLER_EXTRACTED_PRODUCT', 'SHOPIFY_PRODUCT', 'WOOCOMMERCE_PRODUCT')),
    CONSTRAINT product_listing_raw_revisions_payload_schema_version_positive_check
        CHECK (payload_schema_version >= 1),
    CONSTRAINT product_listing_raw_revisions_raw_values_schema_version_positive_check
        CHECK (raw_values_schema_version >= 1),
    CONSTRAINT product_listing_raw_revisions_source_payload_object_check
        CHECK (jsonb_typeof(source_payload) = 'object'),
    CONSTRAINT product_listing_raw_revisions_raw_values_object_check
        CHECK (jsonb_typeof(raw_values) = 'object'),
    CONSTRAINT product_listing_raw_revisions_normalization_context_object_check
        CHECK (jsonb_typeof(normalization_context) = 'object'),
    CONSTRAINT product_listing_raw_revisions_provenance_object_check
        CHECK (jsonb_typeof(provenance) = 'object'),
    CONSTRAINT product_listing_raw_revisions_input_sha256_length_check
        CHECK (octet_length(input_sha256) = 32),
    CONSTRAINT product_listing_raw_revisions_revision_positive_check
        CHECK (revision >= 1)
);

CREATE TABLE product_listing_raw_normalization_heads (
    product_listing_raw_stream_id uuid PRIMARY KEY
        REFERENCES product_listing_raw_streams(product_listing_raw_stream_id) ON DELETE CASCADE,
    last_processed_revision bigint NOT NULL DEFAULT 0,
    product_listing_id uuid,
    source_listing_id text,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT product_listing_raw_normalization_heads_last_processed_revision_nonnegative_check
        CHECK (last_processed_revision >= 0),
    CONSTRAINT product_listing_raw_normalization_heads_binding_check
        CHECK (
            (product_listing_id IS NULL AND source_listing_id IS NULL)
            OR (product_listing_id IS NOT NULL AND source_listing_id IS NOT NULL)
        )
);

CREATE TABLE product_listing_raw_normalizations (
    product_listing_raw_revision_id uuid NOT NULL
        REFERENCES product_listing_raw_revisions(product_listing_raw_revision_id) ON DELETE CASCADE,
    product_listing_raw_stream_id uuid NOT NULL
        REFERENCES product_listing_raw_streams(product_listing_raw_stream_id) ON DELETE CASCADE,
    revision bigint NOT NULL,
    normalizer_version smallint NOT NULL,
    outcome text NOT NULL,
    product_listing_id uuid,
    product_listing_event_id uuid,
    error_code text,
    created timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (product_listing_raw_revision_id, normalizer_version),
    CONSTRAINT product_listing_raw_normalizations_revision_positive_check CHECK (revision >= 1),
    CONSTRAINT product_listing_raw_normalizations_normalizer_version_positive_check
        CHECK (normalizer_version >= 1),
    CONSTRAINT product_listing_raw_normalizations_outcome_check
        CHECK (outcome IN ('APPLIED', 'NO_CHANGE', 'IGNORED', 'REJECTED')),
    CONSTRAINT product_listing_raw_normalizations_error_code_check
        CHECK (error_code IS NULL OR octet_length(error_code) <= 128)
);

CREATE INDEX product_listing_raw_normalizations_stream_revision_idx
    ON product_listing_raw_normalizations (product_listing_raw_stream_id, revision ASC);

CREATE TABLE partnerships (
    partnership_id uuid PRIMARY KEY,
    party_id uuid NOT NULL UNIQUE REFERENCES parties(party_id) ON DELETE RESTRICT,
    business_state text NOT NULL DEFAULT 'ACTIVE',
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT partnerships_business_state_check CHECK (business_state IN ('ACTIVE', 'DISSOLVED')),
    CONSTRAINT partnerships_version_positive CHECK (version >= 1)
);

CREATE INDEX partnerships_created_id_idx
    ON partnerships (created DESC, partnership_id DESC);


CREATE TABLE partnership_members (
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    partnership_id uuid NOT NULL REFERENCES partnerships(partnership_id) ON DELETE CASCADE,
    created timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, partnership_id)
);

CREATE TABLE partnership_listing_source_grants (
    partnership_id uuid NOT NULL REFERENCES partnerships(partnership_id) ON DELETE CASCADE,
    listing_source_id uuid NOT NULL REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    created timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (partnership_id, listing_source_id)
);

CREATE TABLE partnership_applications (
    partnership_application_id uuid PRIMARY KEY,
    applicant_user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    business_state text NOT NULL,
    proposal jsonb NOT NULL,
    approved_partnership_id uuid REFERENCES partnerships(partnership_id),
    approved_listing_source_id uuid REFERENCES listing_sources(listing_source_id),
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT partnership_applications_business_state_check CHECK (business_state IN ('SUBMITTED', 'IN_REVIEW', 'APPROVED', 'REJECTED', 'WITHDRAWN')),
    CONSTRAINT partnership_applications_proposal_shape_check CHECK (
        jsonb_typeof(proposal) = 'object'
        AND proposal->>'type' IN ('EXISTING_LISTING_SOURCE', 'PROPOSED_LISTING_SOURCE')
        AND (
            (
                proposal->>'type' = 'EXISTING_LISTING_SOURCE'
                AND jsonb_typeof(proposal->'listing_source_id') = 'string'
            )
            OR (
                proposal->>'type' = 'PROPOSED_LISTING_SOURCE'
                AND jsonb_typeof(proposal->'party') = 'object'
                AND jsonb_typeof(proposal->'listing_source') = 'object'
            )
        )
    ),
    CONSTRAINT partnership_applications_approval_result_check CHECK (
        (business_state = 'APPROVED'
            AND approved_partnership_id IS NOT NULL
            AND approved_listing_source_id IS NOT NULL)
        OR (business_state <> 'APPROVED'
            AND approved_partnership_id IS NULL
            AND approved_listing_source_id IS NULL)
    ),
    CONSTRAINT partnership_applications_version_positive CHECK (version >= 1)
);

CREATE INDEX partnership_members_partnership_id_idx ON partnership_members (partnership_id);
CREATE INDEX partnership_listing_source_grants_listing_source_id_idx
    ON partnership_listing_source_grants (listing_source_id, partnership_id);
CREATE INDEX partnership_applications_created_id_idx
    ON partnership_applications (created DESC, partnership_application_id DESC);
CREATE INDEX partnership_applications_updated_id_idx
    ON partnership_applications (updated DESC, partnership_application_id DESC);
CREATE INDEX partnership_applications_applicant_created_idx
    ON partnership_applications (applicant_user_id, created DESC, partnership_application_id DESC);
CREATE INDEX partnership_applications_business_state_created_idx
    ON partnership_applications (business_state, created DESC, partnership_application_id DESC);
CREATE INDEX partnership_applications_approved_source_created_id_idx
    ON partnership_applications (approved_listing_source_id, created DESC, partnership_application_id DESC)
    WHERE approved_listing_source_id IS NOT NULL;
CREATE INDEX partnership_applications_existing_source_proposal_idx
    ON partnership_applications ((proposal->>'listing_source_id'))
    WHERE proposal->>'type' = 'EXISTING_LISTING_SOURCE';

CREATE OR REPLACE FUNCTION lock_existing_listing_source_proposal()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.proposal->>'type' = 'EXISTING_LISTING_SOURCE' THEN
        PERFORM 1 FROM listing_sources
        WHERE listing_source_id = (NEW.proposal->>'listing_source_id')::uuid
        FOR KEY SHARE;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'existing ListingSource proposal refers to a missing ListingSource'
                USING ERRCODE = '23503',
                      CONSTRAINT = 'partnership_applications_existing_listing_source_id_fkey';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER partnership_applications_lock_existing_listing_source
BEFORE INSERT OR UPDATE OF proposal ON partnership_applications
FOR EACH ROW EXECUTE FUNCTION lock_existing_listing_source_proposal();

CREATE TABLE fx_rates (
    fx_rate_id uuid PRIMARY KEY,
    generation bigint GENERATED ALWAYS AS IDENTITY UNIQUE,
    captured_at timestamptz NOT NULL,
    source text NOT NULL,
    source_event_id text NOT NULL UNIQUE,
    created timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX fx_rates_captured_at_generation_idx
    ON fx_rates (captured_at DESC, generation DESC);

CREATE TABLE fx_rate_quotes (
    fx_rate_id uuid NOT NULL REFERENCES fx_rates(fx_rate_id) ON DELETE RESTRICT,
    currency text NOT NULL,
    units_per_eur bigint NOT NULL CHECK (units_per_eur > 0),
    PRIMARY KEY (fx_rate_id, currency),
    CONSTRAINT fx_rate_quotes_currency_check CHECK (currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR'))
);

CREATE TABLE product_listings (
    product_listing_id uuid PRIMARY KEY,
    product_listing_title_slug_id text NOT NULL,
    version bigint NOT NULL DEFAULT 1,
    current_event_id uuid NOT NULL,
    content_source_event_id uuid NOT NULL,
    embedding_source_event_id uuid NOT NULL,
    listing_source_id uuid NOT NULL
        REFERENCES listing_sources(listing_source_id)
        ON DELETE RESTRICT,
    source_listing_id text NOT NULL,
    title_text text,
    title_language text,
    description_text text,
    description_language text,
    price_kind text,
    price_amount bigint,
    price_currency text,
    price_estimate_min_amount bigint,
    price_estimate_min_currency text,
    price_estimate_max_amount bigint,
    price_estimate_max_currency text,
    sale_observation_fx_rate_id uuid REFERENCES fx_rates(fx_rate_id) ON DELETE RESTRICT,
    sale_observed_at timestamptz,
    availability text,
    lifecycle text NOT NULL,
    url text NOT NULL,
    product_images jsonb NOT NULL DEFAULT '[]',
    embedding real[],
    projection_version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT product_listings_listing_source_listing_unique UNIQUE (listing_source_id, source_listing_id),
    CONSTRAINT product_listings_title_slug_unique UNIQUE (product_listing_title_slug_id),
    CONSTRAINT product_listings_source_listing_id_check CHECK (
        octet_length(source_listing_id) BETWEEN 1 AND 512
        AND source_listing_id !~ '(^[[:space:]]|[[:space:]]$)'
    ),
    CONSTRAINT product_listings_title_slug_id_format CHECK (
        product_listing_title_slug_id ~ '^[a-z0-9]+(-[a-z0-9]+)*-[0-9a-f]{6}$'
    ),
    CONSTRAINT product_listings_title_slug_id_max_bytes CHECK (
        octet_length(product_listing_title_slug_id) <= 120
    ),
    CONSTRAINT product_listings_availability_check CHECK (availability IS NULL OR availability IN ('AVAILABLE', 'IN_STOCK', 'LIMITED_AVAILABILITY', 'BACK_ORDER', 'MADE_TO_ORDER', 'PRE_ORDER', 'PRE_SALE', 'UNAVAILABLE', 'RESERVED', 'OUT_OF_STOCK', 'SOLD_OUT')),
    CONSTRAINT product_listings_lifecycle_check CHECK (lifecycle IN ('ACTIVE', 'WITHDRAWN')),
    CONSTRAINT product_listings_withdrawn_availability_check CHECK (lifecycle <> 'WITHDRAWN' OR availability IS NULL),
    CONSTRAINT product_listings_title_pair_check CHECK ((title_text IS NULL) = (title_language IS NULL)),
    CONSTRAINT product_listings_description_pair_check CHECK ((description_text IS NULL) = (description_language IS NULL)),
    CONSTRAINT product_listings_title_language_check CHECK (title_language IS NULL OR title_language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar')),
    CONSTRAINT product_listings_description_language_check CHECK (description_language IS NULL OR description_language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar')),
    CONSTRAINT product_listings_price_shape_check CHECK (
        (price_kind IS NULL AND price_amount IS NULL AND price_currency IS NULL)
        OR (price_kind = 'MONETARY' AND price_amount IS NOT NULL AND price_currency IS NOT NULL)
        OR (price_kind = 'ON_REQUEST' AND price_amount IS NULL AND price_currency IS NULL)
    ),
    CONSTRAINT product_listings_price_kind_check CHECK (price_kind IS NULL OR price_kind IN ('MONETARY', 'ON_REQUEST')),
    CONSTRAINT product_listings_price_estimate_min_pair_check CHECK ((price_estimate_min_amount IS NULL) = (price_estimate_min_currency IS NULL)),
    CONSTRAINT product_listings_price_estimate_max_pair_check CHECK ((price_estimate_max_amount IS NULL) = (price_estimate_max_currency IS NULL)),
    CONSTRAINT product_listings_price_amount_nonnegative CHECK (price_amount IS NULL OR price_amount >= 0),
    CONSTRAINT product_listings_price_estimate_min_amount_nonnegative CHECK (price_estimate_min_amount IS NULL OR price_estimate_min_amount >= 0),
    CONSTRAINT product_listings_price_estimate_max_amount_nonnegative CHECK (price_estimate_max_amount IS NULL OR price_estimate_max_amount >= 0),
    CONSTRAINT product_listings_price_currency_check CHECK (price_currency IS NULL OR price_currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR')),
    CONSTRAINT product_listings_price_estimate_min_currency_check CHECK (price_estimate_min_currency IS NULL OR price_estimate_min_currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR')),
    CONSTRAINT product_listings_price_estimate_max_currency_check CHECK (price_estimate_max_currency IS NULL OR price_estimate_max_currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR')),
    CONSTRAINT product_listings_sale_observation_pair_check CHECK ((sale_observation_fx_rate_id IS NULL) = (sale_observed_at IS NULL)),
    CONSTRAINT product_listings_images_array CHECK (jsonb_typeof(product_images) = 'array'),
    CONSTRAINT product_listings_embedding_dimension_check CHECK (embedding IS NULL OR (array_ndims(embedding) = 1 AND cardinality(embedding) = 768)),
    CONSTRAINT product_listings_version_positive CHECK (version >= 1),
    CONSTRAINT product_listings_projection_version_positive CHECK (projection_version >= 1)
);

-- Listing-owned source assertions. They deliberately do not identify or join an
-- Auction aggregate: membership arrives in a later iteration.
CREATE TABLE product_listing_auction_contexts (
    product_listing_id uuid PRIMARY KEY
        REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    lot_number text,
    catalogue_position bigint,
    CONSTRAINT product_listing_auction_contexts_lot_number_check CHECK (
        lot_number IS NULL OR (
            octet_length(lot_number) BETWEEN 1 AND 128
            AND lot_number !~ '(^[[:space:]]|[[:space:]]$)'
        )
    ),
    CONSTRAINT product_listing_auction_contexts_catalogue_position_check CHECK (
        catalogue_position IS NULL OR catalogue_position BETWEEN 1 AND 4294967295
    )
);

CREATE TABLE product_listing_lot_auction_timings (
    product_listing_id uuid PRIMARY KEY
        REFERENCES product_listing_auction_contexts(product_listing_id) ON DELETE CASCADE,
    bidding_opens_precision text,
    bidding_opens_instant_at timestamptz,
    bidding_opens_date_on date,
    bidding_opens_source_timezone text,
    scheduled_closes_precision text,
    scheduled_closes_instant_at timestamptz,
    scheduled_closes_date_on date,
    scheduled_closes_source_timezone text,
    reported_closed_at timestamptz,
    CONSTRAINT product_listing_lot_auction_timings_bidding_opens_shape_check CHECK (
        (bidding_opens_precision IS NULL AND bidding_opens_instant_at IS NULL AND bidding_opens_date_on IS NULL AND bidding_opens_source_timezone IS NULL)
        OR (bidding_opens_precision = 'INSTANT' AND bidding_opens_instant_at IS NOT NULL AND bidding_opens_date_on IS NULL)
        OR (bidding_opens_precision = 'DATE' AND bidding_opens_instant_at IS NULL AND bidding_opens_date_on IS NOT NULL)
    ),
    CONSTRAINT product_listing_lot_auction_timings_scheduled_closes_shape_check CHECK (
        (scheduled_closes_precision IS NULL AND scheduled_closes_instant_at IS NULL AND scheduled_closes_date_on IS NULL AND scheduled_closes_source_timezone IS NULL)
        OR (scheduled_closes_precision = 'INSTANT' AND scheduled_closes_instant_at IS NOT NULL AND scheduled_closes_date_on IS NULL)
        OR (scheduled_closes_precision = 'DATE' AND scheduled_closes_instant_at IS NULL AND scheduled_closes_date_on IS NOT NULL)
    )
);

CREATE INDEX product_listings_listing_source_id_idx ON product_listings (listing_source_id);
CREATE INDEX product_listings_lifecycle_updated_idx ON product_listings (lifecycle, updated DESC);
CREATE INDEX product_listings_sale_observation_fx_rate_id_idx ON product_listings (sale_observation_fx_rate_id);

CREATE TABLE product_listing_translations (
    product_listing_id uuid NOT NULL REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    source_event_id uuid NOT NULL,
    language text NOT NULL,
    title text,
    description text,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (product_listing_id, language),
    CONSTRAINT product_listing_translations_language_check CHECK (language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar')),
    CONSTRAINT product_listing_translations_has_content CHECK (title IS NOT NULL OR description IS NOT NULL)
);

CREATE INDEX product_listing_translations_language_idx ON product_listing_translations (language);

CREATE TABLE product_listing_events (
    event_id uuid PRIMARY KEY,
    product_listing_id uuid NOT NULL REFERENCES product_listings(product_listing_id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    event_type text NOT NULL,
    event_group text NOT NULL,
    event_type_schema_version smallint NOT NULL,
    payload jsonb NOT NULL,
    event_time timestamptz NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT product_listing_events_product_event_unique UNIQUE (product_listing_id, event_id),
    CONSTRAINT product_listing_events_group_check CHECK (event_group IN ('DOMAIN', 'ENRICHMENT')),
    CONSTRAINT product_listing_events_type_group_check CHECK (
        (
            event_group = 'DOMAIN'
            AND event_type IN (
                'PRODUCT_LISTING_DISCOVERED',
                'PRODUCT_LISTING_CHANGED'
            )
        )
        OR
        (
            event_group = 'ENRICHMENT'
            AND event_type IN (
                'ENRICHMENT_EMBEDDED',
                'ENRICHMENT_TRANSLATED_TITLES'
            )
        )
    ),
    CONSTRAINT product_listing_events_schema_version_positive CHECK (event_type_schema_version >= 1),
    CONSTRAINT product_listing_events_payload_object CHECK (jsonb_typeof(payload) = 'object')
);

ALTER TABLE product_listings
    ADD CONSTRAINT product_listings_current_event_id_same_product_fkey
    FOREIGN KEY (product_listing_id, current_event_id)
    REFERENCES product_listing_events(product_listing_id, event_id)
    DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE product_listings
    ADD CONSTRAINT product_listings_content_source_event_same_product_fkey
    FOREIGN KEY (product_listing_id, content_source_event_id)
    REFERENCES product_listing_events(product_listing_id, event_id)
    DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE product_listings
    ADD CONSTRAINT product_listings_embedding_source_event_same_product_fkey
    FOREIGN KEY (product_listing_id, embedding_source_event_id)
    REFERENCES product_listing_events(product_listing_id, event_id)
    DEFERRABLE INITIALLY DEFERRED;

ALTER TABLE product_listing_translations
    ADD CONSTRAINT product_listing_translations_source_event_fkey
    FOREIGN KEY (product_listing_id, source_event_id)
    REFERENCES product_listing_events(product_listing_id, event_id)
    DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE product_listing_content_assessments (
    product_listing_id uuid PRIMARY KEY
        REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    source_event_id uuid NOT NULL,
    decision text NOT NULL,
    category text,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT product_listing_content_assessments_decision_check
        CHECK (decision IN ('ALLOWED', 'REQUIRES_CONSENT')),
    CONSTRAINT product_listing_content_assessments_category_check
        CHECK (category IS NULL OR category IN ('NAZI_GERMANY')),
    CONSTRAINT product_listing_content_assessments_decision_category_check
        CHECK ((decision = 'ALLOWED' AND category IS NULL)
            OR (decision = 'REQUIRES_CONSENT' AND category IS NOT NULL)),
    CONSTRAINT product_listing_content_assessments_source_event_fkey
        FOREIGN KEY (product_listing_id, source_event_id)
        REFERENCES product_listing_events(product_listing_id, event_id)
        ON DELETE CASCADE
);

CREATE INDEX product_listing_events_product_listing_time_event_idx
    ON product_listing_events (product_listing_id, event_time ASC, event_id ASC);
CREATE INDEX product_listing_translations_source_event_idx
    ON product_listing_translations (product_listing_id, source_event_id);
CREATE INDEX product_listing_events_type_time_idx ON product_listing_events (event_type, event_time ASC);

CREATE TABLE product_listing_watchlist (
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    product_listing_id uuid NOT NULL REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    notifications boolean NOT NULL DEFAULT true,
    state text NOT NULL,
    active_since timestamptz,
    notifications_enabled_since timestamptz,
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, product_listing_id),
    CONSTRAINT product_listing_watchlist_state_check CHECK (state IN ('ACTIVE', 'INACTIVE_BY_USER', 'INACTIVE_BY_RESTRICTED_PLAN')),
    CONSTRAINT product_listing_watchlist_active_since_check
        CHECK ((state = 'ACTIVE') = (active_since IS NOT NULL)),
    CONSTRAINT product_listing_watchlist_notifications_enabled_since_check
        CHECK (notifications = (notifications_enabled_since IS NOT NULL)),
    CONSTRAINT product_listing_watchlist_version_positive
        CHECK (version >= 1)
);

CREATE INDEX product_listing_watchlist_user_created_product_listing_idx
    ON product_listing_watchlist (user_id, created DESC, product_listing_id ASC);
CREATE INDEX product_listing_watchlist_product_user_idx
    ON product_listing_watchlist (product_listing_id, user_id ASC);
CREATE INDEX product_listing_watchlist_product_active_since_idx
    ON product_listing_watchlist (product_listing_id, active_since, user_id)
    WHERE state = 'ACTIVE';

CREATE TABLE search_filters (
    user_search_filter_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    name text NOT NULL,
    notifications boolean NOT NULL DEFAULT true,
    state text NOT NULL,
    search jsonb NOT NULL,
    enhanced_search_description text,
    embedding real[],
    language text NOT NULL,
    currency text NOT NULL,
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT search_filters_user_id_unique UNIQUE (user_search_filter_id, user_id),
    CONSTRAINT search_filters_state_check CHECK (state IN ('ACTIVE', 'INACTIVE_BY_USER', 'INACTIVE_BY_RESTRICTED_PLAN')),
    CONSTRAINT search_filters_search_object CHECK (jsonb_typeof(search) = 'object'),
    CONSTRAINT search_filters_language_check CHECK (language IN ('de', 'en', 'fr', 'es', 'it', 'zh', 'pt', 'pl', 'tr', 'nl', 'cs', 'ja', 'ru', 'ar')),
    CONSTRAINT search_filters_currency_check CHECK (currency IN ('EUR', 'GBP', 'USD', 'AUD', 'CAD', 'NZD', 'CNY', 'BRL', 'PLN', 'TRY', 'JPY', 'CZK', 'RUB', 'AED', 'SAR', 'HKD', 'SGD', 'CHF', 'ZAR')),
    CONSTRAINT search_filters_embedding_dimension_check CHECK (embedding IS NULL OR (array_ndims(embedding) = 1 AND cardinality(embedding) = 768)),
    CONSTRAINT search_filters_enhanced_description_non_blank CHECK (enhanced_search_description IS NULL OR btrim(enhanced_search_description) <> ''),
    CONSTRAINT search_filters_version_positive CHECK (version >= 1)
);

CREATE INDEX search_filters_user_created_idx ON search_filters (user_id, created DESC);
CREATE INDEX search_filters_state_updated_idx ON search_filters (state, updated DESC);
CREATE INDEX search_filters_periodic_match_eligible_idx
    ON search_filters (user_search_filter_id)
    WHERE state = 'ACTIVE'
      AND enhanced_search_description IS NOT NULL;

CREATE TABLE search_filter_periodic_match_state (
    user_search_filter_id uuid PRIMARY KEY
        REFERENCES search_filters(user_search_filter_id)
        ON DELETE CASCADE,
    matched_through timestamptz NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE search_filters REPLICA IDENTITY FULL;

CREATE TABLE search_filter_matches (
    user_id uuid NOT NULL,
    user_search_filter_id uuid NOT NULL,
    product_listing_id uuid NOT NULL REFERENCES product_listings(product_listing_id) ON DELETE CASCADE,
    origin_event_id uuid NOT NULL,
    price_valuation_basis text,
    price_fx_rate_id uuid REFERENCES fx_rates(fx_rate_id) ON DELETE RESTRICT,
    user_search_filter_name text,
    enhanced_match_reason text,
    feedback boolean,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_search_filter_id, product_listing_id),
    CONSTRAINT search_filter_matches_filter_owner_fkey
        FOREIGN KEY (user_search_filter_id, user_id)
        REFERENCES search_filters(user_search_filter_id, user_id)
        ON DELETE CASCADE,
    CONSTRAINT search_filter_matches_origin_event_product_fkey
        FOREIGN KEY (product_listing_id, origin_event_id)
        REFERENCES product_listing_events(product_listing_id, event_id),
    CONSTRAINT search_filter_matches_price_valuation_check CHECK (
        (price_valuation_basis IS NULL AND price_fx_rate_id IS NULL)
        OR (
            price_valuation_basis IN ('CURRENT', 'EVENT', 'SALE_OBSERVATION')
            AND price_fx_rate_id IS NOT NULL
        )
    )
);

CREATE INDEX search_filter_matches_user_created_idx ON search_filter_matches (user_id, created DESC);
CREATE INDEX search_filter_matches_user_filter_created_idx ON search_filter_matches (user_id, user_search_filter_id, created DESC);
CREATE INDEX search_filter_matches_filter_created_product_listing_idx
    ON search_filter_matches (user_search_filter_id, created ASC, product_listing_id ASC);
CREATE INDEX search_filter_matches_filter_created_desc_product_listing_idx
    ON search_filter_matches (user_search_filter_id, created DESC, product_listing_id ASC);
CREATE INDEX search_filter_matches_user_product_listing_created_idx ON search_filter_matches (user_id, product_listing_id, created ASC, user_search_filter_id ASC);
CREATE INDEX search_filter_matches_user_created_rank_idx ON search_filter_matches (user_id, created ASC, user_search_filter_id ASC, product_listing_id ASC);
CREATE INDEX search_filter_matches_product_listing_id_idx ON search_filter_matches (product_listing_id);
CREATE INDEX search_filter_matches_origin_event_id_idx ON search_filter_matches (origin_event_id);

CREATE TABLE notifications (
    notification_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,

    kind text NOT NULL,

    origin_event_id uuid,
    product_listing_id uuid,
    user_search_filter_id uuid,
    partner_shop_application_id uuid,
    partnership_application_id uuid,

    payload_version smallint NOT NULL DEFAULT 1,
    payload jsonb NOT NULL,

    seen boolean NOT NULL DEFAULT false,

    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),

    CONSTRAINT notifications_kind_check CHECK (
        kind IN (
            'WATCHLIST_PRICE_CHANGED',
            'WATCHLIST_AVAILABILITY_CHANGED',
            'SEARCH_FILTER_MATCH',
            'PARTNER_APPLICATION_APPROVED',
            'PARTNER_APPLICATION_REJECTED',
            'PARTNERSHIP_APPLICATION_APPROVED',
            'PARTNERSHIP_APPLICATION_REJECTED'
        )
    ),

    CONSTRAINT notifications_payload_version_positive CHECK (
        payload_version >= 1
    ),

    CONSTRAINT notifications_payload_object CHECK (
        jsonb_typeof(payload) = 'object'
    ),

    CONSTRAINT notifications_source_shape_check CHECK (
        (
            kind IN (
                'WATCHLIST_PRICE_CHANGED',
                'WATCHLIST_AVAILABILITY_CHANGED'
            )
            AND origin_event_id IS NOT NULL
            AND product_listing_id IS NOT NULL
            AND user_search_filter_id IS NULL
            AND partner_shop_application_id IS NULL
        )
        OR
        (
            kind = 'SEARCH_FILTER_MATCH'
            AND origin_event_id IS NOT NULL
            AND product_listing_id IS NOT NULL
            AND user_search_filter_id IS NOT NULL
            AND partner_shop_application_id IS NULL
        )
        OR
        (
            kind IN (
                'PARTNER_APPLICATION_APPROVED',
                'PARTNER_APPLICATION_REJECTED'
            )
            AND origin_event_id IS NULL
            AND product_listing_id IS NULL
            AND user_search_filter_id IS NULL
            AND partner_shop_application_id IS NOT NULL
            AND partnership_application_id IS NULL
        )
        OR
        (
            kind IN (
                'PARTNERSHIP_APPLICATION_APPROVED',
                'PARTNERSHIP_APPLICATION_REJECTED'
            )
            AND origin_event_id IS NULL
            AND product_listing_id IS NULL
            AND user_search_filter_id IS NULL
            AND partner_shop_application_id IS NULL
            AND partnership_application_id IS NOT NULL
        )
    )
);

CREATE UNIQUE INDEX notifications_watchlist_identity_idx
    ON notifications (user_id, origin_event_id, kind)
    WHERE kind IN (
        'WATCHLIST_PRICE_CHANGED',
        'WATCHLIST_AVAILABILITY_CHANGED'
    );

CREATE UNIQUE INDEX notifications_search_filter_identity_idx
    ON notifications (
        user_id,
        user_search_filter_id,
        product_listing_id,
        origin_event_id
    )
    WHERE kind = 'SEARCH_FILTER_MATCH';

CREATE UNIQUE INDEX notifications_partner_application_identity_idx
    ON notifications (
        user_id,
        partner_shop_application_id
    )
    WHERE kind IN (
        'PARTNER_APPLICATION_APPROVED',
        'PARTNER_APPLICATION_REJECTED'
    );

CREATE UNIQUE INDEX notifications_partnership_application_identity_idx
    ON notifications (
        user_id,
        partnership_application_id
    )
    WHERE kind IN (
        'PARTNERSHIP_APPLICATION_APPROVED',
        'PARTNERSHIP_APPLICATION_REJECTED'
    );

CREATE INDEX notifications_user_created_idx
    ON notifications (
        user_id,
        created DESC,
        notification_id DESC
    );

CREATE INDEX notifications_user_product_listing_unseen_idx
    ON notifications (
        user_id,
        product_listing_id,
        created DESC,
        notification_id DESC
    )
    WHERE seen = false
      AND product_listing_id IS NOT NULL;

CREATE TABLE notification_deliveries (
    notification_delivery_id uuid PRIMARY KEY,
    notification_id uuid NOT NULL
        REFERENCES notifications(notification_id)
        ON DELETE CASCADE,

    channel text NOT NULL,
    target_key text NOT NULL,
    status text NOT NULL DEFAULT 'PENDING',

    attempt_count integer NOT NULL DEFAULT 0,

    lease_token uuid,
    lease_expires_at timestamptz,

    completed_lease_token uuid,
    completed_at timestamptz,

    provider_message_id text,
    last_error_code text,
    delivered_at timestamptz,

    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),

    CONSTRAINT notification_deliveries_notification_channel_target_unique
        UNIQUE (notification_id, channel, target_key),

    CONSTRAINT notification_deliveries_channel_check CHECK (
        channel = 'EMAIL'
    ),

    CONSTRAINT notification_deliveries_target_key_nonempty_check CHECK (
        length(trim(target_key)) > 0
    ),

    CONSTRAINT notification_deliveries_status_check CHECK (
        status IN (
            'PENDING',
            'PROCESSING',
            'DELIVERED',
            'FAILED'
        )
    ),

    CONSTRAINT notification_deliveries_attempt_count_nonnegative CHECK (
        attempt_count >= 0
    ),

    CONSTRAINT notification_deliveries_lease_shape_check CHECK (
        (
            status = 'PROCESSING'
            AND lease_token IS NOT NULL
            AND lease_expires_at IS NOT NULL
        )
        OR
        (
            status <> 'PROCESSING'
            AND lease_token IS NULL
            AND lease_expires_at IS NULL
        )
    ),

    CONSTRAINT notification_deliveries_completion_shape_check CHECK (
        (completed_lease_token IS NULL AND completed_at IS NULL)
        OR
        (
            completed_lease_token IS NOT NULL
            AND completed_at IS NOT NULL
            AND status <> 'PROCESSING'
        )
    ),

    CONSTRAINT notification_deliveries_delivered_shape_check CHECK (
        (
            status = 'DELIVERED'
            AND provider_message_id IS NOT NULL
            AND delivered_at IS NOT NULL
        )
        OR
        (
            status <> 'DELIVERED'
            AND provider_message_id IS NULL
            AND delivered_at IS NULL
        )
    )
);

CREATE INDEX notification_deliveries_status_created_idx
    ON notification_deliveries (
        status,
        created,
        notification_delivery_id
    );
-- Credential rows require pg_ttl_index to be installed and preloaded by database
-- provisioning. Do not create the extension here: normal application migration
-- roles need not have that privilege, and silently omitting cleanup is unsafe.
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_extension
        WHERE extname = 'pg_ttl_index'
    ) THEN
        RAISE EXCEPTION
            'pg_ttl_index must be provisioned before business schema migrations run';
    END IF;
END;
$$;

CREATE TABLE access_tokens (
    access_token_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    token_short text NOT NULL,
    token_hash text NOT NULL,
    name text NOT NULL,
    scopes text[] NOT NULL DEFAULT '{}',
    origin text NOT NULL,
    oauth_client_id uuid,
    expires_at timestamptz,
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT access_tokens_version_positive CHECK (version >= 1),
    CONSTRAINT access_tokens_origin_check CHECK (origin IN ('USER', 'OAUTH')),
    CONSTRAINT access_tokens_oauth_origin_client_check CHECK (
        (origin = 'USER' AND oauth_client_id IS NULL)
        OR (origin = 'OAUTH' AND oauth_client_id IS NOT NULL)
    ),
    CONSTRAINT access_tokens_scopes_check CHECK (
        scopes <@ ARRAY[
            'product-listings:write',
            'shops:read',
            'shops:write',
            'partner-shop-applications:write',
            'partner-shops:read',
            'partner-shops:write',
            'users:read',
            'users:write',
            'access-tokens:read',
            'access-tokens:write',
            'search-filters:write',
            'watchlist:read',
            'watchlist:write'
        ]::text[]
    ),
    CONSTRAINT access_tokens_hash_unique UNIQUE (token_short, token_hash)
);

CREATE INDEX access_tokens_user_created_idx
    ON access_tokens (user_id, created ASC, access_token_id ASC);

CREATE TABLE oauth_clients (
    client_id uuid PRIMARY KEY,
    client_secret_short_token text NOT NULL,
    client_secret_long_token_hash text NOT NULL,
    name text NOT NULL,
    redirect_uris text[] NOT NULL,
    tos_uri text NOT NULL,
    policy_uri text NOT NULL,
    client_uri text NOT NULL,
    logo_uri text NOT NULL,
    scopes text[] NOT NULL DEFAULT '{}',
    version bigint NOT NULL DEFAULT 1,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT oauth_clients_version_positive CHECK (version >= 1),
    CONSTRAINT oauth_clients_redirect_uris_nonempty CHECK (cardinality(redirect_uris) > 0),
    CONSTRAINT oauth_clients_redirect_uris_no_nulls CHECK (array_position(redirect_uris, NULL) IS NULL),
    CONSTRAINT oauth_clients_scopes_check CHECK (
        scopes <@ ARRAY[
            'product-listings:write',
            'shops:read',
            'shops:write',
            'partner-shop-applications:write',
            'partner-shops:read',
            'partner-shops:write',
            'users:read',
            'users:write',
            'access-tokens:read',
            'access-tokens:write',
            'search-filters:write',
            'watchlist:read',
            'watchlist:write'
        ]::text[]
    )
);

ALTER TABLE access_tokens
    ADD CONSTRAINT access_tokens_oauth_client_id_fkey
    FOREIGN KEY (oauth_client_id) REFERENCES oauth_clients(client_id) ON DELETE CASCADE;

CREATE INDEX access_tokens_oauth_client_id_idx
    ON access_tokens (oauth_client_id);

CREATE TABLE oauth_authorization_codes (
    authorization_code text PRIMARY KEY,
    client_id uuid NOT NULL REFERENCES oauth_clients(client_id) ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    redirect_uri text NOT NULL,
    scopes text[] NOT NULL DEFAULT '{}',
    code_challenge text NOT NULL,
    code_challenge_method text NOT NULL,
    expires_at timestamptz NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT oauth_authorization_codes_challenge_method_check
        CHECK (code_challenge_method IN ('S256')),
    CONSTRAINT oauth_authorization_codes_scopes_check CHECK (
        scopes <@ ARRAY[
            'product-listings:write',
            'shops:read',
            'shops:write',
            'partner-shop-applications:write',
            'partner-shops:read',
            'partner-shops:write',
            'users:read',
            'users:write',
            'access-tokens:read',
            'access-tokens:write',
            'search-filters:write',
            'watchlist:read',
            'watchlist:write'
        ]::text[]
    )
);

CREATE TABLE oauth_third_party_exchange_codes (
    third_party_exchange_code text PRIMARY KEY,
    access_token_id uuid NOT NULL REFERENCES access_tokens(access_token_id) ON DELETE CASCADE,
    access_token text NOT NULL,
    access_token_expires_at timestamptz,
    scopes text[] NOT NULL DEFAULT '{}',
    expires_at timestamptz NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT oauth_third_party_exchange_codes_scopes_check CHECK (
        scopes <@ ARRAY[
            'product-listings:write',
            'shops:read',
            'shops:write',
            'partner-shop-applications:write',
            'partner-shops:read',
            'partner-shops:write',
            'users:read',
            'users:write',
            'access-tokens:read',
            'access-tokens:write',
            'search-filters:write',
            'watchlist:read',
            'watchlist:write'
        ]::text[]
    )
);

CREATE INDEX oauth_third_party_exchange_codes_access_token_idx
    ON oauth_third_party_exchange_codes (access_token_id);

-- Absolute semantic expiry is stored in each table. pg-ttl cleanup is deliberately
-- asynchronous; service authentication and redemption still validate expiration.
SELECT ttl_create_index('public.access_tokens', 'expires_at', 0);
SELECT ttl_create_index('public.oauth_authorization_codes', 'expires_at', 0);
SELECT ttl_create_index('public.oauth_third_party_exchange_codes', 'expires_at', 0);
SELECT ttl_create_index(
    'public.product_listing_raw_provider_observation_receipts',
    'expires_at',
    0
);
