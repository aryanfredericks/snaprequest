//! Sends a GET request and prints the status, timings and trace.
//!
//! ```sh
//! cargo run -p snapreq-core --example get -- http://example.com/
//! ```

use snapreq_core::{Client, Request};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "http://example.com/".into());
    let client = Client::default();

    match client.execute(&Request::get(url)).await {
        Ok(resp) => {
            println!(
                "{} {} {}",
                resp.version,
                resp.status,
                resp.status_text().unwrap_or("")
            );
            println!("{} bytes from {:?}\n", resp.body.len(), resp.remote_addr);
            for entry in &resp.trace {
                println!(
                    "{:>9.3} ms  {:?}",
                    entry.at.as_secs_f64() * 1000.0,
                    entry.event
                );
            }
            println!("\n{}", serde_json::to_string_pretty(&resp.timings)?);
        }
        Err(e) => {
            eprintln!("error: {e}");
            for entry in e.trace().unwrap_or_default() {
                eprintln!(
                    "{:>9.3} ms  {:?}",
                    entry.at.as_secs_f64() * 1000.0,
                    entry.event
                );
            }
            std::process::exit(1);
        }
    }
    Ok(())
}
