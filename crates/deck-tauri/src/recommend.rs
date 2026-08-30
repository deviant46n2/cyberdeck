pub fn recommend(workload: String, objective: String) -> Result<Vec<deck_core::recommend::RankedCandidate>, String> {
    deck_core::recommend::recommend(&workload, &objective).map_err(|e| e.to_string())
}
