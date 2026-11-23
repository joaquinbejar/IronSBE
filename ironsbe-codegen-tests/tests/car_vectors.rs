//! Golden vector tests on Real Logic's public example schemas.
//!
//! The schemas (`schemas/real-logic/`, SBE tag 1.42.0) declare no `offset`
//! attributes and no `blockLength`, and their fields are enums, a set,
//! primitive arrays and composites that nest an enum, refer to types
//! declared after them and carry constants. The codecs generated from them
//! must lay the message out as SBE 1.0 does: these tests encode the values
//! of karyos#646 and compare the bytes with the reference tool's
//! (`tests/vectors/`, see `schemas/real-logic/README.md`), then decode them
//! back.

use ironsbe_codegen_tests::{car_v0, car_v1};
use ironsbe_core::decoder::SbeDecoder;

/// The bytes of a golden vector file (one line of lowercase hex).
fn vector(hex: &str) -> Vec<u8> {
    let hex = hex.trim();
    (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).expect("hex digit pair"))
        .collect()
}

/// `someNumbers`: the uint32 array 1, 2, 3, 4, as its bytes.
fn some_numbers() -> Vec<u8> {
    [1u32, 2, 3, 4]
        .iter()
        .flat_map(|n| n.to_le_bytes())
        .collect()
}

/// The fuel figures of the vectors: speed, mpg and usage description.
const FUEL: [(u16, f32, &[u8]); 3] = [
    (30, 35.9, b"Urban Cycle"),
    (55, 49.0, b"Combined Cycle"),
    (75, 40.0, b"Highway Cycle"),
];

/// The performance figures: octane rating and (mph, seconds) accelerations.
const PERFORMANCE: [(u8, [(u16, f32); 3]); 2] = [
    (95, [(30, 4.0), (60, 7.5), (100, 12.2)]),
    (99, [(30, 3.8), (60, 7.1), (100, 11.8)]),
];

/// Encodes the vectors' `Car` with the codecs of one schema version; the
/// two versions share every accessor but `uuid` and `cupHolderCount`.
macro_rules! encode_car {
    ($module:ident, $buf:expr, $extend:expr) => {{
        use $module::*;
        let mut car = CarEncoder::wrap($buf, 0).expect("wrap");
        car.set_serial_number(1234)
            .set_model_year(2013)
            .set_available(BooleanType::T)
            .set_code(Model::A)
            .set_some_numbers(&some_numbers())
            .set_vehicle_code(b"abcdef");
        let mut extras = OptionalExtras::new();
        extras.set_sports_pack();
        extras.set_cruise_control();
        car.set_extras(extras);
        {
            let mut engine = car.set_engine();
            engine
                .set_capacity(2000)
                .set_num_cylinders(4)
                .set_manufacturer_code(b"123")
                .set_efficiency(35)
                .set_booster_enabled(BooleanType::T);
            engine
                .booster()
                .set_boost_type(BoostType::NITROUS)
                .set_horse_power(200);
        }
        $extend(&mut car);
        {
            let mut fuel = car.fuel_figures_count(3).expect("fuelFigures");
            for (speed, mpg, usage) in FUEL {
                fuel.next_entry()
                    .expect("entry fits")
                    .expect("fuel entry")
                    .set_speed(speed)
                    .set_mpg(mpg)
                    .set_usage_description(usage);
            }
        }
        {
            let mut perf = car
                .performance_figures_count(2)
                .expect("performanceFigures");
            for (octane, accelerations) in PERFORMANCE {
                let mut entry = perf.next_entry().expect("entry fits").expect("entry");
                entry.set_octane_rating(octane);
                let mut acceleration = entry.acceleration_count(3).expect("acceleration");
                for (mph, seconds) in accelerations {
                    acceleration
                        .next_entry()
                        .expect("entry fits")
                        .expect("acceleration entry")
                        .set_mph(mph)
                        .set_seconds(seconds);
                }
            }
        }
        car.set_manufacturer(b"Honda")
            .set_model(b"Civic VTi")
            .set_activation_code(b"abcdef");
        car.encoded_length()
    }};
}

/// Checks the decoded fields both versions share.
macro_rules! check_car {
    ($module:ident, $decoder:expr) => {{
        use $module::*;
        let car = $decoder;
        assert_eq!(car.serial_number(), 1234);
        assert_eq!(car.model_year(), 2013);
        assert_eq!(car.available(), BooleanType::T);
        assert_eq!(car.code(), Model::A);
        assert_eq!(car.discounted_model(), Model::C, "a constant field");
        assert_eq!(car.some_numbers(), some_numbers().as_slice());
        assert_eq!(car.vehicle_code(), b"abcdef");
        let extras = car.extras();
        assert!(!extras.is_sun_roof());
        assert!(extras.is_sports_pack());
        assert!(extras.is_cruise_control());
        let engine = car.engine();
        assert_eq!(engine.capacity(), 2000);
        assert_eq!(engine.num_cylinders(), 4);
        assert_eq!(engine.max_rpm(), 9000, "a constant member");
        assert_eq!(engine.manufacturer_code(), b"123");
        assert_eq!(engine.fuel(), "Petrol", "a constant member");
        assert_eq!(engine.efficiency(), 35);
        assert_eq!(engine.booster_enabled(), BooleanType::T);
        assert_eq!(engine.booster().boost_type(), BoostType::NITROUS);
        assert_eq!(engine.booster().horse_power(), 200);
        let fuel: Vec<_> = car
            .fuel_figures()
            .expect("fuelFigures")
            .map(|e| e.expect("fuel entry"))
            .map(|e| {
                (
                    e.speed(),
                    e.mpg(),
                    e.usage_description().expect("usage").to_vec(),
                )
            })
            .collect();
        assert_eq!(
            fuel,
            FUEL.iter()
                .map(|(s, m, u)| (*s, *m, u.to_vec()))
                .collect::<Vec<_>>()
        );
        let perf: Vec<_> = car
            .performance_figures()
            .expect("performanceFigures")
            .map(|e| e.expect("performance entry"))
            .map(|e| {
                let accelerations: Vec<_> = e
                    .acceleration()
                    .expect("acceleration")
                    .map(|a| a.expect("acceleration entry"))
                    .map(|a| (a.mph(), a.seconds()))
                    .collect();
                (e.octane_rating(), accelerations)
            })
            .collect();
        assert_eq!(
            perf,
            PERFORMANCE
                .iter()
                .map(|(o, a)| (*o, a.to_vec()))
                .collect::<Vec<_>>()
        );
        assert_eq!(car.manufacturer().expect("manufacturer"), b"Honda");
        assert_eq!(car.model().expect("model"), b"Civic VTi");
        assert_eq!(car.activation_code().expect("activationCode"), b"abcdef");
    }};
}

#[test]
fn test_car_v0_layout_follows_sbe_1_0() {
    use car_v0::*;
    // 8 + 2 + 1 + 1 + 16 + 6 + 1 + 10, the constant `discountedModel` none.
    assert_eq!(CarEncoder::BLOCK_LENGTH, 45);
    assert_eq!(Engine::ENCODED_LENGTH, 10);
    assert_eq!(Booster::ENCODED_LENGTH, 2);
}

#[test]
fn test_car_v1_layout_adds_its_versioned_fields() {
    use car_v1::*;
    // Version 0's 45 bytes, then `uuid` (int64 x 2) and `cupHolderCount`.
    assert_eq!(CarEncoder::BLOCK_LENGTH, 62);
}

#[test]
fn test_car_v0_encodes_the_reference_tools_bytes() {
    let expected = vector(include_str!("vectors/car-v0.hex"));
    let mut buf = [0u8; 512];
    let len = encode_car!(car_v0, &mut buf, |_: &mut car_v0::CarEncoder<'_>| {});
    assert_eq!(len, 207);
    assert_eq!(&buf[..len], expected.as_slice());
}

#[test]
fn test_car_v0_decodes_the_reference_tools_bytes() {
    let bytes = vector(include_str!("vectors/car-v0.hex"));
    let decoder = car_v0::CarDecoder::decode(&bytes).expect("decode Car v0");
    check_car!(car_v0, &decoder);
    assert_eq!(
        decoder.encoded_length().expect("encoded length"),
        bytes.len()
    );
}

#[test]
fn test_car_v1_encodes_the_reference_tools_bytes() {
    let expected = vector(include_str!("vectors/car-v1.hex"));
    let mut buf = [0u8; 512];
    let uuid: Vec<u8> = [7i64, 3].iter().flat_map(|n| n.to_le_bytes()).collect();
    let len = encode_car!(car_v1, &mut buf, |car: &mut car_v1::CarEncoder<'_>| {
        car.set_uuid(&uuid).set_cup_holder_count(5);
    });
    assert_eq!(len, 224);
    assert_eq!(&buf[..len], expected.as_slice());
}

#[test]
fn test_car_v1_decodes_the_reference_tools_bytes() {
    let bytes = vector(include_str!("vectors/car-v1.hex"));
    let decoder = car_v1::CarDecoder::decode(&bytes).expect("decode Car v1");
    check_car!(car_v1, &decoder);
    let uuid: Vec<u8> = [7i64, 3].iter().flat_map(|n| n.to_le_bytes()).collect();
    assert_eq!(decoder.uuid(), Some(uuid.as_slice()));
    assert_eq!(decoder.cup_holder_count(), Some(5));
    assert_eq!(
        decoder.encoded_length().expect("encoded length"),
        bytes.len()
    );
}

#[test]
fn test_car_v1_codecs_read_a_version_0_car() {
    // A version 1 decoder reads a version 0 message: its fixed block is the
    // shorter one, and the fields added in version 1 are absent.
    let bytes = vector(include_str!("vectors/car-v0.hex"));
    let decoder = car_v1::CarDecoder::decode(&bytes).expect("decode a v0 Car with v1 codecs");
    check_car!(car_v1, &decoder);
    assert_eq!(decoder.uuid(), None);
    assert_eq!(decoder.cup_holder_count(), None);
}
