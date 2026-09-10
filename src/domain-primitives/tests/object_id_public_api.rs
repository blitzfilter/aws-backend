use domain_primitives::object_id::ObjectIdError;
use domain_primitives::object_id_newtype;
use std::collections::HashSet;
use std::error::Error;
use std::str::FromStr;
use uuid::Uuid;

object_id_newtype!(ExampleId, "ex");
object_id_newtype!(OtherId, "other");

const UUID_TEXT: &str = "01890a5d-ac96-774b-bf1d-d5586c639f75";
const TYPE_ID_TEXT: &str = "ex_01h455vb4pex5vy7enb1p677vn";
const TYPE_ID_SUFFIX: &str = "01h455vb4pex5vy7enb1p677vn";

fn fixture_uuid() -> Result<Uuid, uuid::Error> {
    Uuid::parse_str(UUID_TEXT)
}

fn assert_standard_traits<T: DebugCloneCopyEqOrdHash>() {}

trait DebugCloneCopyEqOrdHash: std::fmt::Debug + Clone + Copy + Eq + Ord + std::hash::Hash {}

impl<T> DebugCloneCopyEqOrdHash for T where
    T: std::fmt::Debug + Clone + Copy + Eq + Ord + std::hash::Hash
{
}

#[test]
fn should_expose_complete_public_value_api() -> Result<(), Box<dyn Error>> {
    assert_standard_traits::<ExampleId>();
    assert_eq!("ex", ExampleId::PREFIX);

    let uuid = fixture_uuid()?;
    let id = ExampleId::try_from(uuid)?;

    assert_eq!(&uuid, id.as_uuid());
    assert_eq!(uuid, id.into_uuid());
    assert_eq!(uuid, Uuid::from(id));
    assert_eq!(TYPE_ID_TEXT, id.to_string());
    assert!(format!("{id:?}").contains(TYPE_ID_TEXT));
    assert!(!format!("{id:?}").contains(UUID_TEXT));

    let mut ids = HashSet::new();
    ids.insert(id);
    assert!(ids.contains(&id));

    Ok(())
}

#[test]
fn should_generate_uuid_v7_for_new_and_default() {
    for id in [ExampleId::new(), ExampleId::default()] {
        assert_eq!(uuid::Variant::RFC4122, id.as_uuid().get_variant());
        assert_eq!(7, id.as_uuid().get_version_num());
        assert!(id.to_string().starts_with("ex_"));
    }
}

#[test]
fn should_parse_exact_official_uuidv7_vector_through_all_string_conversions()
-> Result<(), Box<dyn Error>> {
    let from_str = ExampleId::from_str(TYPE_ID_TEXT)?;
    let from_borrowed = ExampleId::try_from(TYPE_ID_TEXT)?;
    let from_owned = ExampleId::try_from(TYPE_ID_TEXT.to_owned())?;

    assert_eq!(fixture_uuid()?, from_str.into_uuid());
    assert_eq!(from_str, from_borrowed);
    assert_eq!(from_str, from_owned);

    Ok(())
}

#[test]
fn should_report_wrong_prefix_for_valid_foreign_prefix() {
    let error = ExampleId::from_str(&format!("other_{TYPE_ID_SUFFIX}"));

    assert!(matches!(
        error,
        Err(ObjectIdError::WrongPrefix {
            expected: "ex",
            actual,
        }) if actual == "other"
    ));
}

#[rstest::rstest]
#[case(TYPE_ID_SUFFIX)]
#[case("")]
#[case("ex_")]
#[case("NOT AN ID")]
#[case(UUID_TEXT)]
#[case("01890A5D-AC96-774B-BF1D-D5586C639F75")]
#[case("other_not-a-typeid")]
#[case("_01h455vb4pex5vy7enb1p677vn")]
#[case("ex-01h455vb4pex5vy7enb1p677vn")]
#[case("ex__01h455vb4pex5vy7enb1p677vn")]
#[case("ex_01h455vb4pex5vy7enb1p677v")]
#[case("ex_01h455vb4pex5vy7enb1p677vnn")]
#[case("ex_01h455vb4pex5vy7enb1p677vi")]
#[case("ex_01h455vb4pex5vy7enb1p677vl")]
#[case("ex_01h455vb4pex5vy7enb1p677vo")]
#[case("ex_01h455vb4pex5vy7enb1p677vu")]
#[case("ex_01h455vb4pex5vy7enb1p677v-")]
#[case("ex_8zzzzzzzzzzzzzzzzzzzzzzzzz")]
fn should_report_malformed_for_invalid_shape_or_suffix(#[case] value: &str) {
    assert!(matches!(
        ExampleId::from_str(value),
        Err(ObjectIdError::Malformed { .. })
    ));
}

#[rstest::rstest]
#[case("EX_01h455vb4pex5vy7enb1p677vn")]
#[case("ex_01H455VB4PEX5VY7ENB1P677VN")]
fn should_report_noncanonical_for_uppercase_input(#[case] value: &str) {
    assert!(matches!(
        ExampleId::from_str(value),
        Err(ObjectIdError::NonCanonical { .. })
    ));
}

#[test]
fn should_reject_non_v7_raw_uuid() -> Result<(), Box<dyn Error>> {
    let uuid = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000")?;

    assert!(matches!(
        ExampleId::try_from(uuid),
        Err(ObjectIdError::UnsupportedUuidVersion { actual: 4 })
    ));

    Ok(())
}

#[test]
fn should_reject_non_v7_encoded_suffix() {
    assert!(matches!(
        ExampleId::from_str("ex_2n1t201rmv87aae5j4csam8000"),
        Err(ObjectIdError::UnsupportedUuidVersion { actual: 4 })
    ));
}

#[test]
fn should_reject_non_rfc_uuid_variant() -> Result<(), Box<dyn Error>> {
    let mut bytes = fixture_uuid()?.into_bytes();
    bytes[8] &= 0b0011_1111;
    let uuid = Uuid::from_bytes(bytes);

    assert!(matches!(
        ExampleId::try_from(uuid),
        Err(ObjectIdError::UnsupportedUuidVariant)
    ));

    Ok(())
}

#[test]
fn should_serialize_and_deserialize_only_canonical_typeid() -> Result<(), Box<dyn Error>> {
    let id = ExampleId::try_from(fixture_uuid()?)?;
    let json = serde_json::to_string(&id)?;
    let decoded: ExampleId = serde_json::from_str(&json)?;

    assert_eq!(format!("\"{TYPE_ID_TEXT}\""), json);
    assert_eq!(id, decoded);
    assert!(serde_json::from_str::<ExampleId>(&format!("\"{UUID_TEXT}\"")).is_err());
    assert!(serde_json::from_str::<ExampleId>("\"EX_01h455vb4pex5vy7enb1p677vn\"").is_err());

    Ok(())
}

#[test]
fn should_preserve_all_uuid_bits_across_text_and_raw_roundtrips() -> Result<(), Box<dyn Error>> {
    let uuid = fixture_uuid()?;
    let raw_roundtrip = ExampleId::try_from(uuid)?.into_uuid();
    let text_roundtrip = ExampleId::from_str(TYPE_ID_TEXT)?.into_uuid();

    assert_eq!(uuid.as_bytes(), raw_roundtrip.as_bytes());
    assert_eq!(uuid.as_bytes(), text_roundtrip.as_bytes());

    Ok(())
}

#[test]
fn should_order_by_backing_uuid() -> Result<(), Box<dyn Error>> {
    let first_uuid = Uuid::parse_str("01890a5d-ac96-774b-bf1d-d5586c639f75")?;
    let second_uuid = Uuid::parse_str("01890a5d-ac96-774b-bf1d-d5586c639f76")?;
    let first = ExampleId::try_from(first_uuid)?;
    let second = ExampleId::try_from(second_uuid)?;

    assert!(first < second);
    assert!(first.to_string() < second.to_string());

    Ok(())
}

#[test]
fn should_keep_generated_types_distinct() -> Result<(), Box<dyn Error>> {
    let uuid = fixture_uuid()?;
    let example = ExampleId::try_from(uuid)?;
    let other = OtherId::try_from(uuid)?;

    assert_ne!(example.to_string(), other.to_string());
    assert_eq!(example.into_uuid(), other.into_uuid());

    Ok(())
}

#[cfg(feature = "test-data")]
#[test]
fn should_generate_uuid_v7_with_supplied_faker_rng() {
    use fake::rand::{SeedableRng, rngs::StdRng};
    use fake::{Dummy, Faker};

    let mut first_rng = StdRng::seed_from_u64(7);
    let mut matching_rng = StdRng::seed_from_u64(7);
    let mut different_rng = StdRng::seed_from_u64(8);
    let first = ExampleId::dummy_with_rng(&Faker, &mut first_rng);
    let matching = ExampleId::dummy_with_rng(&Faker, &mut matching_rng);
    let different = ExampleId::dummy_with_rng(&Faker, &mut different_rng);

    assert_eq!(uuid_v7_random_bits(first), uuid_v7_random_bits(matching));
    assert_ne!(uuid_v7_random_bits(first), uuid_v7_random_bits(different));
    for id in [first, matching, different] {
        assert_eq!(uuid::Variant::RFC4122, id.as_uuid().get_variant());
        assert_eq!(7, id.as_uuid().get_version_num());
    }
}

#[cfg(feature = "test-data")]
fn uuid_v7_random_bits(id: ExampleId) -> [u8; 10] {
    let bytes = id.as_uuid().as_bytes();
    [
        bytes[6] & 0b0000_1111,
        bytes[7],
        bytes[8] & 0b0011_1111,
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    ]
}
