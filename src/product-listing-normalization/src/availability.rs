use product_listing_core::listing_availability::ListingAvailability;
use regex::{Error as RegexError, RegexSet};
use std::sync::LazyLock;

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

pub const MAX_AVAILABILITY_TEXT_BYTES: usize = 512;

const AVAILABLE_REGEX_PATTERNS: &[&str] = &[
    r"\b[1-9][0-9]*\s+(available|remaining|left|in stock|on hand)\b",
    r"\b(only|just)\s+[1-9][0-9]*\s+(remaining|left)\b",
    r"\b[1-9][0-9]*\s+(verfügbar|vorrätig|auf lager|en stock|disponibles?|disponibili|en existencia|en existencias|em estoque|em stock|op voorraad|i lager|på lager|varastossa|w magazynie|skladem|na sklade|în stoc|raktáron)\b",
    r"\b[1-9][0-9]*\s+(в наличии|на складе|в наявності|на складі|在庫あり|在庫有り|재고 있음|有货|有庫存)\b",
    r"\b(quedan|restan|rimangono|resten|bleiben|verbleibend|осталось|залишилось)\s+[1-9][0-9]*\b",
];

const OUT_OF_STOCK_REGEX_PATTERNS: &[&str] = &[
    r"\b0\s+(available|remaining|left|in stock|on hand)\b",
    r"\b0\s+(verfügbar|vorrätig|auf lager|en stock|disponibles?|disponibili|en existencia|en existencias|em estoque|em stock|op voorraad|i lager|på lager|varastossa|w magazynie|skladem|na sklade|în stoc|raktáron)\b",
    r"\b0\s+(в наличии|на складе|в наявності|на складі|在庫あり|在庫有り|재고 있음|有货|有庫存)\b",
];

// Both results are immutable after their synchronized first construction.
static AVAILABLE_REGEX_SET: LazyLock<Result<RegexSet, RegexError>> =
    LazyLock::new(|| compile_regex_set(AVAILABLE_REGEX_PATTERNS));
static OUT_OF_STOCK_REGEX_SET: LazyLock<Result<RegexSet, RegexError>> =
    LazyLock::new(|| compile_regex_set(OUT_OF_STOCK_REGEX_PATTERNS));

// Test-only inspection avoids timing or allocator thresholds on the warm path.
#[cfg(test)]
static REGEX_SET_INITIALIZATIONS: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingAvailabilityQuickCheck {
    Resolved(ListingAvailability),
    NoAssertion,
    Unsupported,
}

impl ListingAvailabilityQuickCheck {
    pub const fn availability(self) -> Option<ListingAvailability> {
        match self {
            Self::Resolved(value) => Some(value),
            Self::NoAssertion | Self::Unsupported => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AvailabilityNormalizationError {
    #[error("availability input exceeds the maximum length")]
    InputTooLong { len: usize, max: usize },
    #[error("availability input contains an embedded NUL")]
    EmbeddedNul,
    #[error("availability regex set configuration is invalid")]
    RegexSetCompilationFailed,
}

/// Recognizes generic availability evidence without provider mapping or I/O.
///
/// The check accepts short, self-contained status labels. It intentionally does not
/// infer availability from arbitrary product prose: unknown or contradictory input is
/// `Unsupported`, so callers cannot accidentally clear or overwrite canonical state.
pub fn quick_check_availability(
    raw: &str,
) -> Result<ListingAvailabilityQuickCheck, AvailabilityNormalizationError> {
    if raw.len() > MAX_AVAILABILITY_TEXT_BYTES {
        return Err(AvailabilityNormalizationError::InputTooLong {
            len: raw.len(),
            max: MAX_AVAILABILITY_TEXT_BYTES,
        });
    }
    if raw.contains('\0') {
        return Err(AvailabilityNormalizationError::EmbeddedNul);
    }
    quick_check_availability_with_regex_sets(
        raw,
        LazyLock::force(&AVAILABLE_REGEX_SET).as_ref(),
        LazyLock::force(&OUT_OF_STOCK_REGEX_SET).as_ref(),
    )
}

fn quick_check_availability_with_regex_sets(
    raw: &str,
    available_regex_set: Result<&RegexSet, &RegexError>,
    out_of_stock_regex_set: Result<&RegexSet, &RegexError>,
) -> Result<ListingAvailabilityQuickCheck, AvailabilityNormalizationError> {
    let available_regex_set = available_regex_set
        .map_err(|_| AvailabilityNormalizationError::RegexSetCompilationFailed)?;
    let out_of_stock_regex_set = out_of_stock_regex_set
        .map_err(|_| AvailabilityNormalizationError::RegexSetCompilationFailed)?;

    let raw_value = raw.trim();
    if raw_value.is_empty() {
        return Ok(ListingAvailabilityQuickCheck::NoAssertion);
    }
    if let Some(result) = schema_org_availability(raw_value) {
        return Ok(result);
    }

    let value = normalize_availability_text(raw_value);
    if let Some(result) = exact_availability(value.as_str()) {
        return Ok(result);
    }
    if let Some(result) =
        regex_availability(value.as_str(), available_regex_set, out_of_stock_regex_set)
    {
        return Ok(result);
    }

    Ok(ListingAvailabilityQuickCheck::Unsupported)
}

/// Scraped labels commonly contain non-breaking whitespace and decorative separators.
/// Normalize only layout and case; words, accents, and scripts remain intact.
fn normalize_availability_text(raw: &str) -> String {
    let mut normalized = String::with_capacity(raw.len());
    let mut needs_space = false;

    for character in raw.trim().chars().flat_map(char::to_lowercase) {
        if character.is_whitespace()
            || matches!(
                character,
                '-' | '_' | '/' | '\\' | '‐' | '‑' | '‒' | '–' | '—' | '−'
            )
        {
            needs_space = !normalized.is_empty();
        } else {
            if needs_space {
                normalized.push(' ');
                needs_space = false;
            }
            // Turkish capital dotted I case-folds to `i` plus this combining mark.
            if character != '\u{307}' {
                normalized.push(character);
            }
        }
    }

    normalized
}

fn schema_org_availability(value: &str) -> Option<ListingAvailabilityQuickCheck> {
    let tail = value.rsplit('/').next().unwrap_or(value);
    let result = match tail {
        "InStock" => ListingAvailabilityQuickCheck::Resolved(ListingAvailability::InStock),
        "LimitedAvailability" => {
            ListingAvailabilityQuickCheck::Resolved(ListingAvailability::LimitedAvailability)
        }
        "BackOrder" => ListingAvailabilityQuickCheck::Resolved(ListingAvailability::BackOrder),
        "MadeToOrder" => ListingAvailabilityQuickCheck::Resolved(ListingAvailability::MadeToOrder),
        "PreOrder" => ListingAvailabilityQuickCheck::Resolved(ListingAvailability::PreOrder),
        "PreSale" => ListingAvailabilityQuickCheck::Resolved(ListingAvailability::PreSale),
        "Reserved" => ListingAvailabilityQuickCheck::Resolved(ListingAvailability::Reserved),
        "OutOfStock" => ListingAvailabilityQuickCheck::Resolved(ListingAvailability::OutOfStock),
        "SoldOut" => ListingAvailabilityQuickCheck::Resolved(ListingAvailability::SoldOut),
        "OnlineOnly" | "InStoreOnly" | "Discontinued" => ListingAvailabilityQuickCheck::NoAssertion,
        _ if value.contains("schema.org/") => ListingAvailabilityQuickCheck::Unsupported,
        _ => return None,
    };
    Some(result)
}

fn exact_availability(value: &str) -> Option<ListingAvailabilityQuickCheck> {
    let availability = if contains(IN_STOCK, value) {
        ListingAvailability::InStock
    } else if contains(AVAILABLE, value) {
        ListingAvailability::Available
    } else if contains(LIMITED_AVAILABILITY, value) {
        ListingAvailability::LimitedAvailability
    } else if contains(BACK_ORDER, value) {
        ListingAvailability::BackOrder
    } else if contains(MADE_TO_ORDER, value) {
        ListingAvailability::MadeToOrder
    } else if contains(PRE_ORDER, value) {
        ListingAvailability::PreOrder
    } else if contains(PRE_SALE, value) {
        ListingAvailability::PreSale
    } else if contains(UNAVAILABLE, value) {
        ListingAvailability::Unavailable
    } else if contains(RESERVED, value) {
        ListingAvailability::Reserved
    } else if contains(OUT_OF_STOCK, value) {
        ListingAvailability::OutOfStock
    } else if contains(SOLD_OUT, value) {
        ListingAvailability::SoldOut
    } else if contains(NO_ASSERTION, value) {
        return Some(ListingAvailabilityQuickCheck::NoAssertion);
    } else {
        return None;
    };

    Some(ListingAvailabilityQuickCheck::Resolved(availability))
}

fn contains(values: &[&str], value: &str) -> bool {
    values.contains(&value)
}

// Generic availability and purchase actions. The remaining lists are intentionally
// exact labels, not substring rules, because a scraped description may mention both
// availability and an unrelated historical state.
const AVAILABLE: &[&str] = &[
    "available",
    "available now",
    "currently available",
    "ready to buy",
    "buy now",
    "add to cart",
    "add to basket",
    "add to bag",
    "add to trolley",
    "add to shopping cart",
    "in den warenkorb",
    "verfügbar",
    "lieferbar",
    "disponible",
    "disponível",
    "disponibile",
    "disponibel",
    "beschikbaar",
    "tillgänglig",
    "tilgængelig",
    "tilgjengelig",
    "saatavilla",
    "dostępny",
    "dostupné",
    "dostupny",
    "disponibil",
    "elérhető",
    "διαθέσιμο",
    "mevcut",
    "в наличии",
    "доступно",
    "в наявності",
    "доступно",
    "наявний",
    "доступний",
    "dostupno",
    "na voljo",
    "достъпно",
    "可用",
    "有货",
    "有貨",
    "購入可能",
    "재고 있음",
    "구매 가능",
    "متاح",
    "متوفر",
    "זמין",
    "उपलब्ध",
    "tersedia",
    "có sẵn",
    "còn hàng",
    "พร้อมจำหน่าย",
];

const IN_STOCK: &[&str] = &[
    "in stock",
    "in inventory",
    "in store",
    "on hand",
    "stock available",
    "auf lager",
    "vorrätig",
    "lagernd",
    "en stock",
    "en inventaire",
    "en existencia",
    "en existencias",
    "en inventario",
    "en almacén",
    "em estoque",
    "em stock",
    "op voorraad",
    "voorradig",
    "i lager",
    "på lager",
    "varastossa",
    "w magazynie",
    "na stanie",
    "skladem",
    "na sklade",
    "în stoc",
    "raktáron",
    "σε απόθεμα",
    "stokta",
    "на складе",
    "в наличии на складе",
    "на складі",
    "у наявності",
    "на склад",
    "na zalihi",
    "na stanju",
    "na zalogi",
    "в наличност",
    "在庫",
    "在庫あり",
    "在庫有り",
    "库存",
    "庫存",
    "有庫存",
    "재고 있음",
    "재고 보유",
    "متوفر في المخزون",
    "במלאי",
    "в наличии",
    "स्टॉक में",
    "ada stok",
    "còn kho",
    "มีสินค้าในสต็อก",
];

const LIMITED_AVAILABILITY: &[&str] = &[
    "limited availability",
    "limited stock",
    "limited quantity",
    "low stock",
    "few left",
    "limited",
    "begrenzt verfügbar",
    "begrenzte verfügbarkeit",
    "begrenzter bestand",
    "stock limité",
    "quantité limitée",
    "disponibilidad limitada",
    "existencias limitadas",
    "disponibilidade limitada",
    "estoque limitado",
    "disponibilità limitata",
    "scorte limitate",
    "beperkte beschikbaarheid",
    "beperkte voorraad",
    "begränsad tillgänglighet",
    "begränsat lager",
    "begrænset tilgængelighed",
    "begrænset lager",
    "begrenset tilgjengelighet",
    "begrenset lager",
    "rajoitettu saatavuus",
    "rajoitettu varasto",
    "ograniczona dostępność",
    "ograniczony stan",
    "omezená dostupnost",
    "omezené množství",
    "obmedzená dostupnosť",
    "disponibilitate limitată",
    "stoc limitat",
    "korlátozott elérhetőség",
    "korlátozott készlet",
    "περιορισμένη διαθεσιμότητα",
    "sınırlı stok",
    "ограниченное наличие",
    "ограниченный запас",
    "обмежена наявність",
    "обмежений запас",
    "ograničena dostupnost",
    "ограничена наличност",
    "数量限定",
    "残りわずか",
    "限量",
    "限量供應",
    "재고 한정",
    "수량 한정",
    "كمية محدودة",
    "מלאי מוגבל",
    "सीमित उपलब्धता",
    "stok terbatas",
    "số lượng có hạn",
    "จำนวนจำกัด",
];

const BACK_ORDER: &[&str] = &[
    "back order",
    "backorder",
    "on backorder",
    "back ordered",
    "backordered",
    "nachbestellung",
    "nachbestellbar",
    "rückstand",
    "en réapprovisionnement",
    "en commande",
    "sur commande",
    "bajo pedido",
    "pendiente de reposición",
    "por encomenda",
    "sob encomenda",
    "su ordinazione",
    "in riordino",
    "nabestelling",
    "in nabestelling",
    "restorder",
    "restordre",
    "restordre",
    "jälkitoimitus",
    "na zamówienie",
    "zamówienie oczekujące",
    "na objednávku",
    "na objednavku",
    "la comandă",
    "la comanda",
    "utánrendelés",
    "utánrendelhető",
    "κατόπιν παραγγελίας",
    "sipariş üzerine",
    "предзаказ поставщику",
    "под заказ",
    "під замовлення",
    "po narudžbi",
    "по поръчка",
    "取り寄せ",
    "お取り寄せ",
    "入荷待ち",
    "예약 주문",
    "طلب مسبق",
    "طلب مؤجل",
    "בהזמנה",
    "בהזמנה חוזרת",
    "पुनः ऑर्डर",
    "pesan ulang",
    "đặt hàng trước",
    "สั่งจอง",
];

const MADE_TO_ORDER: &[&str] = &[
    "made to order",
    "made on demand",
    "manufactured to order",
    "custom made",
    "made for you",
    "auf bestellung gefertigt",
    "anfertigung auf bestellung",
    "maßanfertigung",
    "fabriqué sur commande",
    "fait sur commande",
    "hecho por encargo",
    "fabricado por encargo",
    "feito por encomenda",
    "feito sob encomenda",
    "realizzato su ordinazione",
    "fatto su ordinazione",
    "op bestelling gemaakt",
    "gemaakt op bestelling",
    "tillverkas på beställning",
    "fremstilles på bestilling",
    "laget på bestilling",
    "tilauksesta valmistettu",
    "produkowany na zamówienie",
    "vyrobeno na zakázku",
    "vyrobené na objednávku",
    "fabricat la comandă",
    "gyártás rendelésre",
    "κατασκευή κατόπιν παραγγελίας",
    "siparişe özel üretim",
    "изготовление на заказ",
    "виготовлення на замовлення",
    "izrađeno po narudžbi",
    "изработка по поръчка",
    "受注生産",
    "オーダーメイド",
    "주문 제작",
    "يصنع حسب الطلب",
    "מיוצר לפי הזמנה",
    "ऑर्डर पर बनाया गया",
    "dibuat sesuai pesanan",
    "làm theo đơn đặt hàng",
    "ผลิตตามสั่ง",
];

const PRE_ORDER: &[&str] = &[
    "pre order",
    "preorder",
    "available for pre order",
    "pre purchase",
    "advance order",
    "vorbestellung",
    "vorbestellbar",
    "précommande",
    "precommande",
    "preorden",
    "pre pedido",
    "pré encomenda",
    "pre encomenda",
    "preordine",
    "voorbestelling",
    "förbeställning",
    "forudbestilling",
    "forhåndsbestilling",
    "ennakkotilaus",
    "przedsprzedaż",
    "przedsprzedaz",
    "předobjednávka",
    "predobjednavka",
    "predobjednávka",
    "precomandă",
    "precomanda",
    "előrendelés",
    "προπαραγγελία",
    "ön sipariş",
    "предзаказ",
    "передзамовлення",
    "prednarudžba",
    "предварителна поръчка",
    "予約注文",
    "予約販売",
    "사전 주문",
    "예약 구매",
    "طلب مسبق",
    "הזמנה מוקדמת",
    "प्री ऑर्डर",
    "preorder",
    "đặt trước",
    "สั่งซื้อล่วงหน้า",
];

const PRE_SALE: &[&str] = &[
    "pre sale",
    "presale",
    "pre sale only",
    "early sale",
    "advance sale",
    "vorverkauf",
    "im vorverkauf",
    "prévente",
    "prevente",
    "venta anticipada",
    "pre venta",
    "pré venda",
    "prevenda",
    "prevendita",
    "voorverkoop",
    "förköp",
    "forsalg",
    "forhåndssalg",
    "ennakkomyynti",
    "przedsprzedaż",
    "předprodej",
    "predpredaj",
    "prevânzare",
    "prevanzare",
    "elővétel",
    "προπώληση",
    "ön satış",
    "предпродажа",
    "передпродаж",
    "pretprodaja",
    "предпродажба",
    "先行販売",
    "사전 판매",
    "بيع مسبق",
    "מכירה מוקדמת",
    "पूर्व बिक्री",
    "penjualan awal",
    "bán trước",
    "ขายล่วงหน้า",
];

const UNAVAILABLE: &[&str] = &[
    "unavailable",
    "not available",
    "currently unavailable",
    "temporarily unavailable",
    "not purchasable",
    "nicht verfügbar",
    "derzeit nicht verfügbar",
    "non disponible",
    "indisponible",
    "no disponible",
    "no está disponible",
    "não disponível",
    "indisponível",
    "non disponibile",
    "niet beschikbaar",
    "inte tillgänglig",
    "ej tillgänglig",
    "ikke tilgængelig",
    "ikke tilgjengelig",
    "ei saatavilla",
    "niedostępny",
    "nedostępny",
    "není dostupné",
    "neni dostupne",
    "nie je dostupné",
    "indisponibil",
    "nem elérhető",
    "μη διαθέσιμο",
    "mevcut değil",
    "недоступно",
    "недоступен",
    "недоступно",
    "недоступний",
    "недоступна",
    "nije dostupno",
    "ni na voljo",
    "недостъпно",
    "不可用",
    "使用不可",
    "利用不可",
    "구매 불가",
    "사용 불가",
    "غير متاح",
    "غير متوفر",
    "לא זמין",
    "उपलब्ध नहीं",
    "tidak tersedia",
    "không có sẵn",
    "ไม่มีสินค้า",
];

const RESERVED: &[&str] = &[
    "reserved",
    "on hold",
    "held",
    "pending reservation",
    "reserviert",
    "zurückgelegt",
    "réservé",
    "reserve",
    "reservado",
    "reservada",
    "riservato",
    "riservata",
    "gereserveerd",
    "reserverad",
    "reserveret",
    "reservert",
    "varattu",
    "zarezerwowany",
    "zarezerwowano",
    "rezervováno",
    "rezervovane",
    "rezervat",
    "fenntartva",
    "δεσμευμένο",
    "ayırtıldı",
    "зарезервировано",
    "зарезервований",
    "rezervisano",
    "rezervirano",
    "резервирано",
    "予約済み",
    "取り置き",
    "예약됨",
    "예약 완료",
    "محجوز",
    "שמור",
    "आरक्षित",
    "dipesan",
    "đã đặt trước",
    "จองแล้ว",
];

const OUT_OF_STOCK: &[&str] = &[
    "out of stock",
    "out of inventory",
    "no stock",
    "not in stock",
    "stock unavailable",
    "nicht auf lager",
    "nicht vorrätig",
    "aus dem lager",
    "rupture de stock",
    "hors stock",
    "sin existencias",
    "sin stock",
    "fuera de stock",
    "sem estoque",
    "sem stock",
    "non disponibile a magazzino",
    "esaurito a magazzino",
    "niet op voorraad",
    "niet voorradig",
    "slut i lager",
    "ej i lager",
    "ikke på lager",
    "ikke i lager",
    "ikke på lager",
    "ei varastossa",
    "brak w magazynie",
    "brak na stanie",
    "není skladem",
    "neni skladem",
    "nie je na sklade",
    "nu este în stoc",
    "nu este in stoc",
    "nincs raktáron",
    "εκτός αποθέματος",
    "stokta yok",
    "нет в наличии",
    "нет на складе",
    "немає в наявності",
    "немає на складі",
    "nema na stanju",
    "ni na zalogi",
    "няма наличност",
    "在庫切れ",
    "在庫なし",
    "无库存",
    "無庫存",
    "缺货",
    "품절 임박",
    "재고 없음",
    "غير متوفر في المخزون",
    "غير موجود بالمخزون",
    "אזל מהמלאי",
    "स्टॉक में नहीं",
    "stok habis",
    "hết hàng",
    "หมดสต็อก",
];

const SOLD_OUT: &[&str] = &[
    "sold",
    "sold out",
    "soldout",
    "all sold",
    "fully sold",
    "gone",
    "verkauft",
    "ausverkauft",
    "vergriffen",
    "épuisé",
    "epuise",
    "vendu",
    "agotado",
    "agotada",
    "vendido",
    "vendida",
    "esgotado",
    "esgotada",
    "venduto",
    "venduta",
    "esaurito",
    "esaurita",
    "uitverkocht",
    "verkocht",
    "slutsåld",
    "udsolgt",
    "utsolgt",
    "loppuunmyyty",
    "wyprzedany",
    "wyprzedane",
    "vyprodáno",
    "vypredané",
    "epuizat",
    "epuizată",
    "eladva",
    "elfogyott",
    "εξαντλήθηκε",
    "tükendi",
    "satıldı",
    "распродано",
    "продано",
    "розпродано",
    "продано",
    "rasprodano",
    "prodano",
    "разпродадено",
    "完売",
    "売り切れ",
    "售罄",
    "售完",
    "已售完",
    "품절",
    "매진",
    "نفد",
    "تم البيع",
    "אזל",
    "נמכר",
    "बिक गया",
    "बिक चुका",
    "habis terjual",
    "đã bán hết",
    "หมด",
    "ขายหมดแล้ว",
];

const NO_ASSERTION: &[&str] = &[
    "listed",
    "listing",
    "gelistet",
    "listé",
    "liste",
    "listado",
    "inserito",
    "elencato",
    "catalogued",
    "cataloged",
    "online only",
    "in store only",
    "discontinued",
    "eingestellt",
    "arrêté",
    "descatalogado",
    "fuori catalogo",
    "uit assortiment",
];

fn regex_availability(
    value: &str,
    available_regex_set: &RegexSet,
    out_of_stock_regex_set: &RegexSet,
) -> Option<ListingAvailabilityQuickCheck> {
    match (
        available_regex_set.is_match(value),
        out_of_stock_regex_set.is_match(value),
    ) {
        (true, false) => Some(ListingAvailabilityQuickCheck::Resolved(
            ListingAvailability::Available,
        )),
        (false, true) => Some(ListingAvailabilityQuickCheck::Resolved(
            ListingAvailability::OutOfStock,
        )),
        // A label with both zero and positive quantity evidence is corrupt or ambiguous.
        (true, true) => Some(ListingAvailabilityQuickCheck::Unsupported),
        (false, false) => None,
    }
}

fn compile_regex_set(patterns: &[&str]) -> Result<RegexSet, RegexError> {
    let regex_set = RegexSet::new(patterns);

    #[cfg(test)]
    if regex_set.is_ok() {
        REGEX_SET_INITIALIZATIONS.fetch_add(1, Ordering::SeqCst);
    }

    regex_set
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(
        availability: ListingAvailability,
    ) -> Result<ListingAvailabilityQuickCheck, AvailabilityNormalizationError> {
        Ok(ListingAvailabilityQuickCheck::Resolved(availability))
    }

    #[test]
    fn should_resolve_common_multilingual_status_labels() {
        for (raw, expected) in [
            (" verfügbar ", ListingAvailability::Available),
            ("在庫あり", ListingAvailability::InStock),
            ("en stock", ListingAvailability::InStock),
            (
                "begränsad tillgänglighet",
                ListingAvailability::LimitedAvailability,
            ),
            ("под заказ", ListingAvailability::BackOrder),
            ("fabriqué sur commande", ListingAvailability::MadeToOrder),
            ("ön sipariş", ListingAvailability::PreOrder),
            ("предпродажа", ListingAvailability::PreSale),
            ("غير متاح", ListingAvailability::Unavailable),
            ("zarezerwowany", ListingAvailability::Reserved),
            ("在庫切れ", ListingAvailability::OutOfStock),
            ("품절", ListingAvailability::SoldOut),
        ] {
            assert_eq!(quick_check_availability(raw), resolved(expected), "{raw}");
        }
    }

    #[test]
    fn should_normalize_scraped_whitespace_case_and_separators() {
        assert_eq!(
            quick_check_availability("  PRE\u{a0}ORDER  "),
            resolved(ListingAvailability::PreOrder)
        );
        assert_eq!(
            quick_check_availability("OUT-OF_STOCK"),
            resolved(ListingAvailability::OutOfStock)
        );
        assert_eq!(
            quick_check_availability("İNDİSPONİBLE"),
            resolved(ListingAvailability::Unavailable)
        );
    }

    #[test]
    fn should_resolve_multilingual_quantity_evidence() {
        for raw in [
            "Only 2 remaining",
            "3 en stock",
            "quedan 4",
            "осталось 5",
            "6 在庫あり",
        ] {
            assert_eq!(
                quick_check_availability(raw),
                resolved(ListingAvailability::Available),
                "{raw}"
            );
        }
        for raw in ["0 available", "0 en stock", "0 в наличии", "0 在庫あり"] {
            assert_eq!(
                quick_check_availability(raw),
                resolved(ListingAvailability::OutOfStock),
                "{raw}"
            );
        }
    }

    #[test]
    fn should_not_guess_when_quantity_evidence_conflicts() {
        assert_eq!(
            quick_check_availability("0 available; only 2 remaining"),
            Ok(ListingAvailabilityQuickCheck::Unsupported)
        );
    }

    #[test]
    fn should_fail_closed_without_a_state_decision_when_regex_constants_are_invalid()
    -> Result<(), RegexError> {
        let available_regex_set = RegexSet::new(AVAILABLE_REGEX_PATTERNS)?;
        let invalid_out_of_stock_regex_set = compile_regex_set(&[r"["]);

        for value in ["sold out", "Only 2 remaining", "0 available"] {
            assert_eq!(
                quick_check_availability_with_regex_sets(
                    value,
                    Ok(&available_regex_set),
                    invalid_out_of_stock_regex_set.as_ref(),
                ),
                Err(AvailabilityNormalizationError::RegexSetCompilationFailed)
            );
        }

        Ok(())
    }

    #[test]
    fn should_fail_closed_for_blank_input_when_a_regex_set_is_invalid() -> Result<(), RegexError> {
        let available_regex_set = RegexSet::new(AVAILABLE_REGEX_PATTERNS)?;
        let out_of_stock_regex_set = RegexSet::new(OUT_OF_STOCK_REGEX_PATTERNS)?;
        let invalid_regex_set = compile_regex_set(&[r"["]);

        for value in ["", " \t\r\n"] {
            assert_eq!(
                quick_check_availability_with_regex_sets(
                    value,
                    invalid_regex_set.as_ref(),
                    Ok(&out_of_stock_regex_set),
                ),
                Err(AvailabilityNormalizationError::RegexSetCompilationFailed)
            );
            assert_eq!(
                quick_check_availability_with_regex_sets(
                    value,
                    Ok(&available_regex_set),
                    invalid_regex_set.as_ref(),
                ),
                Err(AvailabilityNormalizationError::RegexSetCompilationFailed)
            );
        }

        Ok(())
    }

    #[test]
    fn should_initialize_regex_sets_once_and_reuse_them_when_called_concurrently() {
        let start = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let start = &start;
                scope.spawn(move || {
                    start.wait();
                    for _ in 0..64 {
                        assert_eq!(
                            quick_check_availability("Only 2 remaining"),
                            resolved(ListingAvailability::Available)
                        );
                        assert_eq!(
                            quick_check_availability("0 available"),
                            resolved(ListingAvailability::OutOfStock)
                        );
                    }
                });
            }
        });

        let initialization_count = REGEX_SET_INITIALIZATIONS.load(Ordering::SeqCst);
        assert_eq!(initialization_count, 2);
        assert!(std::ptr::eq(
            LazyLock::force(&AVAILABLE_REGEX_SET),
            LazyLock::force(&AVAILABLE_REGEX_SET)
        ));
        assert!(std::ptr::eq(
            LazyLock::force(&OUT_OF_STOCK_REGEX_SET),
            LazyLock::force(&OUT_OF_STOCK_REGEX_SET)
        ));
    }

    #[test]
    fn should_resolve_schema_org_availability() {
        assert_eq!(
            quick_check_availability("https://schema.org/OutOfStock"),
            resolved(ListingAvailability::OutOfStock)
        );
    }

    #[test]
    fn should_return_no_assertion_for_explicit_absence() {
        assert_eq!(
            quick_check_availability("listed"),
            Ok(ListingAvailabilityQuickCheck::NoAssertion)
        );
    }

    #[test]
    fn should_return_unsupported_for_unknown_or_prose_values() {
        for raw in ["limited availability soon", "This antique was sold in 1970"] {
            assert_eq!(
                quick_check_availability(raw),
                Ok(ListingAvailabilityQuickCheck::Unsupported),
                "{raw}"
            );
        }
    }

    #[test]
    fn should_reject_invalid_input() {
        let overlong = "x".repeat(MAX_AVAILABILITY_TEXT_BYTES + 1);
        assert!(matches!(
            quick_check_availability(overlong.as_str()),
            Err(AvailabilityNormalizationError::InputTooLong { .. })
        ));
        assert!(matches!(
            quick_check_availability("\0"),
            Err(AvailabilityNormalizationError::EmbeddedNul)
        ));
    }
}
