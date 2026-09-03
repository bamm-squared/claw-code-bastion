use std::env;
use std::path::PathBuf;

use agent_bench::{
    compare_files, load_tasks, run_mock, run_production, select_tasks, write_jsonl, BenchmarkConfig,
};

fn usage() -> ! {
    eprintln!(
        "usage:\n  agent-bench run --tasks PATH --output PATH [--task ID] [--models PATH] [--settings PATH] [--model ALIAS] [--repetitions N] [--execution mock|production] [--binary PATH] [--runtime-image REF] [--validator-image REF] [--task-timeout SECONDS] [--exploration-timeout SECONDS] [--interactive]\n  agent-bench compare BASELINE.jsonl CURRENT.jsonl"
    );
    std::process::exit(2);
}

fn value(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("run") => {
            let tasks_path = value(&args, "--tasks").map_or_else(|| usage(), PathBuf::from);
            let output_path = value(&args, "--output").map_or_else(|| usage(), PathBuf::from);
            let task_id = value(&args, "--task");
            let model_path = value(&args, "--models").map(PathBuf::from);
            let model_alias = value(&args, "--model");
            let settings_path = value(&args, "--settings").map(PathBuf::from);
            let repetitions = value(&args, "--repetitions")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(1);
            let execution = value(&args, "--execution").unwrap_or_else(|| "mock".to_string());
            let binary = value(&args, "--binary").map(PathBuf::from);
            let runtime_image = value(&args, "--runtime-image");
            let validator_image = value(&args, "--validator-image");
            let task_timeout = value(&args, "--task-timeout").and_then(|value| value.parse().ok());
            let exploration_timeout =
                value(&args, "--exploration-timeout").and_then(|value| value.parse().ok());
            let dry_run = args.iter().any(|arg| arg == "--dry-run");
            let interactive = args.iter().any(|arg| arg == "--interactive");
            if repetitions == 0 {
                eprintln!("--repetitions must be positive");
                std::process::exit(2);
            }
            let tasks = load_tasks(&tasks_path)
                .and_then(|tasks| select_tasks(&tasks, task_id.as_deref()))
                .unwrap_or_else(|error| {
                    eprintln!("failed to load tasks: {error}");
                    std::process::exit(1);
                });
            let config =
                BenchmarkConfig::from_path(model_path.as_deref()).unwrap_or_else(|error| {
                    eprintln!("failed to load model profiles: {error}");
                    std::process::exit(1);
                });
            let records = match execution.as_str() {
                "mock" => run_mock(
                    &tasks,
                    &config,
                    model_alias.as_deref().unwrap_or("local-mock"),
                    repetitions,
                ),
                "production" => run_production(
                    &tasks,
                    &config,
                    model_alias.as_deref(),
                    repetitions,
                    binary.as_deref(),
                    runtime_image.as_deref(),
                    settings_path.as_deref(),
                    validator_image.as_deref(),
                    task_timeout,
                    exploration_timeout,
                    dry_run,
                    interactive,
                ),
                other => {
                    eprintln!("--execution must be mock or production, got {other:?}");
                    std::process::exit(2);
                }
            }
            .unwrap_or_else(|error| {
                eprintln!("benchmark failed: {error}");
                std::process::exit(1);
            });
            write_jsonl(&output_path, &records).unwrap_or_else(|error| {
                eprintln!("failed to write benchmark output: {error}");
                std::process::exit(1);
            });
            println!(
                "wrote {} benchmark records to {}",
                records.len(),
                output_path.display()
            );
        }
        Some("compare") => {
            if args.len() != 3 {
                usage();
            }
            let report = compare_files(&PathBuf::from(&args[1]), &PathBuf::from(&args[2]))
                .unwrap_or_else(|error| {
                    eprintln!("comparison failed: {error}");
                    std::process::exit(1);
                });
            print!("{report}");
        }
        _ => usage(),
    }
}
