//! fds-detect: hardware detection and config tooling (sub-project 3).
//! No Python; pure `std` + `serde_json`. Deterministic on a given machine:
//! same machine + same inputs -> same output.
//!
//! Usage:
//!   fds-detect                             print the detection summary
//!   fds-detect --emit-config PATH          write ./config.json (defaults with
//!                                          D-1-derived socket buffers)
//!   fds-detect --generate-schema PATH      write ./config/config.schema.json
//!   fds-detect --validate-config FILE      validate a config.json against the
//!                                          engine's config surface

mod config_model;
mod detect;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--emit-config") => emit_config(args.get(1)),
        Some("--generate-schema") => generate_schema(args.get(1)),
        Some("--validate-config") => validate_config(args.get(1)),
        Some(other) => {
            eprintln!("fds-detect: unknown argument {other:?}");
            eprintln!("usage: fds-detect [--emit-config PATH] [--generate-schema PATH] [--validate-config FILE]");
            ExitCode::FAILURE
        }
        None => {
            print_summary(&detect::detect());
            ExitCode::SUCCESS
        }
    }
}

fn emit_config(path: Option<&String>) -> ExitCode {
    let path = path.map(String::as_str).unwrap_or("config.json");
    let hw = detect::detect();
    match std::fs::write(path, config_model::emit_config(hw.l3_bytes)) {
        Ok(()) => {
            println!(
                "fds-detect: wrote {path} (D-1 socket buffers from L3: {} bytes)",
                hw.l3_bytes.map_or_else(|| "unknown".to_string(), |n| n.to_string())
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("fds-detect: {path}: {e}");
            ExitCode::FAILURE
        }
    }
}

fn generate_schema(path: Option<&String>) -> ExitCode {
    let path = path.map(String::as_str).unwrap_or("config/config.schema.json");
    match std::fs::write(path, config_model::generate_schema()) {
        Ok(()) => {
            println!("fds-detect: wrote {path}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("fds-detect: {path}: {e}");
            ExitCode::FAILURE
        }
    }
}

fn validate_config(path: Option<&String>) -> ExitCode {
    let Some(path) = path else {
        eprintln!("usage: fds-detect --validate-config <file>");
        return ExitCode::FAILURE;
    };
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let errors = config_model::validate_config(&text);
            if errors.is_empty() {
                println!("fds-detect: {path}: valid");
                ExitCode::SUCCESS
            } else {
                for e in &errors {
                    eprintln!("fds-detect: {e}");
                }
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("fds-detect: {path}: {e}");
            ExitCode::FAILURE
        }
    }
}

fn print_summary(hw: &detect::Hardware) {
    println!("fds-detect: hardware summary (deterministic; see build/detect.sh)");
    println!("  cpu:      {}", if hw.model.is_empty() { "unknown" } else { &hw.model });
    println!("  vendor:   {}", if hw.vendor.is_empty() { "unknown" } else { &hw.vendor });
    let tpc = hw
        .threads_per_core()
        .map_or_else(|| "?".to_string(), |n| n.to_string());
    println!(
        "  cores:    {} logical / {} physical ({} threads/core)",
        hw.logical_cores.map_or_else(|| "?".to_string(), |n| n.to_string()),
        hw.physical_cores.map_or_else(|| "?".to_string(), |n| n.to_string()),
        tpc
    );
    println!(
        "  simd:     {}",
        if hw.simd.is_empty() { "none detected".to_string() } else { hw.simd.join(", ") }
    );
    println!(
        "  l3:       {}",
        hw.l3_bytes.map_or_else(|| "unknown".to_string(), |n| format!("{n} bytes"))
    );
    println!(
        "  numa:     {} node(s)",
        hw.numa_nodes.map_or_else(|| "?".to_string(), |n| n.to_string())
    );
    println!(
        "  hugepages: total {} ({}), {}",
        hw.hugepages_total.map_or_else(|| "?".to_string(), |n| n.to_string()),
        hw.hugepages_free.map_or_else(|| "?".to_string(), |n| n.to_string()),
        if hw.hugepages_available() { "available" } else { "unavailable" }
    );
    println!("  suggested:");
    println!("    build:  target-cpu=native (TARGET_CPU to pin; SIMD follows automatically)");
    println!(
        "    config: core.threads 0 (= one per logical CPU); socket buffers {} B (D-1)",
        hw.l3_bytes
            .map(config_model::d1_socket_buffer_bytes)
            .map_or_else(|| "4 MiB engine default".to_string(), |n| n.to_string())
    );
    println!("    run:    fds-detect --emit-config && fds-detect --validate-config config.json");
}
