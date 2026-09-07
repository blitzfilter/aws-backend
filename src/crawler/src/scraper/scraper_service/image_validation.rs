use crate::network::policy::{
    PublicTargetError, classify_reqwest_error, public_http_client, redirect_target,
    resolve_public_http_target,
};
use crate::scraper::css_selector::rule::split_image_candidate_group;
use crate::scraper::normalization::error::NormalizationError;
use regex::regex;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use url::Url;

const MIN_LONGEST_SIDE: usize = 400;
const MIN_SHORTEST_SIDE: usize = 250;
const IMAGE_PROBE_BYTES: &str = "bytes=0-32767";
const IMAGE_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
const IMAGE_PROBE_MAX_REDIRECTS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageValidation {
    Valid,
    Invalid,
    Unknown,
}

#[async_trait::async_trait]
pub(crate) trait ImageValidator: Send + Sync {
    async fn validate(&self, url: &Url) -> ImageValidation;
}

pub(crate) struct ReqwestImageValidator {
    cache: Mutex<HashMap<String, ImageValidation>>,
}

impl ReqwestImageValidator {
    pub(crate) fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for ReqwestImageValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ImageValidator for ReqwestImageValidator {
    async fn validate(&self, url: &Url) -> ImageValidation {
        // Quality may be cached, but target safety is checked for every acceptance.
        // DNS answers can change after a prior successful image probe.
        if let Err(error) = resolve_public_http_target(url, IMAGE_PROBE_TIMEOUT).await {
            return image_validation_for_target_error(error);
        }

        let key = url.as_str().to_owned();
        if let Some(cached) = self.cache.lock().expect("cache lock").get(&key).copied() {
            return cached;
        }

        let validation = match validate_image_url(url) {
            Some(validation) => validation,
            None => probe_image_dimensions(url).await,
        };

        self.cache
            .lock()
            .expect("cache lock")
            .insert(key, validation);
        validation
    }
}

pub(crate) async fn filter_valid_image_urls(
    raw_images: Vec<String>,
    base_url: &Url,
    validator: &dyn ImageValidator,
) -> Result<Vec<String>, NormalizationError> {
    let mut seen = HashSet::new();
    let mut valid = Vec::new();
    let mut invalid_count = 0usize;

    for raw in raw_images {
        let candidates = split_image_candidate_group(&raw);
        if candidates.is_empty() {
            invalid_count += 1;
            continue;
        }

        for raw_candidate in candidates {
            let trimmed = raw_candidate.trim();
            if trimmed.is_empty() {
                invalid_count += 1;
                continue;
            }

            let resolved = match Url::parse(trimmed).or_else(|_| base_url.join(trimmed)) {
                Ok(url) => url,
                Err(_) => {
                    tracing::debug!("Discarding invalid image URL candidate");
                    invalid_count += 1;
                    continue;
                }
            };

            // Security validation always runs first. Deterministic local quality
            // rejections may then discard a target without replacing that result.
            let validation = match (
                validator.validate(&resolved).await,
                validate_image_url(&resolved),
            ) {
                (_, Some(ImageValidation::Invalid)) => ImageValidation::Invalid,
                (validation, _) => validation,
            };
            match validation {
                ImageValidation::Invalid => invalid_count += 1,
                ImageValidation::Valid | ImageValidation::Unknown => {
                    let normalized = resolved.to_string();
                    if seen.insert(normalized.clone()) {
                        valid.push(normalized);
                    }
                    break;
                }
            }
        }
    }

    if valid.is_empty() {
        return Err(NormalizationError::NoValidImages {
            candidates: invalid_count,
        });
    }

    Ok(valid)
}

fn validate_image_url(url: &Url) -> Option<ImageValidation> {
    if let Some((width, height)) = dimensions_from_query(url) {
        return Some(validation_for_dimensions(width, height));
    }
    if let Some((width, height)) = dimensions_from_path(url.path()) {
        return Some(validation_for_dimensions(width, height));
    }

    let lower = url.as_str().to_ascii_lowercase();
    if lower.contains("thumbnail") || lower.contains("thumb") {
        return Some(ImageValidation::Invalid);
    }

    None
}

fn validation_for_dimensions(width: usize, height: usize) -> ImageValidation {
    let longest = width.max(height);
    let shortest = width.min(height);
    if longest >= MIN_LONGEST_SIDE && shortest >= MIN_SHORTEST_SIDE {
        ImageValidation::Valid
    } else {
        ImageValidation::Invalid
    }
}

fn dimensions_from_query(url: &Url) -> Option<(usize, usize)> {
    let mut width = None;
    let mut height = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "w" | "width" => width = value.parse::<usize>().ok(),
            "h" | "height" => height = value.parse::<usize>().ok(),
            _ => {}
        }
    }
    Some((width?, height?))
}

fn dimensions_from_path(path: &str) -> Option<(usize, usize)> {
    let captures = regex!(r"(?i)[-_/](\d{2,5})x(\d{2,5})(?:[._/?-]|$)").captures(path)?;
    let width = captures.get(1)?.as_str().parse::<usize>().ok()?;
    let height = captures.get(2)?.as_str().parse::<usize>().ok()?;
    Some((width, height))
}

async fn probe_image_dimensions(url: &Url) -> ImageValidation {
    let mut current_url = url.clone();
    let mut redirect_count = 0;
    let response = loop {
        let client = match public_http_client(&current_url, IMAGE_PROBE_TIMEOUT, true).await {
            Ok(client) => client,
            Err(error) => {
                tracing::debug!("Image dimension probe rejected target");
                return image_validation_for_target_error(error);
            }
        };
        let response = match client
            .get(current_url.clone())
            .header(reqwest::header::RANGE, IMAGE_PROBE_BYTES)
            .send()
            .await
        {
            Ok(response) => response,
            Err(err) => {
                tracing::debug!(
                    kind = ?classify_reqwest_error(&err),
                    "Image dimension probe failed"
                );
                return ImageValidation::Unknown;
            }
        };
        if !response.status().is_redirection() {
            break response;
        }
        let next_url = match redirect_target(&current_url, &response) {
            Ok(url) => url,
            Err(error) => {
                tracing::debug!("Image dimension probe rejected redirect");
                return image_validation_for_target_error(error);
            }
        };
        if redirect_count == IMAGE_PROBE_MAX_REDIRECTS || current_url == next_url {
            return ImageValidation::Unknown;
        }
        redirect_count += 1;
        current_url = next_url;
    };

    let response = match response.error_for_status() {
        Ok(response) => response,
        Err(err) => {
            tracing::debug!(
                kind = ?classify_reqwest_error(&err),
                "Image dimension probe returned non-success status"
            );
            return ImageValidation::Unknown;
        }
    };

    let bytes = match read_probe_bytes(response).await {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::debug!(
                kind = ?classify_reqwest_error(&err),
                "Image dimension probe body read failed"
            );
            return ImageValidation::Unknown;
        }
    };

    match imagesize::blob_size(&bytes) {
        Ok(size) => validation_for_dimensions(size.width, size.height),
        Err(_) => {
            tracing::debug!("Image dimension parser could not read probed bytes");
            ImageValidation::Unknown
        }
    }
}

fn image_validation_for_target_error(error: PublicTargetError) -> ImageValidation {
    match error {
        // A URL is accepted only after current DNS resolution has established a
        // safe public target. Quality may remain unknown after that point, but
        // unresolved targets must never reach canonical ProductListing state.
        PublicTargetError::InvalidUrl
        | PublicTargetError::IpLiteral
        | PublicTargetError::InvalidPort
        | PublicTargetError::ResolutionTimeout
        | PublicTargetError::Resolution
        | PublicTargetError::UnsafeResolution
        | PublicTargetError::InvalidRedirect => ImageValidation::Invalid,
    }
}

async fn read_probe_bytes(mut response: reqwest::Response) -> Result<Vec<u8>, reqwest::Error> {
    let mut bytes = Vec::with_capacity(32 * 1024);
    while bytes.len() < 32 * 1024 {
        let Some(chunk) = response.chunk().await? else {
            break;
        };
        let remaining = 32 * 1024 - bytes.len();
        bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::css_selector::rule::IMAGE_CANDIDATE_SEPARATOR;

    struct StaticValidator {
        validation: ImageValidation,
    }

    #[async_trait::async_trait]
    impl ImageValidator for StaticValidator {
        async fn validate(&self, _: &Url) -> ImageValidation {
            self.validation
        }
    }

    #[test]
    fn rejects_small_dimensions_from_path() {
        let url = Url::parse("https://example.com/photo-100x100.jpg").unwrap();
        assert_eq!(validate_image_url(&url), Some(ImageValidation::Invalid));
    }

    #[test]
    fn accepts_large_dimensions_from_query() {
        let url = Url::parse("https://example.com/photo.jpg?width=640&height=640").unwrap();
        assert_eq!(validate_image_url(&url), Some(ImageValidation::Valid));
    }

    #[tokio::test]
    async fn rejects_private_image_url_even_when_the_path_has_large_dimensions() {
        let base = Url::parse("https://example.com/products/1").unwrap();
        let result = filter_valid_image_urls(
            vec!["http://127.0.0.1/image-800x600.jpg".to_string()],
            &base,
            &ReqwestImageValidator::new(),
        )
        .await;

        assert!(matches!(
            result,
            Err(NormalizationError::NoValidImages { .. })
        ));
    }

    #[tokio::test]
    async fn rejects_metadata_image_url_even_when_query_has_large_dimensions() {
        let base = Url::parse("https://example.com/products/1").unwrap();
        let result = filter_valid_image_urls(
            vec!["http://169.254.169.254/photo.jpg?width=800&height=600".to_string()],
            &base,
            &ReqwestImageValidator::new(),
        )
        .await;

        assert!(matches!(
            result,
            Err(NormalizationError::NoValidImages { .. })
        ));
    }

    #[tokio::test]
    async fn filters_invalid_images_but_keeps_valid_ones() {
        let base = Url::parse("https://example.com/products/1").unwrap();
        let result = filter_valid_image_urls(
            vec![
                "/image-100x100.jpg".to_string(),
                "/image-800x600.jpg".to_string(),
            ],
            &base,
            &StaticValidator {
                validation: ImageValidation::Unknown,
            },
        )
        .await
        .unwrap();

        assert_eq!(result, vec!["https://example.com/image-800x600.jpg"]);
    }

    #[tokio::test]
    async fn accepts_first_valid_candidate_from_each_ordered_image_group() {
        let base = Url::parse("https://example.com/products/1").unwrap();
        let result = filter_valid_image_urls(
            vec![
                format!("/image-100x100.jpg{IMAGE_CANDIDATE_SEPARATOR}/image-800x600.jpg"),
                format!("/other-80x80.jpg{IMAGE_CANDIDATE_SEPARATOR}/other-640x480.jpg"),
            ],
            &base,
            &StaticValidator {
                validation: ImageValidation::Unknown,
            },
        )
        .await
        .unwrap();

        assert_eq!(
            result,
            vec![
                "https://example.com/image-800x600.jpg",
                "https://example.com/other-640x480.jpg"
            ]
        );
    }

    #[tokio::test]
    async fn fails_when_all_images_are_invalid() {
        let base = Url::parse("https://example.com/products/1").unwrap();
        let err = filter_valid_image_urls(
            vec![format!(
                "/image-100x100.jpg{IMAGE_CANDIDATE_SEPARATOR}/image-120x120.jpg"
            )],
            &base,
            &StaticValidator {
                validation: ImageValidation::Unknown,
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(err, NormalizationError::NoValidImages { .. }));
    }

    #[tokio::test]
    async fn treats_malformed_image_url_candidates_as_invalid() {
        let base = Url::parse("https://example.com/products/1").unwrap();
        let err = filter_valid_image_urls(
            vec!["//".to_string()],
            &base,
            &StaticValidator {
                validation: ImageValidation::Unknown,
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(err, NormalizationError::NoValidImages { .. }));
    }

    #[tokio::test]
    async fn keeps_valid_fallback_candidate_after_malformed_candidate_in_same_group() {
        let base = Url::parse("https://example.com/products/1").unwrap();
        let result = filter_valid_image_urls(
            vec![format!("//{IMAGE_CANDIDATE_SEPARATOR}/image-800x600.jpg")],
            &base,
            &StaticValidator {
                validation: ImageValidation::Unknown,
            },
        )
        .await
        .unwrap();

        assert_eq!(result, vec!["https://example.com/image-800x600.jpg"]);
    }
}
