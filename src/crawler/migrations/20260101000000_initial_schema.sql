CREATE EXTENSION IF NOT EXISTS "pgcrypto";


CREATE TABLE IF NOT EXISTS listing_sources (
    listing_source_id   UUID        PRIMARY KEY,
    listing_source_name TEXT        NOT NULL,
    listing_source_slug TEXT        NOT NULL,
    crawl_enabled       BOOLEAN     NOT NULL DEFAULT FALSE,
    llm_calls_count     BIGINT      NOT NULL DEFAULT 0,
    created             TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated             TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS listing_source_product_schemas (
    listing_source_id         UUID        PRIMARY KEY REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    product_schema  JSONB       NOT NULL,
    created         TIMESTAMPTZ NOT NULL,
    updated         TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS listing_source_domains (
    domain_id   UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    listing_source_id     UUID        NOT NULL REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    listing_source_domain TEXT        NOT NULL,
    crawl_root_host       TEXT        NOT NULL,
    url_pattern TEXT,
    last_crawled TIMESTAMPTZ,
    crawl_failure_count INT NOT NULL DEFAULT 0,
    last_crawl_error_kind TEXT,
    next_crawl_at TIMESTAMPTZ,
    UNIQUE (listing_source_domain),
    CHECK (listing_source_domain = lower(listing_source_domain)),
    CHECK (listing_source_domain = rtrim(listing_source_domain, '.')),
    CHECK (listing_source_domain !~ '^www[.]'),
    CHECK (crawl_root_host = lower(crawl_root_host)),
    CHECK (crawl_root_host = rtrim(crawl_root_host, '.')),
    CHECK (regexp_replace(crawl_root_host, '^www[.]', '') = listing_source_domain),
    UNIQUE (listing_source_id, domain_id)
);

CREATE INDEX IF NOT EXISTS idx_listing_source_domains_listing_source_id ON listing_source_domains (listing_source_id);


CREATE TABLE IF NOT EXISTS listing_source_urls (
    listing_source_id           UUID        NOT NULL REFERENCES listing_sources(listing_source_id) ON DELETE CASCADE,
    domain_id         UUID        NOT NULL,
    FOREIGN KEY (listing_source_id, domain_id)
        REFERENCES listing_source_domains (listing_source_id, domain_id)
        ON DELETE CASCADE,
    url               TEXT        NOT NULL,
    url_class         TEXT        NOT NULL,
    crawler_disposition               TEXT        NOT NULL DEFAULT 'ACTIVE',
    last_scraped_hash                 TEXT,
    last_scraped_schema_fingerprint   TEXT,
    last_captured_raw_input_sha256    BYTEA,
    last_scraped                      TIMESTAMPTZ,
    failure_count         INT         NOT NULL DEFAULT 0,
    last_error_kind       TEXT,
    last_error_message    TEXT,
    last_status_code      INT,
    next_retry_at         TIMESTAMPTZ,
    created           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (char_length(url) > 0),
    CHECK (url_class IN ('product', 'category', 'imprint', 'info', 'other')),
    CHECK (crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD', 'DORMANT_REMOVED')),
    CHECK (last_captured_raw_input_sha256 IS NULL OR octet_length(last_captured_raw_input_sha256) = 32),
    PRIMARY KEY (url)
);

CREATE INDEX IF NOT EXISTS idx_listing_source_urls_class_last_scraped ON listing_source_urls (url_class, last_scraped);
CREATE INDEX IF NOT EXISTS idx_listing_source_urls_domain_id ON listing_source_urls (domain_id);
CREATE INDEX IF NOT EXISTS idx_listing_source_urls_next_retry_at ON listing_source_urls (next_retry_at);
CREATE INDEX IF NOT EXISTS idx_listing_source_domains_next_crawl_at ON listing_source_domains (next_crawl_at);
