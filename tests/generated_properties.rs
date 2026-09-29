#[path = "support/generated.rs"]
mod generated;

const SEEDS: [u64; 8] = [
    0,
    1,
    42,
    65535,
    0x123456789abcdef0,
    0x8000000000000000,
    0xdeadbeef,
    u64::MAX,
];
const CASES: u64 = 128;
const PARSER_SEEDS: [&[u8]; 5] = [
    include_bytes!("../fuzz/seeds/parse/values.botwork"),
    include_bytes!("../fuzz/seeds/parse/control.botwork"),
    include_bytes!("../fuzz/seeds/parse/calls.botwork"),
    include_bytes!("../fuzz/seeds/parse/unicode.botwork"),
    include_bytes!("../fuzz/seeds/parse/imports.botwork"),
];

fn campaign(check: impl Fn(&[u8])) {
    let seeds = std::env::var("BOTWORK_PROPERTY_SEED")
        .map(|seed| vec![seed.parse::<u64>().expect("decimal u64 seed")])
        .unwrap_or_else(|_| SEEDS.to_vec());
    let selected = std::env::var("BOTWORK_PROPERTY_CASE")
        .map(|case| case.parse::<u64>().expect("decimal u64 case index"))
        .ok();
    for seed in seeds {
        let cases: Box<dyn Iterator<Item = u64>> = match selected {
            Some(case) => Box::new(std::iter::once(case)),
            None => Box::new(0..CASES),
        };
        for case in cases {
            let bytes = generated::case_bytes(seed, case);
            let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(&bytes)));
            assert!(checked.is_ok(), "replay with BOTWORK_PROPERTY_SEED={seed} BOTWORK_PROPERTY_CASE={case}; input={bytes:02x?}");
        }
    }
}

#[test]
fn integer_expressions_match_independent_wide_arithmetic() {
    // The DSL specifies zero for this remainder even though Rust i32 traps.
    generated::expression(include_bytes!("../fuzz/seeds/expression/minimum-remainder"));
    campaign(generated::expression);
}

#[test]
fn generated_literals_preserve_types_unicode_and_float_bits() {
    campaign(generated::literals);
}

#[test]
fn parser_seed_corpus_is_valid() {
    for seed in PARSER_SEEDS {
        assert!(generated::parse(seed));
    }
}

#[test]
fn parser_mutations_preserve_bounded_results_and_source_coordinates() {
    campaign(|bytes| {
        let mut input = PARSER_SEEDS[bytes[0] as usize % PARSER_SEEDS.len()].to_vec();
        for mutation in bytes[1..].as_chunks::<3>().0.iter().take(8) {
            let index = mutation[1] as usize % (input.len() + 1);
            match mutation[0] % 4 {
                0 => input.insert(index, mutation[2]),
                1 if index < input.len() => {
                    input.remove(index);
                }
                2 if index < input.len() => input[index] = mutation[2],
                _ => input.truncate(index),
            }
        }
        generated::parse(&input);
    });
}
