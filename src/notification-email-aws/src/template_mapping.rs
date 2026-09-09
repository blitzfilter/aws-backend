use localization::Language;

use notification_core::{
    notification::{
        LocalizedNotificationContent, LocalizedNotificationWatchlistChange, NotificationContent,
        NotificationWatchlistChange, PartnershipApplicationDecision,
    },
    presentation::present_image,
};
use notification_service::ports::notification_delivery_repository::NotificationDeliverySource;
use product_listing_core::{
    listing_availability::ListingAvailability, product_listing_price::ProductListingPrice,
    product_listing_slug_id::ProductListingSlugId,
};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmailLanguage {
    De,
    En,
    Fr,
    Es,
    It,
}

impl EmailLanguage {
    pub(crate) const fn resolve(language: Language) -> Self {
        match language {
            Language::De => Self::De,
            Language::Fr => Self::Fr,
            Language::Es => Self::Es,
            Language::It => Self::It,
            Language::En
            | Language::Zh
            | Language::Pt
            | Language::Pl
            | Language::Tr
            | Language::Nl
            | Language::Cs
            | Language::Ja
            | Language::Ru
            | Language::Ar => Self::En,
        }
    }

    pub(crate) const fn as_language(self) -> Language {
        match self {
            Self::De => Language::De,
            Self::En => Language::En,
            Self::Fr => Language::Fr,
            Self::Es => Language::Es,
            Self::It => Language::It,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::De => "de",
            Self::En => "en",
            Self::Fr => "fr",
            Self::Es => "es",
            Self::It => "it",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmailTemplateType {
    WatchlistUpdatePrice,
    WatchlistUpdateAvailability,
    SearchFilterMatch,
    PartnershipApplicationApproval,
    PartnershipApplicationRejection,
}

pub(crate) const fn template_type(content: &NotificationContent) -> EmailTemplateType {
    match content {
        NotificationContent::Watchlist {
            change: NotificationWatchlistChange::PriceChange { .. },
            ..
        } => EmailTemplateType::WatchlistUpdatePrice,
        NotificationContent::Watchlist { .. } => EmailTemplateType::WatchlistUpdateAvailability,
        NotificationContent::SearchFilter { .. } => EmailTemplateType::SearchFilterMatch,
        NotificationContent::PartnershipApplication {
            decision: PartnershipApplicationDecision::Approved,
            ..
        } => EmailTemplateType::PartnershipApplicationApproval,
        NotificationContent::PartnershipApplication { .. } => {
            EmailTemplateType::PartnershipApplicationRejection
        }
    }
}

pub(crate) const fn template_directory(template_type: EmailTemplateType) -> &'static str {
    match template_type {
        EmailTemplateType::WatchlistUpdatePrice => "mjml/watchlist/product-update/price",
        EmailTemplateType::WatchlistUpdateAvailability => {
            "mjml/watchlist/product-update/availability"
        }
        EmailTemplateType::SearchFilterMatch => "mjml/search-filter/match",
        EmailTemplateType::PartnershipApplicationApproval => {
            "mjml/partnership-application/approval"
        }
        EmailTemplateType::PartnershipApplicationRejection => {
            "mjml/partnership-application/rejection"
        }
    }
}

pub(crate) fn s3_template_key(
    stage: &str,
    commit_sha: &str,
    template_type: EmailTemplateType,
    language: EmailLanguage,
) -> String {
    format!(
        "{stage}/{commit_sha}/{}/{}.html",
        template_directory(template_type),
        language.as_str()
    )
}

pub(crate) const fn ses_template_tag_value(template_type: EmailTemplateType) -> &'static str {
    match template_type {
        EmailTemplateType::WatchlistUpdatePrice => "WATCHLIST_UPDATE_PRICE",
        EmailTemplateType::WatchlistUpdateAvailability => "WATCHLIST_UPDATE_AVAILABILITY",
        EmailTemplateType::SearchFilterMatch => "SEARCH_FILTER_MATCH",
        EmailTemplateType::PartnershipApplicationApproval => "PARTNERSHIP_APPLICATION_APPROVAL",
        EmailTemplateType::PartnershipApplicationRejection => "PARTNERSHIP_APPLICATION_REJECTION",
    }
}

pub(crate) const fn subject(
    template_type: EmailTemplateType,
    language: EmailLanguage,
) -> &'static str {
    match language {
        EmailLanguage::De => match template_type {
            EmailTemplateType::WatchlistUpdatePrice => {
                "Der Preis auf deiner Merkliste hat sich geändert"
            }
            EmailTemplateType::WatchlistUpdateAvailability => {
                "Die Verfügbarkeit eines Artikels auf deiner Merkliste hat sich geändert"
            }
            EmailTemplateType::SearchFilterMatch => "Neuer Treffer für deinen Suchfilter",
            EmailTemplateType::PartnershipApplicationApproval => "Partnerschaftsantrag genehmigt",
            EmailTemplateType::PartnershipApplicationRejection => {
                "Update zu deinem Partnerschaftsantrag"
            }
        },
        EmailLanguage::Fr => match template_type {
            EmailTemplateType::WatchlistUpdatePrice => {
                "Le prix de votre liste de souhaits a changé"
            }
            EmailTemplateType::WatchlistUpdateAvailability => {
                "La disponibilité d’un article de votre liste de souhaits a changé"
            }
            EmailTemplateType::SearchFilterMatch => {
                "Nouveau résultat pour votre filtre de recherche"
            }
            EmailTemplateType::PartnershipApplicationApproval => "Demande de partenariat approuvée",
            EmailTemplateType::PartnershipApplicationRejection => {
                "Mise à jour de votre demande de partenariat"
            }
        },
        EmailLanguage::Es => match template_type {
            EmailTemplateType::WatchlistUpdatePrice => {
                "El precio de tu lista de deseos ha cambiado"
            }
            EmailTemplateType::WatchlistUpdateAvailability => {
                "La disponibilidad de un artículo de tu lista de deseos ha cambiado"
            }
            EmailTemplateType::SearchFilterMatch => "Nuevo resultado para tu filtro de búsqueda",
            EmailTemplateType::PartnershipApplicationApproval => "Solicitud de asociación aprobada",
            EmailTemplateType::PartnershipApplicationRejection => {
                "Actualización de tu solicitud de asociación"
            }
        },
        EmailLanguage::It => match template_type {
            EmailTemplateType::WatchlistUpdatePrice => {
                "Il prezzo della tua lista dei desideri è cambiato"
            }
            EmailTemplateType::WatchlistUpdateAvailability => {
                "La disponibilità di un articolo nella tua lista dei desideri è cambiata"
            }
            EmailTemplateType::SearchFilterMatch => "Nuovo risultato per il tuo filtro di ricerca",
            EmailTemplateType::PartnershipApplicationApproval => {
                "Richiesta di partnership approvata"
            }
            EmailTemplateType::PartnershipApplicationRejection => {
                "Aggiornamento della tua richiesta di partnership"
            }
        },
        EmailLanguage::En => match template_type {
            EmailTemplateType::WatchlistUpdatePrice => "Your watchlist price changed",
            EmailTemplateType::WatchlistUpdateAvailability => {
                "Your watchlist item's availability changed"
            }
            EmailTemplateType::SearchFilterMatch => "New search filter match",
            EmailTemplateType::PartnershipApplicationApproval => "Partnership application approved",
            EmailTemplateType::PartnershipApplicationRejection => "Partnership application update",
        },
    }
}

pub(crate) fn template_data(
    source: &NotificationDeliverySource,
    email_language: EmailLanguage,
    first_name: Option<&str>,
) -> Value {
    let localized = source
        .content
        .clone()
        .localized(&[email_language.as_language()]);
    let present_image_url = |image, content_policy| {
        present_image(
            image,
            content_policy,
            source
                .presentation_preferences
                .show_unassessed_or_sensitive_content,
        )
        .and_then(|image| image.url)
        .map(|url| url.to_string())
    };
    match localized {
        LocalizedNotificationContent::Watchlist {
            snapshot, change, ..
        } => {
            let mut data = product_template_data(
                snapshot.listing_source_name.to_string(),
                &snapshot.product_listing_title_slug_id,
                snapshot.title.map(|title| title.payload.to_string()),
                present_image_url(snapshot.image, snapshot.content_policy),
                snapshot.view_url.to_string(),
            );
            match change {
                LocalizedNotificationWatchlistChange::PriceChange {
                    old_price,
                    new_price,
                } => {
                    data["old_price"] = json!(price_text(old_price, email_language));
                    data["new_price"] = json!(price_text(new_price, email_language));
                    data["notification_type"] = json!("price_change");
                }
                LocalizedNotificationWatchlistChange::AvailabilityChange {
                    old_availability,
                    new_availability,
                } => {
                    data["old_availability"] = json!(old_availability.map(|availability| {
                        availability_text(availability, email_language.as_language())
                    }));
                    data["new_availability"] = json!(new_availability.map(|availability| {
                        availability_text(availability, email_language.as_language())
                    }));
                    data["notification_type"] = json!("availability_change");
                }
            }
            add_recipient_data(data, first_name)
        }
        LocalizedNotificationContent::SearchFilter {
            snapshot,
            user_search_filter_id,
            user_search_filter_name,
            ..
        } => {
            let mut data = product_template_data(
                snapshot.listing_source_name.to_string(),
                &snapshot.product_listing_title_slug_id,
                snapshot.title.map(|title| title.payload.to_string()),
                present_image_url(snapshot.image, snapshot.content_policy),
                snapshot.view_url.to_string(),
            );
            data["search_filter_id"] = json!(user_search_filter_id.to_string());
            data["search_filter_name"] = json!(user_search_filter_name.to_string());
            data["notification_type"] = json!("search_filter_match");
            add_recipient_data(data, first_name)
        }
        LocalizedNotificationContent::PartnershipApplication {
            snapshot, decision, ..
        } => add_recipient_data(
            json!({
                "party_name": snapshot.party_name.to_string(),
                "listing_source_name": snapshot.listing_source_name.to_string(),
                "image_url": snapshot.image.map(|image| image.to_string()),
                "notification_type": match decision {
                    PartnershipApplicationDecision::Approved => "partnership_application_approval",
                    PartnershipApplicationDecision::Rejected => "partnership_application_rejection",
                },
            }),
            first_name,
        ),
    }
}

fn add_recipient_data(mut data: Value, first_name: Option<&str>) -> Value {
    data["first_name"] = json!(first_name);
    data
}
fn product_template_data(
    listing_source_name: String,
    product_listing_title_slug_id: &ProductListingSlugId,
    title: Option<String>,
    image_url: Option<String>,
    view_url: String,
) -> Value {
    json!({
        "listing_source_name": listing_source_name,
        "product_listing_url": product_listing_url(product_listing_title_slug_id),
        "title": title,
        "image_url": image_url,
        "view_url": view_url,
    })
}

fn product_listing_url(product_listing_title_slug_id: &ProductListingSlugId) -> String {
    format!("https://aura-historia.com/product-listings/{product_listing_title_slug_id}")
}
fn price_text(price: Option<ProductListingPrice>, language: EmailLanguage) -> Option<String> {
    price.map(|price| match price {
        ProductListingPrice::Monetary(price) => price.format_human_readable(),
        ProductListingPrice::OnRequest => match language {
            EmailLanguage::De => "Preis auf Anfrage",
            EmailLanguage::En => "Price on request",
            EmailLanguage::Fr => "Prix sur demande",
            EmailLanguage::Es => "Precio a consultar",
            EmailLanguage::It => "Prezzo su richiesta",
        }
        .to_owned(),
    })
}

fn availability_text(availability: ListingAvailability, language: Language) -> &'static str {
    match availability {
        ListingAvailability::Available => match language {
            Language::De => "Verfügbar",
            Language::Fr => "Disponible",
            Language::Es => "Disponible",
            Language::It => "Disponibile",
            _ => "Available",
        },
        ListingAvailability::InStock => match language {
            Language::De => "Auf Lager",
            Language::Fr => "En stock",
            Language::Es => "En stock",
            Language::It => "Disponibile",
            _ => "In stock",
        },
        ListingAvailability::LimitedAvailability => match language {
            Language::De => "Begrenzt verfügbar",
            Language::Fr => "Disponibilité limitée",
            Language::Es => "Disponibilidad limitada",
            Language::It => "Disponibilità limitata",
            _ => "Limited availability",
        },
        ListingAvailability::BackOrder => match language {
            Language::De => "Nachbestellung",
            Language::Fr => "Sur commande",
            Language::Es => "Bajo pedido",
            Language::It => "Su ordinazione",
            _ => "Backorder",
        },
        ListingAvailability::MadeToOrder => match language {
            Language::De => "Auf Bestellung gefertigt",
            Language::Fr => "Fabriqué sur commande",
            Language::Es => "Fabricado por encargo",
            Language::It => "Realizzato su ordinazione",
            _ => "Made to order",
        },
        ListingAvailability::PreOrder => match language {
            Language::De => "Vorbestellung",
            Language::Fr => "Précommande",
            Language::Es => "Preventa",
            Language::It => "Preordine",
            _ => "Pre-order",
        },
        ListingAvailability::PreSale => match language {
            Language::De => "Vorverkauf",
            Language::Fr => "Vente anticipée",
            Language::Es => "Venta anticipada",
            Language::It => "Vendita anticipata",
            _ => "Presale",
        },
        ListingAvailability::Unavailable => match language {
            Language::De => "Nicht verfügbar",
            Language::Fr => "Indisponible",
            Language::Es => "No disponible",
            Language::It => "Non disponibile",
            _ => "Unavailable",
        },
        ListingAvailability::Reserved => match language {
            Language::De => "Reserviert",
            Language::Fr => "Réservé",
            Language::Es => "Reservado",
            Language::It => "Riservato",
            _ => "Reserved",
        },
        ListingAvailability::OutOfStock => match language {
            Language::De => "Nicht auf Lager",
            Language::Fr => "Rupture de stock",
            Language::Es => "Agotado",
            Language::It => "Esaurito",
            _ => "Out of stock",
        },
        ListingAvailability::SoldOut => match language {
            Language::De => "Ausverkauft",
            Language::Fr => "Épuisé",
            Language::Es => "Agotado",
            Language::It => "Esaurito",
            _ => "Sold out",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain_primitives::event_id::EventId;
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use localization::Language;
    use money::{Currency, MonetaryAmount, Price};
    use notification_core::notification_id::NotificationId;
    use notification_core::{
        notification::{
            NotificationContent, NotificationWatchlistChange, ProductListingNotificationSnapshot,
        },
        notification_delivery::{NotificationDeliveryChannel, NotificationDeliveryTargetKey},
        notification_delivery_id::NotificationDeliveryId,
    };
    use notification_service::{
        ports::notification_delivery_repository::NotificationDeliverySource,
        presentation::NotificationPresentationPreferences,
    };
    use product_listing_core::{
        content_policy::{ContentPolicyDecision, SensitiveContentCategory},
        listing_availability::ListingAvailability,
        product_listing_id::ProductListingId,
        product_listing_slug_id::ProductListingSlugId,
        source_listing_id::SourceListingId,
    };
    use rstest::rstest;
    use std::collections::HashMap;
    use url::Url;
    use user_core::user_id::UserId;

    #[rstest]
    #[case(Language::De, EmailLanguage::De)]
    #[case(Language::En, EmailLanguage::En)]
    #[case(Language::Fr, EmailLanguage::Fr)]
    #[case(Language::Es, EmailLanguage::Es)]
    #[case(Language::It, EmailLanguage::It)]
    #[case(Language::Zh, EmailLanguage::En)]
    #[case(Language::Pt, EmailLanguage::En)]
    #[case(Language::Pl, EmailLanguage::En)]
    #[case(Language::Tr, EmailLanguage::En)]
    #[case(Language::Nl, EmailLanguage::En)]
    #[case(Language::Cs, EmailLanguage::En)]
    #[case(Language::Ja, EmailLanguage::En)]
    #[case(Language::Ru, EmailLanguage::En)]
    #[case(Language::Ar, EmailLanguage::En)]
    fn should_resolve_profile_language_to_deployed_email_language(
        #[case] profile_language: Language,
        #[case] expected_email_language: EmailLanguage,
    ) {
        assert_eq!(
            expected_email_language,
            EmailLanguage::resolve(profile_language)
        );
    }

    #[test]
    fn should_use_english_template_key_for_ingestion_only_language() {
        let key = s3_template_key(
            "test",
            "commit",
            EmailTemplateType::WatchlistUpdateAvailability,
            EmailLanguage::resolve(Language::Zh),
        );

        assert_eq!(
            "test/commit/mjml/watchlist/product-update/availability/en.html",
            key
        );
    }

    #[rstest]
    #[case(
        EmailTemplateType::WatchlistUpdatePrice,
        "Der Preis auf deiner Merkliste hat sich geändert",
        "Your watchlist price changed",
        "Le prix de votre liste de souhaits a changé",
        "El precio de tu lista de deseos ha cambiado",
        "Il prezzo della tua lista dei desideri è cambiato"
    )]
    #[case(
        EmailTemplateType::WatchlistUpdateAvailability,
        "Die Verfügbarkeit eines Artikels auf deiner Merkliste hat sich geändert",
        "Your watchlist item's availability changed",
        "La disponibilité d’un article de votre liste de souhaits a changé",
        "La disponibilidad de un artículo de tu lista de deseos ha cambiado",
        "La disponibilità di un articolo nella tua lista dei desideri è cambiata"
    )]
    #[case(
        EmailTemplateType::SearchFilterMatch,
        "Neuer Treffer für deinen Suchfilter",
        "New search filter match",
        "Nouveau résultat pour votre filtre de recherche",
        "Nuevo resultado para tu filtro de búsqueda",
        "Nuovo risultato per il tuo filtro di ricerca"
    )]
    #[case(
        EmailTemplateType::PartnershipApplicationApproval,
        "Partnerschaftsantrag genehmigt",
        "Partnership application approved",
        "Demande de partenariat approuvée",
        "Solicitud de asociación aprobada",
        "Richiesta di partnership approvata"
    )]
    #[case(
        EmailTemplateType::PartnershipApplicationRejection,
        "Update zu deinem Partnerschaftsantrag",
        "Partnership application update",
        "Mise à jour de votre demande de partenariat",
        "Actualización de tu solicitud de asociación",
        "Aggiornamento della tua richiesta di partnership"
    )]
    fn should_localize_subject_for_every_email_template_and_email_language(
        #[case] template: EmailTemplateType,
        #[case] expected_de: &str,
        #[case] expected_en: &str,
        #[case] expected_fr: &str,
        #[case] expected_es: &str,
        #[case] expected_it: &str,
    ) {
        assert_eq!(expected_de, subject(template, EmailLanguage::De));
        assert_eq!(expected_en, subject(template, EmailLanguage::En));
        assert_eq!(expected_fr, subject(template, EmailLanguage::Fr));
        assert_eq!(expected_es, subject(template, EmailLanguage::Es));
        assert_eq!(expected_it, subject(template, EmailLanguage::It));
    }

    #[rstest]
    #[case(Language::En, "Available", "In stock")]
    #[case(Language::De, "Verfügbar", "Auf Lager")]
    fn should_localize_watchlist_availability_change_for_recipient_language(
        #[case] language: Language,
        #[case] expected_old_availability: &str,
        #[case] expected_new_availability: &str,
    ) {
        assert_eq!(
            expected_old_availability,
            availability_text(ListingAvailability::Available, language)
        );
        assert_eq!(
            expected_new_availability,
            availability_text(ListingAvailability::InStock, language)
        );
    }

    #[rstest]
    #[case(
        Some(ContentPolicyDecision::Allowed),
        false,
        Some("https://shop.example/image.jpg")
    )]
    #[case(
        Some(ContentPolicyDecision::RequiresConsent(SensitiveContentCategory::NaziGermany)),
        false,
        None
    )]
    #[case(
        Some(ContentPolicyDecision::RequiresConsent(SensitiveContentCategory::NaziGermany)),
        true,
        Some("https://shop.example/image.jpg")
    )]
    #[case(None, false, None)]
    fn should_filter_image_url_in_email_template_data(
        #[case] content_policy: Option<ContentPolicyDecision>,
        #[case] consent: bool,
        #[case] expected_image_url: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let source = source(consent, content_policy)?;
        let data = template_data(&source, EmailLanguage::En, None);

        assert_eq!(expected_image_url, data["image_url"].as_str());
        Ok(())
    }

    #[test]
    fn should_include_frontend_product_listing_url_and_first_name_in_template_data()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = source(false, None)?;
        let data = template_data(&source, EmailLanguage::En, Some("Ada"));

        assert_eq!(
            Some("https://aura-historia.com/product-listings/antique-vase-a1b2c3"),
            data["product_listing_url"].as_str()
        );
        assert_eq!(Some("Ada"), data["first_name"].as_str());
        assert!(data.get("listing_source_slug_id").is_none());
        assert!(data.get("product_listing_title_slug_id").is_none());
        Ok(())
    }

    #[test]
    fn should_localize_template_data_with_resolved_email_language()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut source = source(false, None)?;
        source.presentation_preferences.language = Language::Zh;
        let email_language = EmailLanguage::resolve(source.presentation_preferences.language);
        let data = template_data(&source, email_language, None);

        assert_eq!(Some("Available"), data["old_availability"].as_str());
        assert_eq!(Some("In stock"), data["new_availability"].as_str());
        Ok(())
    }

    #[test]
    fn should_render_null_for_absent_watchlist_availability()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut source = source(false, None)?;
        let NotificationContent::Watchlist { change, .. } = &mut source.content else {
            return Err(
                std::io::Error::other("test source is not a watchlist notification").into(),
            );
        };
        *change = NotificationWatchlistChange::AvailabilityChange {
            old_availability: None,
            new_availability: Some(ListingAvailability::InStock),
        };

        let data = template_data(&source, EmailLanguage::En, None);

        assert!(data["old_availability"].is_null());
        assert_eq!(Some("In stock"), data["new_availability"].as_str());
        assert_eq!(
            Some("availability_change"),
            data["notification_type"].as_str()
        );
        Ok(())
    }

    #[test]
    fn should_localize_notification_title_with_resolved_email_language()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut source = source(false, None)?;
        source.presentation_preferences.language = Language::Zh;
        let NotificationContent::Watchlist { snapshot, .. } = &mut source.content else {
            return Err(
                std::io::Error::other("test source is not a watchlist notification").into(),
            );
        };
        snapshot.title = Some(HashMap::from([
            (
                Language::En,
                serde_json::from_value(json!("English title"))?,
            ),
            (
                Language::Zh,
                serde_json::from_value(json!("Chinese title"))?,
            ),
        ]));

        let email_language = EmailLanguage::resolve(source.presentation_preferences.language);
        let data = template_data(&source, email_language, None);

        assert_eq!(Some("English title"), data["title"].as_str());
        Ok(())
    }

    #[rstest]
    #[case(Currency::Eur, "10,00 €", "9,00 €")]
    #[case(Currency::Usd, "$10.00", "$9.00")]
    fn should_format_watchlist_prices_in_their_source_currency(
        #[case] currency: Currency,
        #[case] expected_old_price: &str,
        #[case] expected_new_price: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut source = source(false, None)?;
        let NotificationContent::Watchlist { change, .. } = &mut source.content else {
            return Err(
                std::io::Error::other("test source is not a watchlist notification").into(),
            );
        };
        *change = NotificationWatchlistChange::PriceChange {
            old_price: Some(Price::new(MonetaryAmount::from(1000_u64), currency).into()),
            new_price: Some(Price::new(MonetaryAmount::from(900_u64), currency).into()),
        };

        let data = template_data(&source, EmailLanguage::En, None);

        assert_eq!(Some(expected_old_price), data["old_price"].as_str());
        assert_eq!(Some(expected_new_price), data["new_price"].as_str());
        Ok(())
    }

    #[rstest]
    #[case(EmailLanguage::De, "Preis auf Anfrage")]
    #[case(EmailLanguage::En, "Price on request")]
    #[case(EmailLanguage::Fr, "Prix sur demande")]
    #[case(EmailLanguage::Es, "Precio a consultar")]
    #[case(EmailLanguage::It, "Prezzo su richiesta")]
    fn should_localize_on_request_watchlist_prices(
        #[case] language: EmailLanguage,
        #[case] expected: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut source = source(false, None)?;
        let NotificationContent::Watchlist { change, .. } = &mut source.content else {
            return Err(
                std::io::Error::other("test source is not a watchlist notification").into(),
            );
        };
        *change = NotificationWatchlistChange::PriceChange {
            old_price: None,
            new_price: Some(ProductListingPrice::OnRequest),
        };

        let data = template_data(&source, language, None);

        assert!(data["old_price"].is_null());
        assert_eq!(Some(expected), data["new_price"].as_str());
        Ok(())
    }

    #[test]
    fn should_distinguish_zero_and_absent_watchlist_prices()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut source = source(false, None)?;
        let NotificationContent::Watchlist { change, .. } = &mut source.content else {
            return Err(
                std::io::Error::other("test source is not a watchlist notification").into(),
            );
        };
        *change = NotificationWatchlistChange::PriceChange {
            old_price: None,
            new_price: Some(Price::new(MonetaryAmount::from(0_u64), Currency::Eur).into()),
        };

        let data = template_data(&source, EmailLanguage::En, None);

        assert!(data["old_price"].is_null());
        assert_eq!(Some("0,00 €"), data["new_price"].as_str());
        Ok(())
    }

    fn source(
        show_unassessed_or_sensitive_content: bool,
        content_policy: Option<ContentPolicyDecision>,
    ) -> Result<NotificationDeliverySource, Box<dyn std::error::Error>> {
        Ok(NotificationDeliverySource {
            notification_delivery_id: NotificationDeliveryId::new(),
            notification_id: NotificationId::new(),
            user_id: UserId::new(),
            channel: NotificationDeliveryChannel::Email,
            target_key: NotificationDeliveryTargetKey::primary(),
            content: NotificationContent::Watchlist {
                origin_event_id: EventId::new(),
                product_listing_id: ProductListingId::new(),
                snapshot: ProductListingNotificationSnapshot {
                    listing_source_id: ListingSourceId::new(),
                    source_listing_id: SourceListingId::try_from("source-product")
                        .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
                    listing_source_slug_id: ListingSourceSlugId::raw("test-source")
                        .unwrap_or_else(|error| panic!("valid test listing source slug: {error}")),
                    product_listing_title_slug_id: ProductListingSlugId::raw("antique-vase-a1b2c3")
                        .unwrap_or_else(|error| {
                            panic!("valid product listing title slug: {error}")
                        }),
                    listing_source_name: ListingSourceName::try_from("Test Listing Source")
                        .unwrap_or_else(|error| {
                            panic!("invalid test listing source name: {error}")
                        }),
                    title: None,
                    image: Some(Url::parse("https://shop.example/image.jpg")?),
                    content_policy,
                    url: Url::parse("https://shop.example/product")?,
                    view_url: Url::parse("https://aura-historia.example/product")?,
                },
                change: NotificationWatchlistChange::AvailabilityChange {
                    old_availability: Some(ListingAvailability::Available),
                    new_availability: Some(ListingAvailability::InStock),
                },
            },
            presentation_preferences: NotificationPresentationPreferences {
                language: Language::En,
                show_unassessed_or_sensitive_content,
            },
        })
    }
}
