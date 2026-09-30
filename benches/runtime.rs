//! Correctness-checked workload driver; orchestrated by scripts/performance.py.
use botwork::core::{
    ast::Program,
    ast_limits::{AstLimits, DEFAULT_AST_NODES},
    grammar::Literal,
    operation::NativeOperation,
    run::{Engine, RunLimits, RunOptions, RunOutcome, RunResult},
    signature::{StatementSignature, ValueKind},
    syntax_limits::{SyntaxLimits, DEFAULT_SOURCE_BYTES},
};
use clap::Parser;
use std::{
    collections::BTreeMap,
    fs,
    hint::black_box,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[path = "runtime/probe.rs"]
mod probe;

#[derive(Parser)]
struct Args {
    workload: String,
    source: PathBuf,
    // Up to the largest registered size: the 1,000,000-iteration loop at scale 4.
    #[arg(value_parser = clap::value_parser!(u32).range(1..=4_000_000))]
    units: u32,
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=1000))]
    wait_ms: u32,
}

fn options() -> RunOptions {
    RunOptions {
        inherit_environment: false,
        limits: RunLimits {
            steps: 64_000_000,
            ..RunLimits::default()
        },
        ..RunOptions::default()
    }
}

fn answer(result: RunResult, expected: i32) -> u64 {
    assert_eq!(result.outcome(), RunOutcome::Succeeded, "{result:?}");
    assert!(result.snapshot_error.is_none(), "{result:?}");
    assert!(matches!(result.variables["answer"], Literal::Int(value) if value == expected));
    black_box(expected as u64)
}

struct Active(Arc<AtomicUsize>);

impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn waiting(program: Arc<Program>, count: usize, wait_ms: u32) -> u64 {
    assert!(count <= 400, "registered workloads bound concurrent runs");
    let active = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&active);
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::clone(&gate);
    let (entered, mut ready) = tokio::sync::mpsc::channel(count);
    let operation = NativeOperation::asynchronous(
        StatementSignature::native("Await |value|")
            .unwrap()
            .parameter("value", ValueKind::Int)
            .unwrap()
            .returns(ValueKind::Int),
        move |mut values, _| {
            let active = Arc::clone(&observed);
            let gate = Arc::clone(&gate);
            let entered = entered.clone();
            async move {
                active.fetch_add(1, Ordering::SeqCst);
                let _active = Active(active);
                entered.try_send(()).unwrap();
                let _permit = gate.acquire().await.unwrap();
                Ok(values.remove(0))
            }
        },
    )
    .unwrap();
    let ownership = operation.ownership_budget().clone();
    let mut engine = Engine::default();
    engine.register_operation(operation).unwrap();
    let engine = Arc::new(engine);
    let mut runs = tokio::task::JoinSet::new();
    for id in 0..count {
        let engine = Arc::clone(&engine);
        let program = Arc::clone(&program);
        runs.spawn(async move {
            let result = engine
                .run_program_async(
                    &program,
                    RunOptions {
                        variables: BTreeMap::from([("input".into(), Literal::Int(id as i32))]),
                        ..options()
                    },
                )
                .await;
            answer(result, id as i32)
        });
    }
    for _ in 0..count {
        ready.recv().await.expect("every run must reach its gate");
    }
    assert_eq!(active.load(Ordering::SeqCst), count);
    assert_eq!(ownership.usage().invocations, count);
    tokio::time::sleep(Duration::from_millis(u64::from(wait_ms))).await;
    release.add_permits(count);
    let mut checksum = 0;
    while let Some(result) = runs.join_next().await {
        checksum += result.unwrap();
    }
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert_eq!(ownership.usage(), Default::default());
    checksum
}

fn main() {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--probe")) {
        std::process::exit(probe::main());
    }
    let args = Args::parse();
    // Input generation/read, runtime construction, and reusable parsing are
    // outside workload timing. Process wall time and peak RSS include them.
    let source = fs::read_to_string(&args.source).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let engine = Engine::default();
    let (elapsed, checksum) = match args.workload.as_str() {
        "parse" => {
            // Scaled parses outgrow the default AST node budget (three nodes per
            // statement), so it is raised only above the default, never below.
            let ast_limits = AstLimits {
                nodes: DEFAULT_AST_NODES.max(4 * args.units as usize),
                ..AstLimits::default()
            };
            let start = Instant::now();
            let program = Program::parse_with_budgets(
                "parse",
                black_box(&source),
                DEFAULT_SOURCE_BYTES,
                &SyntaxLimits::default(),
                &ast_limits,
            )
            .unwrap();
            let elapsed = start.elapsed();
            assert_eq!(program.statements.len(), args.units as usize);
            (elapsed, black_box(program.statements.len() as u64))
        }
        "calls" | "loop" => {
            let program = Program::parse_detailed(&args.workload, &source).unwrap();
            let start = Instant::now();
            let result = runtime.block_on(engine.run_program_async(&program, options()));
            let elapsed = start.elapsed();
            (elapsed, answer(result, args.units as i32))
        }
        "source-io" => {
            let start = Instant::now();
            let checksum = runtime.block_on(async {
                let mut checksum = 0;
                for _ in 0..args.units {
                    checksum += answer(engine.run_file_async(&args.source, options()).await, 42);
                }
                checksum
            });
            (start.elapsed(), checksum)
        }
        "waiting" => {
            let program = Arc::new(Program::parse_detailed("waiting", &source).unwrap());
            let start = Instant::now();
            let checksum = runtime.block_on(waiting(program, args.units as usize, args.wait_ms));
            (start.elapsed(), checksum)
        }
        other => panic!("unknown workload: {other}"),
    };
    println!(
        "{}",
        serde_json::json!({
            "schema": 1,
            "workload": args.workload,
            "units": args.units,
            "elapsed_ns": u64::try_from(elapsed.as_nanos()).unwrap(),
            "checksum": checksum,
        })
    );
}
