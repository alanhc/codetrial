//! Scratch check: one real `generate_report` call through
//! `CODETRIAL_GEMINI_REST_BASE`. Not part of the gate.
use codetrial::agent::get_problem;
use codetrial::gemini::generate_report;

#[tokio::test]
#[ignore = "needs a generateContent server at CODETRIAL_GEMINI_REST_BASE"]
async fn local_report() {
    let prompt = match std::env::var("REPORT_PROMPT_FILE") {
        Ok(path) => std::fs::read_to_string(path).unwrap(),
        Err(_) => {
            let golden: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string("tests/golden/prompts.json").unwrap(),
            )
            .unwrap();
            golden["report"].as_str().unwrap().to_string()
        }
    };
    let started = std::time::Instant::now();
    let report = generate_report("local", "local", &prompt, get_problem(Some("two-sum")))
        .await
        .expect("report");
    println!("elapsed {:.1}s", started.elapsed().as_secs_f64());
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
