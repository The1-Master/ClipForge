//! Real-demo acceptance runner. It parses locally and never launches the game.
use anyhow::Result;
use demodesk_core::{parser::DemoParser, stats::build_parsed_demo};
use serde_json::json;
use std::{path::PathBuf, time::Instant};

fn main() -> Result<()> {
    let path = PathBuf::from(std::env::args().nth(1).expect("demo path"));
    let destination = PathBuf::from(std::env::args().nth(2).expect("output JSON path"));
    let start = Instant::now();
    let data = DemoParser::new().load_demo(&path)?;
    let parsed_seconds = start.elapsed().as_secs_f64();
    let raw_deaths = data.kills.len();
    let ledger_deaths = data.recorded_deaths.len();
    let excluded_clutch_rounds = data.clutch_ineligible_rounds.clone();
    let parsed = build_parsed_demo(data);
    let output = json!({
        "parse_seconds": parsed_seconds,
        "total_seconds": start.elapsed().as_secs_f64(),
        "raw_deaths": raw_deaths,
        "ledger_deaths": ledger_deaths,
        "death_highlights": parsed.highlights.iter().filter(|h| h.tags.iter().any(|t| t == "death")).count(),
        "excluded_clutch_rounds": excluded_clutch_rounds,
        "parsed": parsed,
    });
    std::fs::write(&destination, serde_json::to_vec_pretty(&output)?)?;
    println!(
        "{}",
        json!({ "parse_seconds": parsed_seconds, "raw_deaths": raw_deaths, "ledger_deaths": ledger_deaths, "output": destination })
    );
    Ok(())
}
