use std::env;
use std::path::PathBuf;

use agent_bench::{compare_files, load_tasks, run_mock, write_jsonl, BenchmarkConfig};

fn usage() -> ! {
    eprintln!(
        "usage:\n  agent-bench run --tasks PATH --output PATH [--models PATH] [--model ALIAS] [--repetitions N]\n  agent-bench compare BASELINE.jsonl CURRENT.jsonl"
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
            let model_path = value(&args, "--models").map(PathBuf::from);
            let model_alias = value(&args, "--model").unwrap_or_else(|| "local-mock".to_string());
            let repetitions = value(&args, "--repetitions")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(1);
            if repetitions == 0 {
                eprintln!("--repetitions must be positive");
                std::process::exit(2);
            }
            let tasks = load_tasks(&tasks_path).unwrap_or_else(|error| {
                eprintln!("failed to load tasks: {error}");
                std::process::exit(1);
            });
            let config =
                BenchmarkConfig::from_path(model_path.as_deref()).unwrap_or_else(|error| {
                    eprintln!("failed to load model profiles: {error}");
                    std::process::exit(1);
                });
            let records =
                run_mock(&tasks, &config, &model_alias, repetitions).unwrap_or_else(|error| {
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
