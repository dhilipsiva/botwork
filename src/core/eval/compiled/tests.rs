use super::*;

fn limits() -> ParseLimits {
    ParseLimits {
        source_bytes: 1024,
        syntax: SyntaxLimits::default(),
        ast: AstLimits::default(),
    }
}

fn parse(cache: &CompiledModules, name: &str, source: &str, limits: ParseLimits) -> Arc<Program> {
    cache
        .get_or_parse(name, source, limits, || {
            Program::parse(name, source).map_err(|error| error.to_string())
        })
        .unwrap()
}

#[test]
fn identical_text_and_limits_share_one_tree() {
    let cache = CompiledModules::default();
    let first = parse(&cache, "m.botwork", "Log |1|", limits());
    let second = parse(&cache, "m.botwork", "Log |1|", limits());
    assert!(Arc::ptr_eq(&first, &second));
    let changed = parse(&cache, "m.botwork", "Log |2|", limits());
    assert!(!Arc::ptr_eq(&first, &changed), "changed text parses again");
    let tighter = ParseLimits {
        syntax: SyntaxLimits {
            nesting: 8,
            ..SyntaxLimits::default()
        },
        ..limits()
    };
    let other = parse(&cache, "m.botwork", "Log |1|", tighter);
    assert!(!Arc::ptr_eq(&first, &other), "other limits parse again");
    let renamed = parse(&cache, "n.botwork", "Log |1|", limits());
    assert!(
        !Arc::ptr_eq(&first, &renamed),
        "trees carry their source name"
    );
    assert_eq!(
        cache.statistics(),
        CompiledStatistics {
            modules: 4,
            source_bytes: 28,
            hits: 1,
            misses: 4,
            evictions: 0
        }
    );
}

#[test]
fn failures_are_not_kept_and_oversized_modules_are_not_kept() {
    let cache = CompiledModules::with_capacity(8);
    let failed: Result<Arc<Program>, &str> =
        cache.get_or_parse("bad.botwork", "Log |", limits(), || Err("syntax"));
    assert_eq!(failed.err(), Some("syntax"));
    parse(&cache, "big.botwork", "Log |123456|", limits());
    // An oversized parse is never inserted, so nothing is evicted either.
    assert_eq!(
        cache.statistics(),
        CompiledStatistics {
            modules: 0,
            source_bytes: 0,
            hits: 0,
            misses: 2,
            evictions: 0
        }
    );
}

#[test]
fn the_least_recently_used_parse_is_evicted_first() {
    let cache = CompiledModules::with_capacity(14);
    let a = parse(&cache, "a.botwork", "Log |1|", limits());
    parse(&cache, "b.botwork", "Log |2|", limits());
    // Touch a, so b is the least recently used when c arrives.
    assert!(Arc::ptr_eq(
        &a,
        &parse(&cache, "a.botwork", "Log |1|", limits())
    ));
    parse(&cache, "c.botwork", "Log |3|", limits());
    let statistics = cache.statistics();
    assert_eq!((statistics.modules, statistics.source_bytes), (2, 14));
    assert_eq!(statistics.evictions, 1);
    assert!(Arc::ptr_eq(
        &a,
        &parse(&cache, "a.botwork", "Log |1|", limits())
    ));
    let misses = cache.statistics().misses;
    parse(&cache, "b.botwork", "Log |2|", limits());
    assert_eq!(cache.statistics().misses, misses + 1, "b was evicted");
    cache.clear();
    assert_eq!(cache.statistics().modules, 0);
    assert!(format!("{cache:?}").starts_with("CompiledModules("));
}
