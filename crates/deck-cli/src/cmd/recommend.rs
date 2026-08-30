use anyhow::Result;

pub fn run(workload: String, objective: String, json: bool) -> Result<()> {
    let ranked = deck_core::recommend::recommend(&workload, &objective)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&ranked)?);
    } else {
        if ranked.is_empty() { println!("no candidates"); return Ok(()); }
        println!("workload={workload} objective={objective} (ranked)");
        for (i, c) in ranked.iter().enumerate() {
            println!("{}. {} — {}", i+1, c.model, c.explain);
        }
        println!("\nBest: {} — {}", ranked[0].model, ranked[0].explain);
    }
    Ok(())
}
