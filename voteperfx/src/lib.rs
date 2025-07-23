// mimalloc
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

pub mod config;
pub mod dashboard;
pub mod error;
pub mod message;
pub mod performance;
pub mod vote_tracker;
//pub mod simd_utils;

pub use config::{Config, PerformanceFilterConfig};
pub use dashboard::DashboardRenderer;
pub use error::{Result, VoteMonitorError};
pub use performance::{
    ConfirmedVote, PerformanceStats, TvcPerformanceLevel, PoorPerformanceEvent,
    calculate_tvc_credits_from_latency, calculate_tvc_credits, categorize_tvc_performance,
    format_duration, format_number, Slot,
    VOTE_CREDITS_GRACE_SLOTS, VOTE_CREDITS_MAXIMUM_PER_SLOT,
};
pub use vote_tracker::{
    VoteTracker, VoteSlotInfo, PendingVote, VoteTrackerStats,
    parse_vote_instruction_data, process_vote_transaction, process_finalized_block,
    VOTE_PROGRAM_ID,
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub async fn log_simple_transaction(stats: &PerformanceStats, confirmed_vote: &ConfirmedVote) {
    let efficiency = stats.calculate_efficiency();
    
    log::info!(
        "vote confirmed: slot {} → latency {} → {} TVC | TX: https://solscan.io/tx/{}", 
        confirmed_vote.voted_slot, 
        confirmed_vote.latency, 
        confirmed_vote.tvc_credits,
        confirmed_vote.signature
    );
    log::info!(
        "session stats: {} votes, {:.1}% efficiency, {} total tvc earned",
        stats.total_transactions(), 
        efficiency, 
        stats.total_tvc_earned()
    );
    log::info!("---");
}

pub fn print_banner() {
    println!("solana monitor v{}", VERSION);
    println!();
}

pub fn print_help(program_name: &str) {
    print_banner();
    println!("usage:");
    println!("    {} [options]", program_name);
    println!();
    println!("options:");
    println!("    --dashboard    interactive dashboard with real-time metrics (default)");
    println!("    --simple       simple cli logging mode");
    println!("    --help, -h     show this help message");
    println!();
    println!("configuration:");
    println!("    config.toml    all configuration including:");
    println!("                   - grpc_url: yellowstone grpc endpoint");
    println!("                   - vote_account: vote account to monitor");
    println!("                   - performance_logging: logging filters");
    println!();
    println!("for more information, see: https://github.com/1000xsh/voteperfx");
}


pub fn init_logging(simple_mode: bool) {
    if simple_mode {
        std::env::set_var("RUST_LOG", "info");
    } else {
        std::env::set_var("RUST_LOG", "warn");
    }
    pretty_env_logger::init();
}