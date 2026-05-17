mod config;
mod logging;

fn main() {
    logging::init();
    tracing::info!(event = "startup", "shoebox-server starting");
    println!("(stub: nothing running yet)");
}
